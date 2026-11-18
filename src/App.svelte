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
  // J14 (décisions 15 à 19) : au montage, rattachement aux sessions vivantes
  // du cœur (`attach_sessions`, rejeu de l'historique et du journal) avant
  // tout démarrage, puis signal « prêt » (`ui_ready`) ; saisie, historique ↑↓,
  // agents dépliés et Ctrl+O gardés dans `sessionStorage` à travers un
  // rechargement ; `/ameliorer <demande>` et `/revenir-version` (entrées
  // locales de la palette) ; bandeau d'avancement (`ImproveBanner`) ; hash de
  // l'interface dans la ligne d'état.
  import { onMount, tick } from "svelte";
  import AgentColumn from "./lib/components/agents/AgentColumn.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ImproveBanner from "./lib/components/ImproveBanner.svelte";
  import { entriesFrom, filterEntries, nameColumn } from "./lib/components/Palette.svelte";
  import ProjectPicker from "./lib/components/ProjectPicker.svelte";
  import Prompt from "./lib/components/Prompt.svelte";
  import QuestionCard from "./lib/components/QuestionCard.svelte";
  import SessionPicker from "./lib/components/SessionPicker.svelte";
  import StatusLine from "./lib/components/StatusLine.svelte";
  import UiNotice from "./lib/components/UiNotice.svelte";
  import VersionNotice from "./lib/components/VersionNotice.svelte";
  import TitleBar from "./lib/components/TitleBar.svelte";
  import ThemeToggle from "./lib/components/ThemeToggle.svelte";
  // Applique le thème mémorisé dès le démarrage.
  import "./lib/state/theme.svelte";
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
    tauriSelfUpdate,
    type Answer,
    type ImproveStatus,
    type SelfUpdateApi,
    type SessionResync,
    type UiInfo,
    type CoreEvent,
    type SessionKey,
    type SkipReason,
  } from "./lib/core/client";
  import type { ModelInfo } from "./lib/core/bindings/ModelInfo";
  import type { MockCore, MockSelfUpdate } from "./lib/core/mock";
  import { Keys, keyInput, type KeyAction, type Overlay } from "./lib/state/keys.svelte";
  import { SessionStore } from "./lib/state/session.svelte";
  import {
    SessionRouter,
    loadUiSnapshot,
    mainOf,
    resumeSession,
    saveUiSnapshot,
    type UiSnapshot,
  } from "./lib/state/sessions.svelte";
  import { VERSION_NOTICE_CODE, noticeVersion } from "./lib/state/versionNotice";

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
      // Lot d'un ancien magasin (session remplacée) : ses noms ne vont pas
      // dans la palette de la session affichée.
      if (s === store) noteInit(batch);
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
   * Dernier démarrage en échec (`start_session`, ou reprise) : l'écran
   * propose de réessayer ou de revenir aux sélecteurs.
   */
  let startFailed: { path: string; resumeId: string | null } | null = $state(null);
  let mock: MockCore | null = null;
  let conversation: Conversation | undefined = $state();
  let prompt: Prompt | undefined = $state();
  let card: QuestionCard | undefined = $state();
  /** Vue détaillée (Ctrl+O, jalon J5) : un appel par ligne, sorties complètes. */
  let verbose = $state(false);
  /** Agents dépliés dans la colonne (fil de l'agent, bonus de J9). */
  let agentsOpen: string[] = $state([]);
  /** Agent dont la conversation remplace la liste de la colonne (maquette J15). */
  let agentDetail: string | null = $state(null);
  // Panneau des agents terminés : replié au départ, gardé tant que l'app tourne.
  let agentsDoneOpen = $state(false);
  let agentsWaitingOpen = $state(true);
  let agentsRunningOpen = $state(true);

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
  /** Entrées locales de l'auto-amélioration (J14), traitées par l'app, jamais envoyées au CLI. */
  const IMPROVE_NAME = "ameliorer";
  const ROLLBACK_NAME = "revenir-version";
  const paletteEntries = $derived.by(() => {
    const base = entriesFrom(
      store.meta.commands.some((c) => c.name === "model") ? store.meta.commands : [...store.meta.commands, MODEL_ENTRY],
      initNames,
    ).filter((e) => e.name !== IMPROVE_NAME && e.name !== ROLLBACK_NAME);
    return [
      ...base,
      {
        name: IMPROVE_NAME,
        description: "Améliorer l'app : branche, tests, bascule de l'interface",
        argumentHint: "<demande>",
        disabled: improveDisabled,
      },
      {
        name: ROLLBACK_NAME,
        description: "Revenir à la version précédente de l'interface",
        argumentHint: null,
        disabled: rollbackDisabled,
      },
    ];
  });
  const paletteNameWidth = $derived(nameColumn(paletteEntries));
  /** Palette ouverte par Ctrl+K, avec le texte déjà tapé à ce moment. */
  let ctrlK = $state<{ base: string } | null>(null);
  /** Saisie `/…` sur laquelle Échap a fermé la palette (elle reste fermée tant que le texte ne change pas). */
  let slashDismissed: string | null = $state(null);
  let paletteSel = $state(0);

  /**
   * Texte déjà tapé avant Ctrl+K et encore présent en tête de la saisie. Si
   * l'utilisateur l'a effacé (ou modifié), il n'y a plus de `base` : toute la
   * saisie devient le filtre, et rien ne sera recollé après la commande.
   */
  const ctrlKBase = $derived(ctrlK && draft.startsWith(ctrlK.base) ? ctrlK.base : "");

  /** Filtre en cours, ou `null` : palette fermée. */
  const paletteQuery = $derived.by((): string | null => {
    if (store.question || verbose) return null;
    if (ctrlK) return draft.slice(ctrlKBase.length).replace(/^\//, "");
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
    // Entrée locale indisponible : sa raison, rien n'est inséré ni envoyé.
    if (e.disabled) {
      ctrlK = null;
      slashDismissed = draft;
      showNotice(`/${e.name} indisponible : ${e.disabled}`);
      return;
    }
    const tail = ctrlKBase;
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
      if (store.questions.some((x) => x.requestId === q.requestId)) store.putQuestion({ ...q });
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
   * « Chat about this ». Le TUI rend la main à la saisie pour en parler :
   * `skip_question` avec la raison `replied_in_chat` (`sdk.d.ts`, capacité
   * `ask_user_question_unanswered_v1`), puis focus dans la saisie. Sans la
   * capacité, l'entrée est grisée et inactive (aucune requête que le cœur
   * refuserait).
   */
  function chatAboutQuestion(): void {
    if (!store.question?.canSkip) return;
    void replyQuestion("skip", (id) => backend.skip(id, "replied_in_chat"));
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
    // aucun raccourci (palette, vue détaillée, modèle) ne s'applique ;
    // `decide` les rend sans effet (`none`), le navigateur ne les reçoit pas.
    if (selectorOpen) {
      if (action.type === "none") e.preventDefault();
      return;
    }
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

  /** Messages envoyés depuis cette saisie (historique ↑↓), vidés à chaque changement de magasin. */
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
    // J14 : entrées locales de l'auto-amélioration (jamais envoyées au CLI).
    const improve = /^\/ameliorer(?:\s+([\s\S]*))?$/.exec(trimmed);
    if (improve) {
      void startImprove((improve[1] ?? "").trim());
      return;
    }
    if (/^\/revenir-version\s*$/.test(trimmed)) {
      draft = "";
      void rollbackUi();
      return;
    }
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
    saveUi();
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
  function mockBackend(m: MockCore, firstSeq = 0): { backend: Backend; filter: (b: CoreEvent[]) => void } {
    let seq = firstSeq;
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

  /**
   * Mode simulé. Sans `attached`, nouvelle session `MockCore` ; avec, la
   * session rattachée (journal déjà rejoué dans `store`) continue : rien
   * n'est réémis et la numérotation reprend après son dernier `seq`.
   */
  async function startMock(attached: SessionResync | null = null) {
    const { MockCore, mockOptionsFromUrl } = await import("./lib/core/mock");
    const opts = mockOptionsFromUrl(location.search) ?? { name: "demo", rows: 60 };
    mock = new MockCore(attached ? { ...opts, rows: 0 } : opts);
    cwd = attached ? attached.cwd : `simulé : ${opts.name}`;
    const mb = mockBackend(mock, attached ? store.lastSeq + 1 : 0);
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
    if (attached) mock.attach(mb.filter);
    else mock.start(mb.filter);
    mockReady = true;
    // Points d'accès des mesures e2e (mode simulé seulement).
    (window as unknown as Record<string, unknown>).__orch = {
      store,
      mock,
      conversation: () => conversation,
      latencies,
      calls: mockCalls,
      keys,
      selfUpdate: () => mockSelfUpdate,
      router: () => router,
      uiInfo: () => uiInfo,
      saveUi,
    };
  }

  // --- Rattachement, signal « prêt » et auto-amélioration (J14) ----------------

  /** Commandes d'auto-amélioration, réelles ou simulées (choisies au montage). */
  let selfUpdate: SelfUpdateApi | null = $state.raw(null);
  let mockSelfUpdate: MockSelfUpdate | null = null;
  /** Sessions rattachées au montage, et celles apparues ensuite sur le même canal. */
  let router: SessionRouter | null = null;
  /** Réponse de `get_ui_info` (`null` : pas encore lue, ou commande en échec). */
  let uiInfo: UiInfo | null = $state(null);
  let uiInfoError: string | null = $state(null);
  /** Bandeau d'amélioration fermé (id). */
  let improveDismissed: string | null = $state(null);
  /** Dernier état reçu d'une amélioration. */
  let improveLive: ImproveStatus | null = $state(null);
  /** Hash et empreinte du contrat de cette interface (`define` de `vite.config.ts`). */
  const UI_HASH = typeof __UI_HASH__ === "string" ? __UI_HASH__ : "dev";
  const CONTRACT = typeof __CONTRACT__ === "string" ? __CONTRACT__ : null;
  const FINAL_STEPS = ["merged", "restart_required", "failed", "cancelled", "rolled_back"];

  const improveRunning = $derived.by(() => {
    const st = improveLive ?? uiInfo?.improve ?? null;
    return st !== null && !FINAL_STEPS.includes(st.step);
  });

  /** Raison pour laquelle `/ameliorer` est grisé, ou `null`. */
  const improveDisabled = $derived.by((): string | null => {
    if (!selfUpdate) return "interface en cours de démarrage";
    if (!uiInfo) return uiInfoError ? `état inconnu (${uiInfoError})` : "état du clone en cours de lecture";
    const r = uiInfo.repo;
    if (r.state === "disabled") return r.reason;
    if (r.state === "cloning") return `clonage du dépôt en cours (${r.path})`;
    if (improveRunning) return "une amélioration est déjà en cours";
    return null;
  });

  /** Raison pour laquelle `/revenir-version` est grisé, ou `null`. */
  const rollbackDisabled = $derived.by((): string | null => {
    if (!selfUpdate) return "interface en cours de démarrage";
    if (!uiInfo) return uiInfoError ? `état inconnu (${uiInfoError})` : "état de l'interface en cours de lecture";
    // Pendant un cycle (bascule comprise), le retour arrière croiserait la bascule.
    if (improveRunning) return "une amélioration est en cours";
    if (uiInfo.source === "embedded" && !uiInfo.previous_hash) return "interface embarquée, aucune version précédente";
    return null;
  });

  async function refreshUiInfo(): Promise<void> {
    const su = selfUpdate;
    if (!su) return;
    try {
      const info = await su.getUiInfo();
      uiInfo = info ?? null;
      uiInfoError = info ? null : "réponse vide";
    } catch (err) {
      uiInfoError = describeError(err);
    }
  }

  // Clone de l'app en préparation au montage (premier lancement : clonage en
  // tâche de fond côté cœur) : état relu toutes les 2 s jusqu'à son issue,
  // pour dégriser `/ameliorer` sans recharger.
  $effect(() => {
    const state = uiInfo?.repo.state;
    if (state !== "cloning" && state !== "unknown") return;
    const timer = setInterval(() => void refreshUiInfo(), 2000);
    return () => clearInterval(timer);
  });

  async function startImprove(request: string): Promise<void> {
    if (!request) {
      draft = "/ameliorer ";
      showNotice("Usage : /ameliorer <demande>");
      prompt?.focusEnd();
      return;
    }
    const su = selfUpdate;
    if (improveDisabled || !su) {
      showNotice(`/ameliorer indisponible : ${improveDisabled ?? "interface en cours de démarrage"}`);
      return;
    }
    draft = "";
    try {
      const st = await su.improveStart(request);
      improveLive = st;
      improveDismissed = null;
      if (uiInfo) uiInfo = { ...uiInfo, improve: st };
    } catch (err) {
      store.addBanner("Error", "improve_start", `/ameliorer : ${describeError(err)}`);
      void refreshUiInfo();
    }
  }

  async function rollbackUi(): Promise<void> {
    const su = selfUpdate;
    if (rollbackDisabled || !su) {
      showNotice(`/revenir-version indisponible : ${rollbackDisabled ?? "interface en cours de démarrage"}`);
      return;
    }
    try {
      const info = await su.uiRollback();
      uiInfo = info;
      showNotice(info?.active_hash ? `Interface ${info.active_hash.slice(0, 7)} réactivée` : "Interface embarquée réactivée");
    } catch (err) {
      store.addBanner("Error", "ui_rollback", `Retour arrière : ${describeError(err)}`);
    }
  }

  function onImproveStatus(st: ImproveStatus): void {
    improveLive = st;
    if (FINAL_STEPS.includes(st.step)) void refreshUiInfo();
  }

  /** Abonnement du bandeau à l'avancement (`IMPROVE_EVENT`, ou le cœur simulé). */
  function subscribeImprove(cb: (st: ImproveStatus) => void): Promise<() => void> {
    const su = selfUpdate;
    return su ? su.onImproveStatus(cb) : Promise.resolve(() => {});
  }

  /** Clé de la session principale affichée (mode simulé : `mock-1`). */
  function mainKey(): SessionKey | null {
    return mockMode ? (mockReady ? "mock-1" : null) : session;
  }

  /** État de l'interface à garder à travers un rechargement. */
  function snapshot(): UiSnapshot {
    return {
      session: mainKey(),
      draft,
      sent: sent.slice(-500),
      agentsOpen: [...agentsOpen],
      agentsDoneOpen,
      agentsWaitingOpen,
      agentsRunningOpen,
      verbose,
      improveDismissed,
    };
  }

  function saveUi(): void {
    try {
      saveUiSnapshot(snapshot());
    } catch {
      // Jamais bloquant.
    }
  }

  /** Rétablit l'état gardé s'il appartient à la session affichée. */
  function restoreUi(saved: UiSnapshot | null): void {
    if (!saved) return;
    try {
      improveDismissed = saved.improveDismissed;
      if (saved.session === null || saved.session !== mainKey()) return;
      draft = saved.draft;
      sent.length = 0;
      sent.push(...saved.sent);
      agentsOpen = saved.agentsOpen;
      agentsDoneOpen = saved.agentsDoneOpen;
      agentsWaitingOpen = saved.agentsWaitingOpen;
      agentsRunningOpen = saved.agentsRunningOpen;
      verbose = saved.verbose;
    } catch {
      // État illisible : on garde l'état neuf.
    }
  }

  // Sauvegarde après chaque changement, temporisée (une frappe n'écrit pas tout
  // de suite : 300 ms sans changement), une fois le montage fini ; `pagehide`
  // écrit tout de suite ce qui reste (le rechargement de la bascule ne lui
  // laisse pas toujours le temps, d'où l'écriture au fil de l'eau).
  const SAVE_DELAY_MS = 300;
  let booted = $state(false);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  function scheduleSave(): void {
    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      saveUi();
    }, SAVE_DELAY_MS);
  }
  $effect(() => {
    void [draft, agentsOpen, agentsDoneOpen, agentsWaitingOpen, agentsRunningOpen, verbose, improveDismissed, session, mockReady];
    if (booted) scheduleSave();
  });

  /**
   * Magasin d'une session rattachée : la principale a les crochets de l'app
   * (palette, latence). La session d'amélioration n'est jamais affichée (le
   * bandeau suit `ImproveStatus`) : pas de magasin, le routeur n'en garde que
   * les derniers événements.
   */
  function routedStore(_key: SessionKey, role: "main" | "improve"): SessionStore | null {
    return role === "main" ? newStore() : null;
  }

  /** La session principale rattachée devient la session affichée (aucune session relancée). */
  async function adoptMain(r: SessionRouter, main: SessionResync): Promise<void> {
    const s = r.get(main.session);
    if (!s) return;
    phase = "start";
    cwd = main.cwd;
    store = s;
    noteInit(main.history?.events ?? []);
    noteInit(main.events);
    if (mockMode) await startMock(main);
    else if (main.running) session = main.session;
    else startFailed = { path: main.cwd, resumeId: main.resumed_from };
  }

  /** Démarrage sans session rattachée (comportement d'avant J14). */
  async function startFresh(): Promise<void> {
    if (mockMode) {
      await startMock();
      return;
    }
    const info = await getLaunchInfo();
    launchError = info.launch_error;
    if (info.cwd_from_args) await startNew(info.cwd);
    else phase = "projects";
  }

  /** Échec du montage (rattachement ou rejeu) : aucune session lancée, bouton « Réessayer ». */
  let mountError: string | null = $state(null);
  let mountRetrying = $state(false);
  const ATTACH_TRIES = 3;
  const ATTACH_RETRY_MS = 300;

  /**
   * `attach_sessions`, essayé 3 fois à 300 ms d'intervalle. Le même routeur
   * sert à chaque essai : un canal resté enregistré après un échec livre au
   * même endroit, et les doublons sont écartés par `seq`.
   */
  async function attachWithRetry(su: SelfUpdateApi, r: SessionRouter): Promise<SessionResync[]> {
    let last: unknown = null;
    for (let i = 0; i < ATTACH_TRIES; i++) {
      if (i > 0) await new Promise((res) => setTimeout(res, ATTACH_RETRY_MS));
      try {
        return (await su.attachSessions(r.onBatch)) ?? [];
      } catch (err) {
        last = err;
        console.warn(`attach_sessions (essai ${i + 1}/${ATTACH_TRIES})`, describeError(err));
      }
    }
    throw last;
  }

  /** Une image peinte après le rendu (`ui_ready` ne part qu'ensuite) ; repli à 1 s si rAF est suspendu (fenêtre cachée). */
  function painted(): Promise<void> {
    return new Promise((res) => {
      const t = setTimeout(res, 1000);
      requestAnimationFrame(() => {
        clearTimeout(t);
        res();
      });
    });
  }

  /**
   * Montage : rattachement, rejeu, puis `ui_ready`, puis démarrage habituel
   * s'il n'y avait rien à reprendre. Relancé tel quel par « Réessayer ».
   */
  async function boot(su: SelfUpdateApi, saved: UiSnapshot | null): Promise<void> {
    mountError = null;
    router?.dispose();
    const r = new SessionRouter(routedStore);
    router = r;
    // 1. Rattachement, AVANT tout démarrage : une session vivante n'est jamais
    //    relancée. En échec, AUCUNE session n'est lancée (le cœur n'a pas de
    //    garde : `start_session` en lancerait une deuxième à côté d'une vivante).
    let live: SessionResync[];
    try {
      live = await attachWithRetry(su, r);
    } catch (err) {
      r.dispose();
      mountError = `Rattachement aux sessions impossible : ${describeError(err)}. Aucune session n'a été lancée.`;
      return;
    }
    let main: SessionResync | null;
    try {
      r.resync(live);
      main = mainOf(live);
      if (main) {
        await adoptMain(r, main);
        restoreUi(saved);
      }
    } catch (err) {
      r.dispose();
      mountError = `Reprise des sessions impossible : ${describeError(err)}. Aucune session n'a été lancée.`;
      return;
    }
    // 2. État de l'interface, puis signal « prêt » au watchdog du cœur, une
    //    fois le rattachement répondu, le rejeu fait et une image peinte.
    //    Un échec de `get_ui_info` seul n'empêche pas `ui_ready` : l'interface
    //    reste utilisable (ligne d'état et palette indiquent l'état inconnu) ;
    //    le hash envoyé est alors celui de cette page.
    await refreshUiInfo();
    await tick();
    await painted();
    try {
      await su.uiReady(uiInfo?.source === "embedded" ? null : UI_HASH);
    } catch (err) {
      console.warn("ui_ready", describeError(err));
    }
    // 3. Sans session à reprendre : démarrage habituel.
    if (!main) {
      try {
        await startFresh();
        if (mockMode) restoreUi(saved);
      } catch (err) {
        startError = mockMode ? String(err) : describeError(err);
      }
    }
    booted = true;
    saveUi();
  }

  async function retryMount(): Promise<void> {
    const su = selfUpdate;
    if (!su || mountRetrying) return;
    mountRetrying = true;
    try {
      await boot(su, loadUiSnapshot());
    } finally {
      mountRetrying = false;
    }
  }

  onMount(() => {
    const saved = loadUiSnapshot();
    // Rien n'est écrit avant la fin du montage : un état à moitié rétabli
    // écraserait celui gardé par la page d'avant.
    const onHide = () => {
      if (!booted) return;
      if (saveTimer !== null) clearTimeout(saveTimer);
      saveTimer = null;
      saveUi();
    };
    window.addEventListener("pagehide", onHide);
    window.addEventListener("beforeunload", onHide);
    (async () => {
      let su: SelfUpdateApi = tauriSelfUpdate;
      if (mockMode) {
        const m = await import("./lib/core/mock");
        const o = m.mockSelfUpdateFromUrl(location.search);
        const ms = new m.MockSelfUpdate(() => {}, o.stepMs ?? 300, o);
        ms.info.build_commit = UI_HASH;
        if (CONTRACT) ms.info.contract = CONTRACT;
        mockSelfUpdate = ms;
        su = ms;
        // Accès e2e avant tout démarrage (échec du rattachement).
        (window as unknown as Record<string, unknown>).__orchSelfUpdate = ms;
      }
      selfUpdate = su;
      await boot(su, saved);
    })();
    return () => {
      window.removeEventListener("pagehide", onHide);
      window.removeEventListener("beforeunload", onHide);
      if (saveTimer !== null) clearTimeout(saveTimer);
    };
  });

  // --- Projets et sessions (J11) -------------------------------------------------

  /**
   * Magasin neuf pour une autre session : tout ce qui appartenait à la
   * précédente est oublié (saisie, historique ↑↓ des envois, noms de `init`
   * pour la palette, palette et sélecteur de modèle, agents dépliés, erreur
   * de démarrage).
   */
  function switchStore(path: string): SessionStore {
    phase = "start";
    cwd = path;
    const s = newStore();
    store = s;
    draft = "";
    sent.length = 0;
    initNames = [];
    ctrlK = null;
    slashDismissed = null;
    pickerOpen = false;
    agentsOpen = [];
    agentDetail = null;
    startError = null;
    startFailed = null;
    return s;
  }

  /** Nouvelle session dans `path`, sur un magasin neuf. */
  async function startNew(path: string): Promise<void> {
    const s = switchStore(path);
    try {
      const started = await startSession(path, s.push);
      // Un `exited` arrivé pendant le démarrage ne doit pas être écrasé.
      if (s === store && s.status !== "Exited") session = started.session;
    } catch (err) {
      if (s !== store) return;
      startError = describeError(err);
      startFailed = { path, resumeId: null };
    }
  }

  /**
   * Reprise de la session `id` du projet choisi : magasin neuf, puis
   * `stop_session`, `start_session --resume`, `load_history`
   * (`resumeSession`). Un historique illisible donne le bandeau
   * `history_unavailable`, et la session continue ; un historique tronqué,
   * la ligne « Début de l'historique omis » en tête du fil.
   */
  async function resume(path: string, id: string): Promise<void> {
    const s = switchStore(path);
    const current = session;
    session = null;
    try {
      const { started } = await resumeSession({ startSession, stopSession }, { current, cwd: path, sessionId: id, store: s });
      if (s !== store) return;
      if (s.status !== "Exited") session = started.session;
    } catch (err) {
      if (s !== store) return;
      startError = describeError(err);
      startFailed = { path, resumeId: id };
    }
  }

  /** Après un échec de démarrage : même demande, magasin neuf. */
  function retryStart(): void {
    const f = startFailed;
    if (!f) return;
    if (f.resumeId) void resume(f.path, f.resumeId);
    else void startNew(f.path);
  }

  /** Après un échec de démarrage : retour au sélecteur des sessions du projet. */
  function backToSelectors(): void {
    const f = startFailed;
    startFailed = null;
    startError = null;
    projectPath = f?.path ?? projectPath;
    phase = projectPath ? "sessions" : "projects";
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<main>
  <TitleBar version={store.meta.version} />
  {#each store.banners as b (b.id)}
    {#if b.code === VERSION_NOTICE_CODE}
      <VersionNotice
        message={b.message}
        version={noticeVersion(b.detail) ?? store.meta.version}
        onclose={() => prompt?.focus()}
      />
    {:else}
      <div class="banner {b.level.toLowerCase()}">{b.message}</div>
    {/if}
  {/each}
  <UiNotice uiHash={UI_HASH} {uiInfo} contract={CONTRACT} />
  {#if startError}
    <div class="banner error">{startError}</div>
  {/if}
  {#if mountError}
    <div class="banner error" data-testid="mount-error">
      {mountError}
      <button data-testid="mount-retry" disabled={mountRetrying} onclick={() => void retryMount()}>Réessayer</button>
    </div>
  {/if}
  {#if launchError}
    <div class="banner" data-testid="launch-error">{launchError}</div>
  {/if}
  {#if selfUpdate}
    <!-- J14 : avancement de `/ameliorer` (session dédiée, tests, bascule, échec). -->
    <ImproveBanner
      initial={uiInfo?.improve ?? null}
      subscribe={subscribeImprove}
      repoPath={uiInfo && uiInfo.repo.state !== "disabled" && uiInfo.repo.state !== "unknown" ? uiInfo.repo.path : null}
      dismissed={improveDismissed}
      oncancel={(id) => selfUpdate!.improveCancel(id)}
      onrollback={() => void rollbackUi()}
      ondismiss={(id) => {
        improveDismissed = id;
        prompt?.focus();
      }}
      onstatus={onImproveStatus}
    />
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
        {#if startFailed}
          <!-- Échec du démarrage (plan 1.9) : réessayer, ou revenir aux sélecteurs. -->
          <div class="start-failed term" data-testid="start-failed">
            <button data-testid="start-retry" onclick={retryStart}>Réessayer</button>
            <button data-testid="start-back" onclick={backToSelectors}>Revenir aux sélecteurs</button>
          </div>
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
    <AgentColumn {store} width="430px" detail bind:selected={agentDetail} bind:open={agentsOpen}
      bind:doneOpen={agentsDoneOpen}
      bind:waitingOpen={agentsWaitingOpen}
      bind:runningOpen={agentsRunningOpen}
    />
  </div>
  <!-- Barre d'état (maquette J15) : segments en flèche. -->
  <footer>
    {#if UI_HASH}
      <span class="seg accent first" data-testid="ui-hash" title="Interface exécutée"
        >ui {UI_HASH.slice(0, 7)}{uiInfo ? (uiInfo.source === "disk" ? " (disque)" : " (embarquée)") : ""}</span
      >
    {/if}
    <span class="seg surface path" class:alone={!UI_HASH}>{cwd}</span>
    <!-- data-* : mesures de la recette sur l'exe (J14 : lignes, trous de seq). -->
    <span
      class="mid"
      data-testid="footer-status"
      data-rows={store.rows.length}
      data-gaps={store.gaps}
      data-last-seq={store.lastSeq}>{store.status} · {store.rows.length} lignes</span
    >
    <span class="seg surface rev" title="Latence d'affichage d'un token (médiane)">p50 {p50} ms</span>
    <!-- Le bouton Clair/Sombre est posé sur le segment p95 (maquette J15). -->
    <span class="seg accent rev last">
      <span title="Latence d'affichage d'un token (95 %)">p95 {p95} ms</span>
      <ThemeToggle />
    </span>
  </footer>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg);
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
  .start-failed {
    flex: none;
    display: flex;
    gap: 2ch;
    padding: var(--t-lh) 1ch 0;
  }
  .start-failed button {
    background: var(--t-bg);
    color: var(--t-fg);
    border: 1px solid var(--tree);
    font: inherit;
    padding: 0 1ch;
    cursor: pointer;
  }
  .start-failed button:focus-visible {
    outline: 1px solid var(--t-fg);
  }
  .banner {
    padding: 2px 1ch;
    color: var(--warn);
    background: var(--warn-bg);
    border-bottom: 1px solid var(--rule);
  }
  .banner.error {
    color: var(--danger);
    background: var(--danger-bg);
  }
  footer {
    flex: 0 0 auto;
    display: flex;
    align-items: stretch;
    height: 24px;
    font-size: 12px;
    background: var(--bar);
    white-space: nowrap;
  }
  .seg {
    display: flex;
    align-items: center;
    min-width: 0;
    overflow: hidden;
  }
  .seg.accent {
    background: var(--accent-grad);
    color: var(--on-accent);
    font-weight: 700;
  }
  .seg.surface {
    background: var(--surface);
    color: var(--fg);
  }
  .seg.first {
    flex: 0 0 auto;
    padding: 0 18px 0 12px;
    clip-path: polygon(0 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 0 100%);
  }
  .seg.path {
    flex: 0 1 auto;
    padding: 0 20px 0 16px;
    margin-left: -8px;
    text-overflow: ellipsis;
    clip-path: polygon(0 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 0 100%, 10px 50%);
  }
  .seg.rev {
    flex: 0 0 auto;
    padding: 0 12px 0 20px;
    clip-path: polygon(10px 0, 100% 0, 100% 100%, 10px 100%, 0 50%);
  }
  /* Sans hash d'interface : le dossier ouvre la barre, sans encoche à gauche. */
  .seg.path.alone {
    margin-left: 0;
    clip-path: polygon(0 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 0 100%);
  }
  .seg.last {
    gap: 10px;
    padding-right: 3px;
    margin-left: -8px;
  }
  .mid {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 1ch;
    color: var(--muted);
    overflow: hidden;
  }
</style>
