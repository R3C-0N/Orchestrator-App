<script lang="ts">
  // Fenêtre principale. J3 : conversation virtualisée alimentée par le
  // magasin de session, soit par le cœur Rust (Tauri), soit par le mode
  // simulé (`?mock=…`, ou d'office hors de Tauri : `npm run dev:mock`).
  // J7 : saisie (`Prompt.svelte`), raccourcis globaux (`state/keys.svelte.ts`),
  // interruption et changement de modèle.
  import { onMount } from "svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import Prompt from "./lib/components/Prompt.svelte";
  import StatusLine from "./lib/components/StatusLine.svelte";
  import {
    describeError,
    getLaunchInfo,
    interrupt,
    sendMessage,
    setModel,
    startSession,
    type CoreEvent,
    type SessionKey,
  } from "./lib/core/client";
  import type { ModelInfo } from "./lib/core/bindings/ModelInfo";
  import type { MockCore } from "./lib/core/mock";
  import { Keys, keyInput, type KeyAction, type Overlay } from "./lib/state/keys.svelte";
  import { SessionStore } from "./lib/state/session.svelte";

  // Mode simulé : `?mock=…`, ou d'office hors de Tauri. `mock.ts` n'est
  // chargé qu'à ce moment (import dynamique, hors du bundle principal).
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const mockMode = new URLSearchParams(location.search).has("mock") || !inTauri;

  const store = new SessionStore();
  let mock: MockCore | null = null;
  let conversation: Conversation | undefined = $state();
  let prompt: Prompt | undefined = $state();
  /** Vue détaillée (Ctrl+O, jalon J5) : un appel par ligne, sorties complètes. */
  let verbose = $state(false);

  let session: SessionKey | null = $state(null);
  let cwd = $state("");
  let startError: string | null = $state(null);
  let draft = $state("");

  // --- Commandes du cœur, réelles ou simulées --------------------------------

  /** Ce que la saisie demande au cœur ; même forme en mode simulé. */
  type Backend = {
    send(text: string): Promise<void>;
    interrupt(): Promise<void>;
    setModel(model: string): Promise<string>;
  };

  const tauriBackend: Backend = {
    send: async (text) => {
      if (session) await sendMessage(session, text);
    },
    interrupt: async () => {
      if (session) await interrupt(session, false);
    },
    setModel: async (model) => {
      if (!session) throw new Error("session non démarrée");
      return setModel(session, model);
    },
  };
  let backend: Backend = tauriBackend;

  // --- Raccourcis (décision 13) ------------------------------------------------

  const keys = new Keys();
  /** Sélecteur de modèle (Alt+P ou `/model`). */
  let pickerOpen = $state(false);
  /** Palette (J10) : point d'accroche, aucune palette n'existe encore. */
  let paletteOpen = $state(false);
  /** Message passager à droite au-dessus de la saisie (« Set model to … »). */
  let notice: string | null = $state(null);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  /** Interruption demandée, en attendant l'état `Interrupting` du cœur. */
  let interruptAsked = $state(false);

  const busy = $derived(
    store.status === "Requesting" || store.status === "Streaming" || store.status === "Interrupting",
  );
  $effect(() => {
    if (!busy) interruptAsked = false;
  });

  const overlay: Overlay = $derived(paletteOpen ? "palette" : pickerOpen ? "picker" : null);

  function showNotice(text: string): void {
    notice = text;
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 4000);
  }

  /** Texte sélectionné dans la conversation ou dans la saisie. */
  function hasSelection(): boolean {
    if (prompt?.hasSelection()) return true;
    const s = window.getSelection();
    return !!s && !s.isCollapsed && s.toString().length > 0;
  }

  /** Point d'accroche de la palette (J10) : `/` en début de saisie, Ctrl+K. */
  function openPalette(_via: "slash" | "ctrlK"): void {
    // J10 : ouvrir `Palette.svelte` ici (`paletteOpen = true`).
  }

  /** Point d'accroche de la carte de question (J8) : Échap sur une carte ignorable. */
  function skipQuestion(): void {
    // J8 : `skip_question(session, request_id, "Skipped")`.
  }

  async function doInterrupt(): Promise<void> {
    interruptAsked = true;
    try {
      await backend.interrupt();
    } catch (err) {
      interruptAsked = false;
      store.addBanner("Error", "interrupt", describeError(err));
    }
  }

  function run(action: KeyAction): void {
    switch (action.type) {
      case "interrupt":
        void doInterrupt();
        break;
      case "clear":
        draft = "";
        keys.disarm();
        break;
      case "closeOverlay":
        if (action.overlay === "picker") pickerOpen = false;
        else paletteOpen = false;
        prompt?.focus();
        break;
      case "skipQuestion":
        skipQuestion();
        break;
      case "openPalette":
        openPalette("ctrlK");
        break;
      case "openModelPicker":
        pickerOpen = true;
        prompt?.focus();
        break;
      case "toggleVerbose":
        verbose = !verbose;
        break;
    }
  }

  function onWindowKey(e: KeyboardEvent) {
    const action = keys.handle(keyInput(e), {
      hasSelection: hasSelection(),
      overlay,
      question: store.question ? { canSkip: store.question.canSkip } : null,
      busy,
      interrupting: interruptAsked || store.status === "Interrupting",
      draft,
      now: performance.now(),
    });
    if (action.type === "pass") {
      // Une frappe ailleurs (conversation sélectionnée) va dans la saisie.
      const a = document.activeElement;
      const typing = e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey;
      if (typing && !verbose && !(a instanceof HTMLTextAreaElement || a instanceof HTMLInputElement)) prompt?.focus();
      return;
    }
    // La copie reste celle du navigateur.
    if (action.type === "copy") return;
    e.preventDefault();
    run(action);
  }

  // --- Envoi ----------------------------------------------------------------

  /** Messages envoyés depuis cette saisie (historique ↑↓). */
  const sent: string[] = [];

  /** Historique : messages du fil (reprise comprise), puis ceux envoyés ici. */
  function history(): string[] {
    const out: string[] = [];
    for (const r of store.rows) if (r.kind === "user") out.push(r.text);
    // Envoyés ici mais pas encore revenus en écho dans le fil.
    for (const s of sent) if (!out.includes(s)) out.push(s);
    return out;
  }

  async function applyModel(m: ModelInfo | string): Promise<void> {
    pickerOpen = false;
    const value = typeof m === "string" ? m : m.value;
    try {
      const resolved = await backend.setModel(value);
      // `init.model` du tour suivant fera foi ; affiché tout de suite.
      store.meta.model = resolved;
      const name = typeof m === "string" ? (store.meta.models.find((x) => x.value === m)?.display_name ?? m) : (m.display_name ?? m.value);
      showNotice(`Set model to ${name}`);
    } catch (err) {
      store.addBanner("Error", "set_model", describeError(err));
    }
  }

  async function submit(text: string) {
    keys.disarm();
    const trimmed = text.trim();
    // `/model` : sélecteur de modèle (le CLI en stream-json ne l'ouvre pas).
    const model = /^\/model(?:\s+(\S+))?\s*$/.exec(trimmed);
    if (model) {
      draft = "";
      if (model[1]) void applyModel(model[1]);
      else pickerOpen = true;
      return;
    }
    draft = "";
    sent.push(trimmed);
    // Envoyer un message ramène en bas, comme le terminal.
    conversation?.scrollToBottom();
    try {
      await backend.send(trimmed);
    } catch (err) {
      store.addBanner("Error", "send", describeError(err));
    }
  }

  // --- Latence d'un token -----------------------------------------------------

  // Latence d'un token : de `t` (lecture de la ligne) à la peinture, pour
  // chaque `text_delta`. Le message posté depuis l'image où le lot est
  // appliqué s'exécute après la peinture de cette image.
  const latencies: number[] = [];
  let p50 = $state("–");
  let p95 = $state("–");
  const afterPaint = new MessageChannel();
  afterPaint.port1.onmessage = () => {
    const now = performance.timeOrigin + performance.now();
    const ts = paintQueue.shift();
    if (ts === undefined) return;
    for (const t of ts) latencies.push(now - t);
    if (latencies.length > 5000) latencies.splice(0, latencies.length - 5000);
  };
  const paintQueue: number[][] = [];
  store.onApplied = ({ deltaTs }) => {
    if (deltaTs.length === 0) return;
    paintQueue.push(deltaTs);
    afterPaint.port2.postMessage(0);
  };

  function pct(xs: number[], p: number): number {
    const s = [...xs].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
  }

  // Mise à jour du pied une fois par seconde, pas à chaque token.
  onMount(() => {
    const id = setInterval(() => {
      if (latencies.length === 0) return;
      p50 = pct(latencies, 50).toFixed(0);
      p95 = pct(latencies, 95).toFixed(0);
    }, 1000);
    return () => clearInterval(id);
  });

  let mockReady = $state(false);
  const ready = $derived(mockReady || session !== null);
  /** Texte d'invite tant qu'aucun message n'a été envoyé (`01`). */
  const placeholder = $derived(
    !ready ? "session non démarrée" : store.rows.some((r) => r.kind === "user") ? "" : 'Try "how do I log an error?"',
  );

  $effect(() => {
    if (ready) prompt?.focus();
  });

  // --- Mode simulé ------------------------------------------------------------

  /** Modèles annoncés en mode simulé (ceux de `t11`, abrégés) quand `ready` n'en donne pas. */
  const MOCK_MODELS: ModelInfo[] = [
    { value: "default", resolved: "claude-opus-5-5", display_name: "Default (recommended)" },
    { value: "sonnet", resolved: "claude-sonnet-5-5", display_name: "Sonnet 5.5" },
    { value: "haiku", resolved: "claude-haiku-5-5", display_name: "Haiku 5.5" },
  ];

  /**
   * Cœur simulé pour la saisie : `mock.ts` ne connaît ni `interrupt` ni
   * `set_model`. Ses lots passent par ce filtre, qui renumérote `seq` et
   * imite le cœur Rust sur `t6` (interruption : `Interrupting`, bloc figé,
   * `user_message Interrupted`, `turn_result aborted_streaming`, `Idle`) et
   * `t5` (`init` à chaque tour, avec le modèle choisi).
   */
  function mockBackend(m: MockCore): { backend: Backend; filter: (b: CoreEvent[]) => void } {
    let seq = 0;
    let model = "claude-opus-5-5";
    let init: Extract<CoreEvent, { kind: "init" }> | null = null;
    let aborting = false;
    const make = (body: Record<string, unknown>): CoreEvent =>
      ({ ...body, seq: 0, t: performance.timeOrigin + performance.now(), session: "mock-1" }) as unknown as CoreEvent;

    const filter = (batch: CoreEvent[]) => {
      const out: CoreEvent[] = [];
      for (const ev of batch) {
        let e = ev;
        if (e.kind === "ready" && e.models.length === 0) e = { ...e, models: MOCK_MODELS };
        if (e.kind === "init") {
          init = e;
          model = e.model ?? model;
        }
        if (e.kind === "turn_result" && aborting) {
          out.push(make({ kind: "user_message", thread: "main", uuid: null, text: "[Request interrupted by user]", origin: "Interrupted" }));
          e = { ...e, subtype: "error_during_execution", is_error: true, terminal_reason: "aborted_streaming", result: null };
        }
        if (e.kind === "status" && e.state === "Idle") aborting = false;
        out.push(e);
        // Le CLI renvoie `system/init` à chaque tour, avec le modèle en cours.
        if (e.kind === "status" && e.state === "Requesting" && init) out.push(make({ ...init, kind: "init", model }));
      }
      for (const e of out) (e as { seq: number }).seq = seq++;
      store.push(out);
    };

    return {
      filter,
      backend: {
        send: async (text) => {
          m.send(text);
        },
        interrupt: async () => {
          if (!m.streaming) return;
          aborting = true;
          filter([make({ kind: "status", state: "Interrupting" })]);
          // Délai de la réponse du CLI (≈ 100 ms dans `t6`).
          setTimeout(() => m.stopStream(), 100);
        },
        setModel: async (value) => {
          const known = store.meta.models.find((x) => x.value === value);
          model = known?.resolved ?? value;
          return model;
        },
      },
    };
  }

  /** Commandes demandées au cœur simulé (vérifiées par les tests e2e). */
  const mockCalls: { cmd: string; args: unknown }[] = [];

  async function startMock() {
    const { MockCore, mockOptionsFromUrl } = await import("./lib/core/mock");
    const opts = mockOptionsFromUrl(location.search) ?? { name: "demo", rows: 60 };
    mock = new MockCore(opts);
    cwd = `simulé : ${opts.name}`;
    const mb = mockBackend(mock);
    backend = {
      send: (t) => (mockCalls.push({ cmd: "send_message", args: { text: t } }), mb.backend.send(t)),
      interrupt: () => (mockCalls.push({ cmd: "interrupt", args: { cancel_queued: false } }), mb.backend.interrupt()),
      setModel: (v) => (mockCalls.push({ cmd: "set_model", args: { model: v } }), mb.backend.setModel(v)),
    };
    mock.start(mb.filter);
    mockReady = true;
    // Points d'accès des mesures e2e (mode simulé seulement).
    (window as unknown as Record<string, unknown>).__orch = {
      store,
      mock,
      conversation: () => conversation,
      latencies,
      calls: mockCalls,
      keys,
    };
  }

  onMount(() => {
    if (mockMode) {
      startMock().catch((err) => (startError = String(err)));
      return;
    }
    (async () => {
      try {
        const info = await getLaunchInfo();
        cwd = info.cwd;
        const started = await startSession(info.cwd, store.push);
        // Un `exited` arrivé pendant le démarrage ne doit pas être écrasé.
        if (store.status !== "Exited") session = started.session;
      } catch (err) {
        startError = describeError(err);
      }
    })();
  });
</script>

<svelte:window onkeydown={onWindowKey} />

<main>
  {#each store.banners as b (b.id)}
    <div class="banner {b.level.toLowerCase()}">{b.message}</div>
  {/each}
  {#if startError}
    <div class="banner error">{startError}</div>
  {/if}
  <Conversation {store} {verbose} bind:this={conversation} />
  <!-- Vue détaillée (Ctrl+O) : la barre « Showing detailed transcript » de la
       conversation remplace la saisie et la ligne d'état, comme le TUI (03b). -->
  {#if !verbose}
    <Prompt
      bind:this={prompt}
      bind:draft
      bind:pickerOpen
      disabled={!ready || store.status === "Exited"}
      {placeholder}
      hint={keys.hint ?? notice}
      {history}
      models={store.meta.models}
      currentModel={store.meta.model}
      onsubmit={submit}
      onslash={() => openPalette("slash")}
      onpick={(m) => void applyModel(m)}
      oninputchange={() => keys.disarm()}
    />
    <StatusLine {store} />
  {/if}
  <footer>
    <span>{cwd}</span>
    <span>{store.status} · {store.rows.length} lignes</span>
    <span>latence token p50 {p50} ms · p95 {p95} ms</span>
  </footer>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #0c0c0c;
  }
  .banner {
    padding: 0 1ch;
    color: #0c0c0c;
    background: var(--warn);
  }
  .banner.error {
    background: var(--error);
  }
  footer {
    display: flex;
    justify-content: space-between;
    gap: 2ch;
    padding: 0 1ch;
    color: #8b8b8b;
    font-size: 12px;
  }
</style>
