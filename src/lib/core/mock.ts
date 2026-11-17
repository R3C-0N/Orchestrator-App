// Mode simulé (plan 1.3, `npm run dev:mock`) : rejoue des `CoreEvent`
// conformes au contrat (src/lib/core/bindings) sans le cœur Rust.
//
// - `buildHistory` fabrique une session longue déterministe (graine fixe)
//   d'environ N lignes de rendu : messages, réponses markdown avec code,
//   appels d'outils avec résultat, sous-agents, fins de tour.
// - `MockCore` imite le canal du cœur : lots envoyés dès 16 ms
//   d'accumulation ou 256 événements, et tout de suite quand le flux est
//   calme (plan 1.5) ; `t` = horodatage d'émission (équivalent de la
//   lecture de la ligne par Rust, sans le coût de l'IPC).
// - Débit des `text_delta` réglable (`rate`, par seconde).
//
// En attendant l'outil `replay` (J4), aucune fixture `fixtures/ui-events/`
// n'existe : `?mock=tN` se replie sur la session synthétique.

import type { AgentEvent } from "./bindings/AgentEvent";
import type { CoreEvent } from "./bindings/CoreEvent";
import type { EventBody } from "./bindings/EventBody";
import type { HistoryLoaded } from "./bindings/HistoryLoaded";
import type { ImproveStatus } from "./bindings/ImproveStatus";
import type { ImproveStep } from "./bindings/ImproveStep";
import type { QuestionItem } from "./bindings/QuestionItem";
import type { RepoStatus } from "./bindings/RepoStatus";
import type { SessionResync } from "./bindings/SessionResync";
import type { UiInfo } from "./bindings/UiInfo";

const SESSION = "mock-1";

/** PRNG déterministe (mulberry32). */
export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS = (
  "le la les un une des du de et ou mais donc car ce cette ces qui que dont où " +
  "fichier fonction module composant session flux événement ligne bloc outil agent " +
  "réducteur état interface cœur canal lot image rendu liste texte message tour " +
  "test mesure seuil latence mémoire défilement collage streaming delta modèle " +
  "the a of to in is it for on with as this that by from read write update parse " +
  "render store event batch frame scroll virtual list row height measure value"
).split(" ");

const CODE_SNIPPETS = [
  "```ts\nexport function sum(xs: number[]): number {\n  let s = 0;\n  for (const x of xs) s += x;\n  return s;\n}\n```",
  "```rust\nfn reduce(s: &mut Session, ev: Inbound) -> Vec<CoreEvent> {\n    match ev {\n        Inbound::Stream(e) => s.on_stream(e),\n        _ => vec![],\n    }\n}\n```",
  "```bash\nnpm run check && npm run build\ncargo test --manifest-path src-tauri/Cargo.toml\n```",
  "```json\n{ \"kind\": \"text_delta\", \"thread\": \"main\", \"index\": 0, \"text\": \"…\" }\n```",
];

const TOOLS = ["Read", "Bash", "Grep", "Glob", "Edit", "Write"];
const AGENT_COLORS = ["blue", "red", "green", "yellow", "purple", "orange", "pink", "cyan"];

function pick<T>(r: () => number, xs: readonly T[]): T {
  return xs[Math.floor(r() * xs.length)];
}

function sentence(r: () => number, min = 6, max = 22): string {
  const n = min + Math.floor(r() * (max - min));
  const w: string[] = [];
  for (let i = 0; i < n; i++) w.push(pick(r, WORDS));
  const s = w.join(" ");
  return s.charAt(0).toUpperCase() + s.slice(1) + ".";
}

function paragraph(r: () => number): string {
  const n = 2 + Math.floor(r() * 5);
  const s: string[] = [];
  for (let i = 0; i < n; i++) s.push(sentence(r));
  return s.join(" ");
}

/** Réponse markdown d'environ 0,5 à 6 ko (titres, listes, `code`, blocs). */
export function assistantText(r: () => number): string {
  const parts: string[] = [];
  const blocks = 2 + Math.floor(r() * 7);
  for (let b = 0; b < blocks; b++) {
    const k = r();
    if (k < 0.15) parts.push(`## ${sentence(r, 2, 5).slice(0, -1)}`);
    else if (k < 0.3)
      parts.push(
        Array.from({ length: 2 + Math.floor(r() * 4) }, () => `- ${sentence(r, 3, 10)} \`${pick(r, WORDS)}\``).join(
          "\n",
        ),
      );
    else if (k < 0.42) parts.push(pick(r, CODE_SNIPPETS));
    else parts.push(paragraph(r));
  }
  return parts.join("\n\n");
}

function toolInput(r: () => number, name: string): Record<string, string> {
  const path = `src/lib/${pick(r, ["core", "state", "components"])}/${pick(r, WORDS)}.ts`;
  switch (name) {
    case "Bash":
      return { command: `npm run ${pick(r, ["check", "build", "test"])}`, description: sentence(r, 2, 5) };
    case "Grep":
      return { pattern: pick(r, WORDS), path: "src" };
    case "Glob":
      return { pattern: "**/*.svelte" };
    case "Edit":
      return { file_path: path, old_string: sentence(r), new_string: sentence(r) };
    case "Write":
      return { file_path: path, content: paragraph(r) };
    default:
      return { file_path: path };
  }
}

function toolOutput(r: () => number): string {
  const n = 1 + Math.floor(r() * 60);
  return Array.from({ length: n }, (_, i) => `${String(i + 1).padStart(4)}  ${sentence(r, 3, 12)}`).join("\n");
}

/** Fabrique d'événements numérotés (`seq` continu, `t` à l'émission). */
export class EventFactory {
  seq = 0;
  make(body: EventBody, t = Date.now()): CoreEvent {
    return { seq: this.seq++, t, session: SESSION, ...body } as CoreEvent;
  }
}

export type HistoryStats = { rows: number; events: number; textBytes: number; tools: number; agents: number };

/**
 * Session longue synthétique : produit des tours jusqu'à atteindre `targetRows`
 * lignes du fil principal (comptées comme le magasin les crée).
 */
export function buildHistory(
  f: EventFactory,
  targetRows: number,
  seed = 15,
): { events: CoreEvent[]; stats: HistoryStats } {
  const r = rng(seed);
  const ev: CoreEvent[] = [];
  const stats: HistoryStats = { rows: 0, events: 0, textBytes: 0, tools: 0, agents: 0 };
  let turn = 0;
  const T0 = Date.UTC(2026, 10, 16, 8, 0, 0);
  while (stats.rows < targetRows) {
    const t = T0 + turn * 60_000;
    const msg = `msg_mock_${turn}`;
    const user = sentence(r, 4, 30);
    ev.push(f.make({ kind: "user_message", thread: "main", uuid: null, text: user, origin: "History" }, t));
    stats.rows++;
    stats.textBytes += user.length;
    // Plus de place pour une fin de tour : le dernier message reste seul.
    if (stats.rows >= targetRows) break;
    let index = 0;
    const steps = 1 + Math.floor(r() * 4);
    for (let s = 0; s < steps && stats.rows < targetRows - 1; s++) {
      const k = r();
      if (k < 0.5) {
        const text = assistantText(r);
        ev.push(f.make({ kind: "block_start", thread: "main", msg, index, block: { type: "Text" } }, t));
        ev.push(f.make({ kind: "block_done", thread: "main", msg, index, block: { type: "Text", text } }, t));
        stats.textBytes += text.length;
      } else if (k < 0.93) {
        const name = pick(r, TOOLS);
        const id = `toolu_mock_${turn}_${index}`;
        const input = toolInput(r, name);
        const out = toolOutput(r);
        const isError = r() < 0.05;
        ev.push(
          f.make({ kind: "block_start", thread: "main", msg, index, block: { type: "ToolUse", tool_use_id: id, name } }, t),
        );
        ev.push(
          f.make(
            { kind: "block_done", thread: "main", msg, index, block: { type: "ToolUse", tool_use_id: id, name, input } },
            t,
          ),
        );
        ev.push(
          f.make(
            {
              kind: "tool_result",
              thread: "main",
              tool_use_id: id,
              content: [{ type: "Text", text: out }],
              is_error: isError,
              meta: null,
            },
            t,
          ),
        );
        stats.tools++;
        stats.textBytes += out.length;
      } else {
        const id = `toolu_agent_${turn}_${index}`;
        const agent: AgentEvent = {
          tool_use_id: id,
          task_id: `task_${turn}_${index}`,
          agent_type: pick(r, ["general-purpose", "Explore", "Plan"]),
          color: pick(r, AGENT_COLORS),
          description: sentence(r, 2, 6).slice(0, -1),
          status: "Starting",
          background: true,
          started_at: t,
          ended_at: null,
          last_tool: null,
          tokens: null,
          tool_uses: null,
          summary: null,
        };
        ev.push(
          f.make(
            { kind: "block_start", thread: "main", msg, index, block: { type: "ToolUse", tool_use_id: id, name: "Agent" } },
            t,
          ),
        );
        ev.push(f.make({ kind: "agent", ...agent }, t));
        // Un message du sous-agent : il va dans `threads`, pas dans le fil.
        ev.push(
          f.make({ kind: "block_start", thread: id, msg: `${msg}_sub`, index: 0, block: { type: "Text" } }, t),
        );
        ev.push(
          f.make(
            { kind: "block_done", thread: id, msg: `${msg}_sub`, index: 0, block: { type: "Text", text: paragraph(r) } },
            t,
          ),
        );
        ev.push(
          f.make(
            {
              kind: "agent",
              ...agent,
              status: "Completed",
              ended_at: t + 42_000,
              tokens: 1000 + Math.floor(r() * 20000),
              tool_uses: Math.floor(r() * 20),
            },
            t,
          ),
        );
        stats.agents++;
      }
      stats.rows++;
      index++;
    }
    const d = 2000 + Math.floor(r() * 90_000);
    ev.push(
      f.make(
        {
          kind: "turn_result",
          subtype: "success",
          is_error: false,
          terminal_reason: null,
          duration_ms: d,
          cost_usd: 0.01,
          num_turns: index,
          context_used: null,
          context_window: null,
          permission_denials: [],
          result: null,
        },
        t + d,
      ),
    );
    stats.rows++;
    turn++;
  }
  stats.events = ev.length;
  return { events: ev, stats };
}

export type MockOptions = {
  /** Lignes d'historique chargées au démarrage. */
  rows?: number;
  /** `text_delta` par seconde pendant le streaming. */
  rate?: number;
  seed?: number;
};

/** Faux cœur : même forme d'API que `client.ts` côté événements. */
export class MockCore {
  readonly factory = new EventFactory();
  readonly opts: Required<MockOptions>;
  stats: HistoryStats | null = null;

  #onEvents: ((batch: CoreEvent[]) => void) | null = null;
  #pending: CoreEvent[] = [];
  #flushTimer: ReturnType<typeof setTimeout> | null = null;
  #lastSent = 0;
  /**
   * Instant du dernier événement émis, quel qu'il soit : mis à jour à chaque
   * `#emit`, comme `last_event` du cœur l'est pour chaque événement reçu, y
   * compris ceux vidés avec le bord avant (`state/batcher.rs`).
   */
  #lastEvent = -Infinity;
  #streamTimer: ReturnType<typeof setTimeout> | null = null;
  #turn = 0;
  #r: () => number;
  /** Bloc de texte ouvert par le flux en cours. */
  #open: { msg: string; text: string } | null = null;
  /** Capacités annoncées par `init` (`can_skip` des questions). */
  #caps: string[] = [];
  /** Questions en attente de réponse (comme `pending_in` du cœur). */
  #questions = new Map<string, { toolUseId: string; thread: string; questions: QuestionItem[]; canSkip: boolean }>();
  #asked = 0;

  constructor(opts: MockOptions = {}) {
    this.opts = { rows: opts.rows ?? 3000, rate: opts.rate ?? 100, seed: opts.seed ?? 15 };
    this.#r = rng(this.opts.seed + 1);
  }

  /** Démarre : `starting`, `ready`, `init`, puis l'historique par lots de 256. */
  start(onEvents: (batch: CoreEvent[]) => void): void {
    this.#onEvents = onEvents;
    const f = this.factory;
    const boot: CoreEvent[] = [
      f.make({ kind: "starting", pid: 4242, cwd: "C:\\projet", args: [] }),
      f.make({
        kind: "ready",
        version: "2.1.325",
        capabilities: [],
        commands: [{ name: "review", description: "Relire le diff", argument_hint: null }],
        agents: [],
        models: [],
        permission_mode: "bypassPermissions",
      } as EventBody),
      f.make({
        kind: "init",
        session_id: "11111111-2222-4333-8444-555555555555",
        model: "claude-opus-5-5",
        permission_mode: "bypassPermissions",
        capabilities: ["ask_user_question_unanswered_v1"],
        tools: [],
        mcp_servers: [],
        skills: [],
        slash_commands: [],
        version: "2.1.325",
      }),
    ];
    onEvents(boot);
    const init = boot[2];
    if (init.kind === "init") this.#caps = init.capabilities;
    if (this.opts.rows > 0) {
      const { events, stats } = buildHistory(f, this.opts.rows, this.opts.seed);
      this.stats = stats;
      for (let i = 0; i < events.length; i += 256) onEvents(events.slice(i, i + 256));
    }
    onEvents([f.make({ kind: "status", state: "Idle" })]);
  }

  /**
   * Rattachement (J14) : reprend une session déjà en cours (son journal est
   * rejoué par l'interface) sans rien réémettre ; la suite va à `onEvents`.
   */
  attach(onEvents: (batch: CoreEvent[]) => void): void {
    this.#onEvents = onEvents;
    this.#caps = ["ask_user_question_unanswered_v1"];
  }

  /** Comme `send_message` : écho `user_message`, puis une réponse en streaming. */
  send(text: string, replyChars = 1200): void {
    this.stopStream();
    this.#emit({ kind: "user_message", thread: "main", uuid: null, text, origin: "Typed" });
    this.streamReply(replyChars);
  }

  /**
   * Réponse en streaming au débit `rate` (deltas/s) ; `chars = Infinity`
   * donne un flux continu jusqu'à `stopStream()`.
   */
  streamReply(chars = 1200): void {
    this.stopStream();
    const r = this.#r;
    const msg = `msg_live_${this.#turn++}`;
    let full = "";
    while (full.length < Math.min(chars, 200_000)) full += (full ? "\n\n" : "") + assistantText(r);
    this.#emit({ kind: "status", state: "Requesting" });
    this.#emit({ kind: "status", state: "Streaming" });
    this.#emit({ kind: "block_start", thread: "main", msg, index: 0, block: { type: "Text" } });
    let pos = 0;
    const open = { msg, text: "" };
    this.#open = open;
    const start = performance.now();
    let emitted = 0;
    const period = 1000 / this.opts.rate;
    const tick = () => {
      const due = Math.floor((performance.now() - start) / period) + 1;
      while (emitted < due) {
        if (pos >= full.length && Number.isFinite(chars)) {
          this.#finishReply(msg, open.text);
          return;
        }
        if (pos >= full.length) {
          full += "\n\n" + assistantText(r);
        }
        const n = 2 + Math.floor(r() * 10);
        const piece = full.slice(pos, pos + n);
        pos += n;
        open.text += piece;
        this.#emit({ kind: "text_delta", thread: "main", msg, index: 0, text: piece });
        emitted++;
      }
      this.#streamTimer = setTimeout(tick, Math.max(1, Math.min(period, 8)));
    };
    tick();
  }

  /** Arrête le flux en cours ; le bloc ouvert est clos avec le texte déjà envoyé. */
  stopStream(): void {
    if (this.#streamTimer) clearTimeout(this.#streamTimer);
    this.#streamTimer = null;
    const open = this.#open;
    if (open) this.#finishReply(open.msg, open.text);
  }

  /** Vrai pendant un flux. */
  get streaming(): boolean {
    return this.#open !== null;
  }

  #finishReply(msg: string, text: string): void {
    this.#streamTimer = null;
    this.#open = null;
    this.#emit({ kind: "block_done", thread: "main", msg, index: 0, block: { type: "Text", text } });
    this.#emit({
      kind: "turn_result",
      subtype: "success",
      is_error: false,
      terminal_reason: null,
      duration_ms: 3000,
      cost_usd: 0.01,
      num_turns: 1,
      context_used: null,
      context_window: null,
      permission_denials: [],
      result: text,
    });
    this.#emit({ kind: "status", state: "Idle" });
  }

  // --- AskUserQuestion (J8) ---------------------------------------------------
  //
  // Imite le cœur sur `t4` (réponse) et `t10` (« ignorer ») : appel
  // AskUserQuestion dans le fil, `question` en attente, puis, à la réponse,
  // `question_closed` (`Answered` ou `Skipped`), `tool_result` du CLI et une
  // courte réponse de Claude. Erreurs du cœur : `UnknownRequest` pour une
  // requête inconnue, `Unsupported` pour « ignorer » sans la capacité.

  /**
   * Claude pose une question ; renvoie le `request_id`. `thread` : le
   * `tool_use_id` d'un sous-agent qui pose la question (il passe alors dans
   * le groupe « en attente » de la colonne des agents), `"main"` par défaut.
   */
  ask(questions: QuestionItem[], opts: { canSkip?: boolean; thread?: string } = {}): string {
    const n = this.#asked++;
    const requestId = `req_mock_q${n}`;
    const toolUseId = `toolu_mock_ask_${n}`;
    const canSkip = opts.canSkip ?? this.#caps.includes("ask_user_question_unanswered_v1");
    const msg = `msg_ask_${n}`;
    const input = {
      questions: questions.map((q) => ({
        question: q.question,
        header: q.header,
        options: q.options,
        multiSelect: q.multi_select,
      })),
    };
    const thread = opts.thread ?? "main";
    this.#questions.set(requestId, { toolUseId, thread, questions, canSkip });
    this.#emit({ kind: "status", state: "Requesting" });
    this.#emit({
      kind: "block_done",
      thread,
      msg,
      index: 0,
      block: { type: "ToolUse", tool_use_id: toolUseId, name: "AskUserQuestion", input },
    });
    this.#emit({ kind: "question", request_id: requestId, tool_use_id: toolUseId, thread, questions, can_skip: canSkip });
    return requestId;
  }

  /**
   * Comme `answer_question` (`t4`). Un tableau (multi-sélection) devient
   * dans le texte les labels joints par `,` sans espace, comme le CLI
   * 2.1.325 (`t16`).
   */
  answerQuestion(requestId: string, answers: Record<string, string | string[]>): void {
    const q = this.#takeQuestion(requestId);
    this.#emit({ kind: "question_closed", request_id: requestId, reason: "Answered" });
    const parts = Object.entries(answers).map(([k, v]) => `"${k}"="${String(v)}"`);
    this.#questionResult(
      q,
      `Your questions have been answered: ${parts.join(", ")}. You can now continue with these answers in mind.`,
    );
  }

  /** Comme `skip_question` (`t10`). */
  skipQuestion(requestId: string, _reason: "skipped" | "replied_in_chat" | "dismissed"): void {
    const q = this.#questions.get(requestId);
    if (q && !q.canSkip) throw { kind: "unsupported", message: "ask_user_question_unanswered_v1" };
    this.#takeQuestion(requestId);
    this.#emit({ kind: "question_closed", request_id: requestId, reason: "Skipped" });
    this.#questionResult(q!, "The user did not answer the questions.");
  }

  #takeQuestion(requestId: string): { toolUseId: string; thread: string; questions: QuestionItem[]; canSkip: boolean } {
    const q = this.#questions.get(requestId);
    if (!q) throw { kind: "unknown_request", message: requestId };
    this.#questions.delete(requestId);
    return q;
  }

  #questionResult(q: { toolUseId: string; thread: string }, text: string): void {
    this.#emit({
      kind: "tool_result",
      thread: q.thread,
      tool_use_id: q.toolUseId,
      content: [{ type: "Text", text }],
      is_error: false,
      meta: null,
    });
    this.streamReply(160);
  }

  /**
   * Regroupement du cœur (`state/batcher.rs`) : 256 événements, ou échéance
   * 16 ms après le dernier lot ; immédiat au bord avant d'une rafale (aucun
   * lot depuis 16 ms, ou événement précédent à 5 ms ou plus) et pour l'écho
   * d'un message tapé.
   */
  #emit(body: EventBody): void {
    const now = performance.now();
    const ev = this.factory.make(body, performance.timeOrigin + now);
    // Écho d'un message tapé : jamais retenu (comme `urgent` du cœur).
    const typed = body.kind === "user_message" && body.origin === "Typed";
    const leadingEdge = now - this.#lastSent >= 16 || now - this.#lastEvent >= 5 || typed;
    this.#lastEvent = now;
    if (this.#pending.length === 0 && leadingEdge) {
      this.#lastSent = now;
      this.#onEvents?.([ev]);
      return;
    }
    this.#pending.push(ev);
    if (this.#pending.length >= 256 || typed) this.#flush();
    else if (!this.#flushTimer)
      this.#flushTimer = setTimeout(() => this.#flush(), Math.max(0, this.#lastSent + 16 - now));
  }

  #flush(): void {
    if (this.#flushTimer) clearTimeout(this.#flushTimer);
    this.#flushTimer = null;
    if (this.#pending.length === 0) return;
    const b = this.#pending;
    this.#pending = [];
    this.#lastSent = performance.now();
    this.#onEvents?.(b);
  }
}

/** Paramètres du mode simulé lus dans l'URL : `?mock=long&rows=3000&rate=100`. */
export function mockOptionsFromUrl(search: string): (MockOptions & { name: string }) | null {
  const p = new URLSearchParams(search);
  const name = p.get("mock");
  if (name === null) return null;
  const num = (k: string) => {
    const v = p.get(k);
    return v === null || v === "" || Number.isNaN(Number(v)) ? undefined : Number(v);
  };
  return {
    name: name || "long",
    rows: num("rows") ?? (name === "demo" ? 60 : 3000),
    rate: num("rate"),
    seed: num("seed"),
  };
}

// --- J14 : auto-amélioration simulée ------------------------------------------

/** Étapes d'un cycle réussi, dans l'ordre (décision 17 de la spec). */
export const MOCK_IMPROVE_STEPS: ImproveStep[] = [
  "preparing",
  "working",
  "checking",
  "testing",
  "building",
  "waiting_idle",
  "switching",
  "merged",
];

/** Issue simulée d'un cycle (`?improve=` en mode simulé). */
export type MockImproveOutcome = "merged" | "restart_required" | "failed";

/** Étapes simulées pour chaque issue (relance requise : pas de build ni de bascule, décision 18). */
export function mockImproveSteps(outcome: MockImproveOutcome): ImproveStep[] {
  if (outcome === "merged") return MOCK_IMPROVE_STEPS;
  return ["preparing", "working", "checking", "testing", outcome];
}

/** Envoi simulé de la branche après la fusion (`?push=` ; propriétaire de l'app seulement). */
export type MockPushOutcome = "pushed" | "failed";

/** Raison d'un envoi simulé en échec. */
export const MOCK_PUSH_FAIL = "ssh: connect to host github.com port 22: Connection timed out";

export type MockSelfUpdateOptions = {
  outcome?: MockImproveOutcome;
  /** Envoi activé (propriétaire) et son issue ; absent : envoi désactivé (`off`), comme pour tout autre utilisateur. */
  push?: MockPushOutcome;
  /** État du clone (`disabled` : `/ameliorer` grisé avec la raison). */
  repo?: RepoStatus;
  /** Sessions vivantes rendues par `attachSessions` (rattachement après un rechargement). */
  live?: SessionResync[];
  /** Nombre d'appels à `attachSessions` rejetés d'abord (`attachFail=<n>`). */
  attachFail?: number;
};

/** Détail d'un échec simulé (étape `testing`). */
export const MOCK_FAIL_DETAIL = "npm test : 2 tests en échec (e2e/unit/resync.test.ts)";

/**
 * Sessions vivantes simulées pour `attachSessions` : la session principale
 * (`mock-1`, historique repris de `historyRows` lignes puis journal d'environ
 * `rows` lignes) et une session d'amélioration `improve-0` (rôle `improve`).
 */
export function mockLiveSessions(rows: number, historyRows = 0, seed = 15): SessionResync[] {
  const hf = new EventFactory();
  const history: HistoryLoaded | null =
    historyRows > 0
      ? {
          session_id: "11111111-2222-4333-8444-555555555555",
          events: buildHistory(hf, historyRows, seed + 7).events,
          lines: historyRows,
          used: historyRows,
          ignored: 0,
          unknown: 0,
          unknown_elements: 0,
          bad: 0,
          sidechain: 0,
          truncated: false,
          subagent_threads: 0,
          omitted_lines: 0,
          omitted_bytes: 0,
          clipped: 0,
          dropped_events: 0,
        }
      : null;
  const f = new EventFactory();
  const T0 = Date.UTC(2026, 10, 17, 9, 0, 0);
  const events: CoreEvent[] = [
    f.make({ kind: "starting", pid: 4242, cwd: "C:\\projet", args: [] }, T0),
    f.make(
      {
        kind: "ready",
        version: "2.1.325",
        capabilities: [],
        commands: [{ name: "review", description: "Relire le diff", argument_hint: null }],
        agents: [],
        models: [],
        permission_mode: "bypassPermissions",
      } as EventBody,
      T0,
    ),
    f.make(
      {
        kind: "init",
        session_id: "11111111-2222-4333-8444-555555555555",
        model: "claude-opus-5-5",
        permission_mode: "bypassPermissions",
        capabilities: ["ask_user_question_unanswered_v1"],
        tools: [],
        mcp_servers: [],
        skills: [],
        slash_commands: [],
        version: "2.1.325",
      },
      T0,
    ),
  ];
  // Messages tapés dans cette session (pas relus d'un transcript).
  for (const e of buildHistory(f, rows, seed).events)
    events.push(e.kind === "user_message" ? { ...e, origin: "Typed" } : e);
  events.push(f.make({ kind: "status", state: "Idle" }, T0));
  const g = new EventFactory();
  const improve: CoreEvent[] = [
    { ...g.make({ kind: "starting", pid: 5151, cwd: "C:\\mock\\work\\auto", args: [] }, T0), session: "improve-0" },
    { ...g.make({ kind: "status", state: "Idle" }, T0), session: "improve-0" },
  ];
  return [
    {
      session: "mock-1",
      role: "main",
      cwd: "C:\\projet",
      pid: 4242,
      resumed_from: history?.session_id ?? null,
      history,
      events,
      running: true,
    },
    {
      session: "improve-0",
      role: "improve",
      cwd: "C:\\mock\\work\\auto",
      pid: 5151,
      resumed_from: null,
      history: null,
      events: improve,
      running: true,
    },
  ];
}

/**
 * Paramètres J14 du mode simulé lus dans l'URL : `improve=merged|restart|failed`,
 * `push=pushed|failed` (envoi de la branche après la fusion, propriétaire),
 * `repo=disabled|cloning`, `attach=<lignes>` (sessions vivantes au montage),
 * `attachFail=<n>` (les n premiers rattachements échouent), `step=<ms>`
 * (durée d'une étape).
 */
export function mockSelfUpdateFromUrl(search: string): MockSelfUpdateOptions & { stepMs?: number } {
  const p = new URLSearchParams(search);
  const out: MockSelfUpdateOptions & { stepMs?: number } = {};
  const imp = p.get("improve");
  if (imp === "restart" || imp === "restart_required") out.outcome = "restart_required";
  else if (imp === "failed" || imp === "fail") out.outcome = "failed";
  const push = p.get("push");
  if (push === "pushed" || push === "ok") out.push = "pushed";
  else if (push === "failed" || push === "fail") out.push = "failed";
  const repo = p.get("repo");
  if (repo === "disabled") out.repo = { state: "disabled", reason: "git introuvable dans le PATH (simulé)" };
  else if (repo === "cloning") out.repo = { state: "cloning", path: "C:\\mock\\repo" };
  const attach = p.get("attach");
  if (attach !== null) out.live = mockLiveSessions(Number(attach) || 40);
  const attachFail = Number(p.get("attachFail"));
  if (Number.isInteger(attachFail) && attachFail > 0) out.attachFail = attachFail;
  const step = p.get("step");
  if (step !== null && step !== "" && Number.isFinite(Number(step))) out.stepMs = Number(step);
  return out;
}

/**
 * Commandes J14 simulées (`attach_sessions`, `ui_ready`, `get_ui_info`,
 * `improve_start`, `improve_cancel`, `ui_rollback`) : réponses conformes au
 * contrat. `improveStart` déroule les étapes de l'issue choisie, une toutes
 * les `stepMs` ms, et passe chaque `ImproveStatus` à `onStatus` et aux
 * abonnés de `onImproveStatus` (l'équivalent de l'événement global
 * `improve_status`). Pendant `working`, la session dédiée émet sur le canal
 * de `attachSessions` : elle doit rester hors de la conversation principale.
 */
export class MockSelfUpdate {
  info: UiInfo = {
    source: "embedded",
    active_hash: null,
    previous_hash: null,
    build_commit: "mock000",
    contract: "0000000000000000",
    restart_required: null,
    repo: { state: "ready", path: "C:\\mock\\repo", url: "git@example.invalid:mock.git", head: "mock000" },
    improve: null,
    push: { state: "off" },
  };
  readyCalls: (string | null)[] = [];
  /** Commandes reçues, dans l'ordre (vérifiées par les tests e2e). */
  calls: { cmd: string; args: unknown }[] = [];
  outcome: MockImproveOutcome;
  pushOutcome: MockPushOutcome | null;
  live: SessionResync[];
  attachFail: number;
  #timers: ReturnType<typeof setTimeout>[] = [];
  #n = 0;
  #onStatus: (s: ImproveStatus) => void;
  #listeners = new Set<(s: ImproveStatus) => void>();
  #attached: ((batch: CoreEvent[]) => void) | null = null;
  #stepMs: number;

  constructor(onStatus: (s: ImproveStatus) => void = () => {}, stepMs = 300, opts: MockSelfUpdateOptions = {}) {
    this.#onStatus = onStatus;
    this.#stepMs = stepMs;
    this.outcome = opts.outcome ?? "merged";
    this.pushOutcome = opts.push ?? null;
    if (this.pushOutcome) this.info.push = { state: "enabled", url: "git@example.invalid:mock.git" };
    this.live = opts.live ?? [];
    this.attachFail = opts.attachFail ?? 0;
    if (opts.repo) this.info.repo = opts.repo;
  }

  attachSessions(onEvents?: (batch: CoreEvent[]) => void): Promise<SessionResync[]> {
    this.calls.push({ cmd: "attach_sessions", args: {} });
    if (this.attachFail > 0) {
      this.attachFail--;
      return Promise.reject({ kind: "internal", message: "canal de rattachement refusé (simulé)" });
    }
    this.#attached = onEvents ?? null;
    return Promise.resolve(structuredClone(this.live));
  }

  uiReady(hash: string | null): Promise<boolean> {
    this.calls.push({ cmd: "ui_ready", args: { hash } });
    this.readyCalls.push(hash);
    return Promise.resolve(false);
  }

  getUiInfo(): Promise<UiInfo> {
    return Promise.resolve(structuredClone(this.info));
  }

  onImproveStatus(cb: (s: ImproveStatus) => void): Promise<() => void> {
    this.#listeners.add(cb);
    return Promise.resolve(() => {
      this.#listeners.delete(cb);
    });
  }

  #emit(s: ImproveStatus): void {
    this.info.improve = s;
    this.#onStatus(s);
    for (const l of this.#listeners) l(s);
  }

  improveStart(request: string): Promise<ImproveStatus> {
    this.calls.push({ cmd: "improve_start", args: { request } });
    const repo = this.info.repo;
    if (repo.state !== "ready") {
      const why = repo.state === "disabled" ? repo.reason : "clone indisponible";
      return Promise.reject({ kind: "unsupported", message: why });
    }
    const n = ++this.#n;
    const id = `improve-${n}`;
    const session = `improve-${n}`;
    const slug = request.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 32) || "demande";
    const commit = `mock${n}`.padEnd(7, "0");
    const status = (step: ImproveStep): ImproveStatus => ({
      id,
      branch: `auto/20261117-1200-${slug}`,
      request,
      step,
      session: step === "preparing" ? null : session,
      commit: step === "preparing" || step === "working" ? null : commit,
      detail: step === "failed" ? MOCK_FAIL_DETAIL : null,
      push: null,
      t: Date.now(),
    });
    mockImproveSteps(this.outcome).forEach((step, i) => {
      if (i === 0) return;
      this.#timers.push(
        setTimeout(() => {
          const s = status(step);
          if (step === "merged") {
            this.info.previous_hash = this.info.active_hash;
            this.info.active_hash = s.commit;
            this.info.source = "disk";
          } else if (step === "restart_required") {
            this.info.restart_required = s.commit;
          } else if (step === "working") {
            this.#improveEvents(session);
          }
          this.#emit(s);
          if (step === "merged" || step === "restart_required") this.#push(s);
        }, i * this.#stepMs),
      );
    });
    const first = status("preparing");
    this.info.improve = first;
    return Promise.resolve(first);
  }

  /** Envoi simulé de la branche après la fusion (propriétaire) : « en cours », puis son issue une étape plus tard. */
  #push(merged: ImproveStatus): void {
    const outcome = this.pushOutcome;
    if (!outcome) return;
    const branch = merged.branch;
    this.#emit({ ...merged, push: { state: "pushing", branch }, t: Date.now() });
    this.#timers.push(
      setTimeout(() => {
        const cur = this.info.improve;
        if (!cur || cur.id !== merged.id) return;
        this.#emit({
          ...cur,
          push:
            outcome === "pushed" ? { state: "pushed", branch } : { state: "failed", branch, reason: MOCK_PUSH_FAIL },
          t: Date.now(),
        });
      }, this.#stepMs),
    );
  }

  /** Quelques événements de la session dédiée, sur le canal du rattachement. */
  #improveEvents(session: string): void {
    const f = new EventFactory();
    const ev = (body: EventBody): CoreEvent => ({ ...f.make(body), session });
    this.#attached?.([
      ev({ kind: "starting", pid: 6000 + this.#n, cwd: "C:\\mock\\work\\auto", args: [] }),
      ev({ kind: "user_message", thread: "main", uuid: null, text: "Prompt d'amélioration (simulé)", origin: "Typed" }),
      ev({ kind: "status", state: "Streaming" }),
    ]);
  }

  improveCancel(id: string): Promise<void> {
    this.calls.push({ cmd: "improve_cancel", args: { id } });
    const cur = this.info.improve;
    if (!cur || cur.id !== id) return Promise.reject({ kind: "invalid_argument", message: id });
    this.#timers.forEach(clearTimeout);
    this.#timers = [];
    this.#emit({ ...cur, step: "cancelled", t: Date.now() });
    return Promise.resolve();
  }

  uiRollback(): Promise<UiInfo> {
    this.calls.push({ cmd: "ui_rollback", args: {} });
    const from = this.info.active_hash;
    this.info.active_hash = this.info.previous_hash;
    this.info.previous_hash = null;
    this.info.source = this.info.active_hash ? "disk" : "embedded";
    const cur = this.info.improve;
    if (cur)
      this.#emit({ ...cur, step: "rolled_back", commit: from ?? cur.commit, detail: null, push: null, t: Date.now() });
    return this.getUiInfo();
  }

  dispose(): void {
    this.#timers.forEach(clearTimeout);
    this.#timers = [];
    this.#listeners.clear();
  }
}

