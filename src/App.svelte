<script lang="ts">
  // Fenêtre principale. J3 : conversation virtualisée alimentée par le
  // magasin de session, soit par le cœur Rust (Tauri), soit par le mode
  // simulé (`?mock=…`, ou d'office hors de Tauri : `npm run dev:mock`).
  // J7 : saisie (`Prompt.svelte`), raccourcis globaux (`state/keys.svelte.ts`),
  // interruption et changement de modèle.
  // J8 : carte de question `AskUserQuestion` (`QuestionCard.svelte`), à la
  // place de la saisie tant qu'une question attend.
  // J10 : palette (`Palette.svelte`) ouverte par `/` en début de saisie ou
  // Ctrl+K, remplie par `ready.commands` et complétée par `init`.
  // Colonne des agents (J9, décision 4) à droite : `.split` = `.left`
  // (conversation, saisie, ligne d'état) + `AgentColumn` ; elle reste
  // visible en vue détaillée (Ctrl+O).
  // J11 (décision 11) : lancée sans dossier en argument, l'app montre le
  // sélecteur des projets, puis celui des sessions du projet choisi
  // (nouvelle session ou reprise avec tout l'historique).
  import { onMount } from "svelte";
  import AgentColumn from "./lib/components/agents/AgentColumn.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import { entriesFrom, filterEntries, nameColumn } from "./lib/components/Palette.svelte";
  import ProjectPicker from "./lib/components/ProjectPicker.svelte";
  import Prompt from "./lib/components/Prompt.svelte";
  import QuestionCard from "./lib/components/QuestionCard.svelte";
  import SessionPicker from "./lib/components/SessionPicker.svelte";
  import StatusLine from "./lib/components/StatusLine.svelte";
  import {
    answerQuestion,
    describeError,
    getLaunchInfo,
    interrupt,
    sendMessage,
    setModel,
    skipQuestion as skipQuestionCmd,
    startSession,
    stopSession,
    type Answer,
    type CoreEvent,
    type SessionKey,
    type SkipReason,
  } from "./lib/core/client";
  import type { ModelInfo } from "./lib/core/bindings/ModelInfo";
  import type { MockCore } from "./lib/core/mock";
  import { Keys, keyInput, type KeyAction, type Overlay } from "./lib/state/keys.svelte";
  import { SessionStore } from "./lib/state/session.svelte";
  import { resumeSession } from "./lib/state/sessions.svelte";

  // Mode simulé : `?mock=…`, ou d'office hors de Tauri. `mock.ts` n'est
  // chargé qu'à ce moment (import dynamique, hors du bundle principal).
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const mockMode = new URLSearchParams(location.search).has("mock") || !inTauri;

  /**
   * Magasin de la session affichée. Une reprise en crée un neuf (plan 1.5) :
   * `$state.raw`, la réaffectation est suivie, le magasin n'est pas proxifié.
   */
  let store: SessionStore = $state.raw(newStore());

  /**
   * Magasin neuf : ses lots passent d'abord par `noteInit` (noms de `init`
   * pour la palette), puis la mesure de latence est branchée dessus.
   */
  function newStore(): SessionStore {
    const s = new SessionStore();
    const push = s.push;
    s.push = (batch) => {
      noteInit(batch);
      push(batch);
    };
    s.onApplied = ({ deltaTs }) => {
      if (deltaTs.length === 0) return;
      paintQueue.push(deltaTs);
      afterPaint.port2.postMessage(0);
    };
    return s;
  }

  /** Écran : sélecteur de projet, de session, ou la session (`start`). */
  let phase: "start" | "projects" | "sessions" = $state("start");
  /** Dossier choisi dans le sélecteur des projets. */
  let projectPath: string | null = $state(null);
  /** `LaunchInfo.launch_error` : argument donné mais inutilisable. */
  let launchError: string | null = $state(null);
  const selectorOpen = $derived(phase !== "start");
  /**
   * Historique repris tronqué (notice Info `history_truncated` du cœur,
   * rangée dans `infos`) : ligne grise discrète en tête de la conversation.
   */
  let historyCut: string | null = $state(null);

  /** Texte de la ligne « début omis » d'après les compteurs de `load_history`. */
  function historyCutText(h: { omitted_lines: number; dropped_events: number } | null): string | null {
    if (!h) return null;
    if (h.omitted_lines > 0) return `Début de l'historique omis (${h.omitted_lines} lignes)`;
    if (h.dropped_events > 0) return `Début de l'historique omis (${h.dropped_events} événements)`;
    return null;
  }
  let mock: MockCore | null = null;
  let conversation: Conversation | undefined = $state();
  let prompt: Prompt | undefined = $state();
  let card: QuestionCard | undefined = $state();
  /** Vue détaillée (Ctrl+O, jalon J5) : un appel par ligne, sorties complètes. */
  let verbose = $state(false);
  /** Agents dépliés dans la colonne (fil de l'agent, bonus de J9). */
  let agentsOpen: string[] = $state([]);

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
    answer(requestId: string, answers: Record<string, Answer>): Promise<void>;
    skip(requestId: string, reason: SkipReason): Promise<void>;
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
    answer: async (requestId, answers) => {
      if (session) await answerQuestion(session, requestId, answers, null);
    },
    skip: async (requestId, reason) => {
      if (session) await skipQuestionCmd(session, requestId, reason);
    },
  };
  let backend: Backend = tauriBackend;

  // --- Raccourcis (décision 13) ------------------------------------------------

  const keys = new Keys();
  /** Sélecteur de modèle (Alt+P ou `/model`). */
  let pickerOpen = $state(false);
  // --- Palette (J10) ---------------------------------------------------------
  //
  // Le filtre est le texte de la saisie : `/` en début de saisie ouvre la
  // palette, les caractères suivants filtrent (`10`) ; une espace, un saut de
  // ligne ou l'effacement du `/` la referment. Ctrl+K l'ouvre n'importe où :
  // le texte déjà tapé (`base`) est gardé et suivra la commande choisie, le
  // texte tapé ensuite filtre. Échap ferme sans rien insérer.

  /** Noms de `init.skills` et `init.slash_commands` (fusionnés, jamais vidés). */
  let initNames: string[] = $state([]);
  /** `/model` est traité par l'app (sélecteur) : toujours dans la palette. */
  const MODEL_ENTRY = { name: "model", description: "Set the AI model for Claude Code", argument_hint: null };
  const paletteEntries = $derived(
    entriesFrom(
      store.meta.commands.some((c) => c.name === "model") ? store.meta.commands : [...store.meta.commands, MODEL_ENTRY],
      initNames,
    ),
  );
  const paletteNameWidth = $derived(nameColumn(paletteEntries));
  /** Palette ouverte par Ctrl+K, avec le texte déjà tapé à ce moment. */
  let ctrlK: { base: string } | null = $state(null);
  /** Saisie `/…` sur laquelle Échap a fermé la palette (elle reste fermée tant que le texte ne change pas). */
  let slashDismissed: string | null = $state(null);
  let paletteSel = $state(0);

  /** Filtre en cours, ou `null` : palette fermée. */
  const paletteQuery = $derived.by((): string | null => {
    if (store.question || verbose) return null;
    if (ctrlK) {
      const rest = draft.startsWith(ctrlK.base) ? draft.slice(ctrlK.base.length) : draft;
      return rest.replace(/^\//, "");
    }
    const m = /^\/(\S*)$/.exec(draft);
    return m && draft !== slashDismissed ? m[1] : null;
  });
  const paletteItems = $derived(paletteQuery === null ? [] : filterEntries(paletteEntries, paletteQuery));
  /** Ouverte : par Ctrl+K (même sans résultat), ou par `/` avec au moins une entrée (le menu du TUI disparaît sinon). */
  const paletteOpen = $derived(paletteQuery !== null && (ctrlK !== null || paletteItems.length > 0));
  // Nouveau filtre : la sélection revient en tête.
  $effect(() => {
    void paletteQuery;
    paletteSel = 0;
  });

  function closePalette(): void {
    if (ctrlK) ctrlK = null;
    else slashDismissed = draft;
  }

  function movePalette(d: number): void {
    const n = paletteItems.length;
    if (n > 0) paletteSel = (paletteSel + d + n) % n;
  }

  /**
   * Choix d'une entrée. Entrée, comme le TUI : une commande sans argument
   * (`argument_hint` vide), ou dont le nom est déjà tapé en entier, part tout
   * de suite ; avec argument, `/nom ` est inséré pour le taper. Tab et clic insèrent toujours `/nom `. Après
   * Ctrl+K, le texte déjà tapé suit la commande (et rien ne part seul).
   */
  function choosePalette(i: number, how: "enter" | "tab" | "click"): void {
    const e = paletteItems[i];
    if (!e) return;
    const tail = ctrlK?.base ?? "";
    ctrlK = null;
    slashDismissed = null;
    // Nom tapé en entier (`/model`) : Entrée l'envoie tel quel, comme sans palette.
    const typedInFull = paletteQuery === e.name;
    if (how === "enter" && (!e.argumentHint || typedInFull) && tail === "") {
      draft = "";
      void submit(`/${e.name}`);
      return;
    }
    draft = `/${e.name} ${tail}`;
    prompt?.focusEnd();
  }

  /** Retient les noms de `init` (skills, commandes) pour la palette. */
  function noteInit(batch: CoreEvent[]): void {
    for (const ev of batch) {
      if (ev.kind !== "init") continue;
      const add = [...ev.skills, ...ev.slash_commands].filter((n) => !initNames.includes(n));
      if (add.length > 0) initNames = [...initNames, ...new Set(add)];
    }
  }
  /** Message passager à droite au-dessus de la saisie (« Set model to … »). */
  let notice: string | null = $state(null);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  /** Interruption demandée, en attendant l'état `Interrupting` du cœur. */
  let interruptAsked = $state(false);

  const busy = $derived(
    store.status === "Requesting" || store.status === "Streaming" || store.status === "Interrupting",
  );
  // Toute nouvelle valeur du statut répond à la demande (`Interrupting`,
  // `Idle`…) : on ne garde pas le drapeau au-delà.
  $effect(() => {
    void store.status;
    interruptAsked = false;
  });
  let interruptTimer: ReturnType<typeof setTimeout> | null = null;

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

  /** Palette (J10) : `/` en début de saisie, Ctrl+K. */
  function openPalette(via: "slash" | "ctrlK"): void {
    slashDismissed = null;
    if (via === "ctrlK") {
      if (!ctrlK) ctrlK = { base: draft.startsWith("/") ? "" : draft };
      pickerOpen = false;
      prompt?.focusEnd();
    }
  }

  // --- Carte de question (J8) -------------------------------------------------

  /**
   * Envoi d'une réponse ou d'un « ignorer ». La carte reste affichée jusqu'à
   * `question_closed` (émis par le cœur après l'écriture sur stdin) ; en cas
   * d'erreur, bandeau et carte de nouveau utilisable.
   */
  async function replyQuestion(kind: "answer" | "skip", send: (id: string) => Promise<void>): Promise<void> {
    const q = store.question;
    if (!q) return;
    try {
      await send(q.requestId);
    } catch (err) {
      store.addBanner("Error", kind === "answer" ? "answer_question" : "skip_question", describeError(err));
      // Même question : la carte repart de zéro (nouvel objet, même requête).
      if (store.question?.requestId === q.requestId) store.question = { ...q };
    }
  }

  function answer(answers: Record<string, Answer>): void {
    void replyQuestion("answer", (id) => backend.answer(id, answers));
  }

  /**
   * « Ignorer » (Échap, ou « Cancel » de la relecture) : `skip_question` avec
   * la raison `skipped` (`t10`), seulement si la carte est ignorable
   * (`can_skip` = capacité `ask_user_question_unanswered_v1` dans
   * `init.capabilities`) ; sinon rien (décision 13).
   */
  function skipQuestion(): void {
    if (!store.question?.canSkip) return;
    void replyQuestion("skip", (id) => backend.skip(id, "skipped"));
  }

  /**
   * « Chat about this ». Le TUI rend la main à la saisie pour en parler.
   * La raison prévue par `sdk.d.ts` (`replied_in_chat`) n'existe pas dans
   * `SkipReason` du cœur (`skipped` | `dismissed`) : choix prudent en
   * attendant, `skip_question` avec `dismissed` (la carte est fermée sans
   * réponse, Claude reçoit « The user did not answer the questions. » et
   * rien n'est inventé à sa place), puis focus dans la saisie. Sans la
   * capacité, l'entrée est grisée et inactive (aucune requête que le cœur
   * refuserait).
   */
  function chatAboutQuestion(): void {
    if (!store.question?.canSkip) return;
    void replyQuestion("skip", (id) => backend.skip(id, "dismissed"));
  }

  async function doInterrupt(): Promise<void> {
    interruptAsked = true;
    // Le cœur abandonne après 5 s (`Timeout`) : au-delà, Échap ou Ctrl+C
    // peuvent redemander l'interruption même si aucun statut n'est arrivé.
    if (interruptTimer) clearTimeout(interruptTimer);
    interruptTimer = setTimeout(() => (interruptAsked = false), 6000);
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
        prompt?.resetHistory();
        break;
      case "closeOverlay":
        if (action.overlay === "picker") pickerOpen = false;
        else closePalette();
        prompt?.focus();
        break;
      case "skipQuestion":
        skipQuestion();
        break;
      case "openPalette":
        if (!store.question) openPalette("ctrlK");
        break;
      case "openModelPicker":
        if (store.question) break;
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
      // La carte de question remplace la saisie : ni palette ni sélecteur.
      overlay: store.question ? null : overlay,
      question: store.question ? { canSkip: store.question.canSkip } : null,
      busy,
      interrupting: interruptAsked || store.status === "Interrupting",
      draft,
      now: performance.now(),
      selector: selectorOpen,
    });
    // Sélecteur de projet ou de session : ses touches lui reviennent, et
    // aucun raccourci de la saisie (palette, modèle) ne s'applique.
    if (selectorOpen && action.type !== "copy" && action.type !== "toggleVerbose") return;
    if (action.type === "pass") {
      // Carte de question : la frappe est à elle (chiffres, champ libre).
      if (store.question) {
        const inCard = e.target instanceof Element && e.target.closest('[data-testid="question-card"]') !== null;
        if (!inCard && e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) card?.focus();
        return;
      }
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

  /**
   * Valeur de `/model <valeur>` acceptée : un modèle annoncé par `ready`
   * (`value` ou `resolved`), sinon un identifiant strict (pas de `-` en tête,
   * pas d'espace ni de caractère de contrôle ; le cœur refuse aussi ces cas).
   */
  function validModel(v: string): boolean {
    if (store.meta.models.some((m) => m.value === v || m.resolved === v)) return true;
    return /^[A-Za-z0-9][A-Za-z0-9._:[\]@-]{0,99}$/.test(v);
  }

  async function submit(text: string) {
    keys.disarm();
    const trimmed = text.trim();
    // `/model` : sélecteur de modèle (le CLI en stream-json ne l'ouvre pas).
    const model = /^\/model(?:\s+(\S+))?\s*$/.exec(trimmed);
    if (model) {
      draft = "";
      if (model[1]) {
        if (validModel(model[1])) void applyModel(model[1]);
        else showNotice(`Invalid model: ${model[1]}`);
      }
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
        answer: async (requestId, answers) => m.answerQuestion(requestId, answers),
        skip: async (requestId, reason) => m.skipQuestion(requestId, reason),
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
      answer: (id, answers) => (
        mockCalls.push({ cmd: "answer_question", args: { request_id: id, answers, response: null } }),
        mb.backend.answer(id, answers)
      ),
      skip: (id, reason) => (
        mockCalls.push({ cmd: "skip_question", args: { request_id: id, reason } }), mb.backend.skip(id, reason)
      ),
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
        launchError = info.launch_error;
        if (info.cwd_from_args) await startNew(info.cwd);
        else phase = "projects";
      } catch (err) {
        startError = describeError(err);
      }
    })();
  });

  // --- Projets et sessions (J11) -------------------------------------------------

  /** Nouvelle session dans `path`. */
  async function startNew(path: string): Promise<void> {
    phase = "start";
    cwd = path;
    historyCut = null;
    const s = store;
    try {
      const started = await startSession(path, s.push);
      // Un `exited` arrivé pendant le démarrage ne doit pas être écrasé.
      if (s === store && s.status !== "Exited") session = started.session;
    } catch (err) {
      startError = describeError(err);
    }
  }

  /**
   * Reprise de la session `id` du projet choisi : magasin neuf, puis
   * `stop_session`, `start_session --resume`, `load_history`
   * (`resumeSession`). Un historique illisible donne le bandeau
   * `history_unavailable`, et la session continue.
   */
  async function resume(path: string, id: string): Promise<void> {
    phase = "start";
    cwd = path;
    const s = newStore();
    store = s;
    draft = "";
    agentsOpen = [];
    const current = session;
    session = null;
    historyCut = null;
    try {
      const { started, history } = await resumeSession({ startSession, stopSession }, { current, cwd: path, sessionId: id, store: s });
      if (s !== store) return;
      historyCut = historyCutText(history);
      if (s.status !== "Exited") session = started.session;
    } catch (err) {
      startError = describeError(err);
    }
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<main>
  {#each store.banners as b (b.id)}
    <div class="banner {b.level.toLowerCase()}">{b.message}</div>
  {/each}
  {#if startError}
    <div class="banner error">{startError}</div>
  {/if}
  {#if launchError}
    <div class="banner" data-testid="launch-error">{launchError}</div>
  {/if}
  <div class="split">
    <div class="left">
      {#if phase === "projects"}
        <ProjectPicker
          onpick={(path) => {
            projectPath = path;
            phase = "sessions";
          }}
        />
      {:else if phase === "sessions" && projectPath}
        {@const path = projectPath}
        <SessionPicker
          cwd={path}
          onnew={() => void startNew(path)}
          onpick={(id) => void resume(path, id)}
          oncancel={() => (phase = "projects")}
        />
      {:else}
        {#if historyCut}
          <div class="history-cut term" data-testid="history-cut">{`  ⎿  ${historyCut}`}</div>
        {/if}
        <Conversation {store} {verbose} bind:this={conversation} />
        <!-- Vue détaillée (Ctrl+O) : la barre « Showing detailed transcript » de la
             conversation remplace la saisie et la ligne d'état, comme le TUI (03b). -->
        {#if store.question}
          <!-- La carte prend la place de la saisie et de la ligne d'état (`07`). -->
          <QuestionCard
            bind:this={card}
            question={store.question}
            onanswer={answer}
            onskip={skipQuestion}
            onchat={chatAboutQuestion}
          />
        {:else if !verbose}
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
            palette={paletteOpen ? { entries: paletteItems, sel: paletteSel, nameWidth: paletteNameWidth } : null}
            onpalettemove={movePalette}
            onpalettechoose={choosePalette}
          />
          <StatusLine {store} />
        {/if}
      {/if}
    </div>
    <AgentColumn {store} width="48ch" bind:open={agentsOpen} />
  </div>
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
  .split {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
  }
  .left {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .history-cut {
    flex: none;
    color: var(--t-dim);
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
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
