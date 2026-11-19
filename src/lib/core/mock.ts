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
import type { ImproveWaiting } from "./bindings/ImproveWaiting";
import type { PushResult } from "./bindings/PushResult";
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

/**
 * Issue simulée d'un cycle (`?improve=` en mode simulé, J14 et J16) :
 * - `merged` : un tour, contrôles et tests verts, fusion ;
 * - `restart_required` : idem, mais relance requise (décision 18) ;
 * - `failed` : échec définitif après les tests (`MOCK_FAIL_DETAIL`) ;
 * - `fix` : tests en échec au premier passage, retour à la session, un tour
 *   de correction (`fixing`, tentative 1/3), puis fusion ;
 * - `exhausted` : 4 échecs de suite, donc 3 tours de correction puis
 *   `awaiting_user` (`attempts_exhausted`) ; un message de l'utilisateur
 *   relance un tour qui passe ;
 * - `no_commit` : le premier tour pose une question sans commiter, puis
 *   `awaiting_user` (`no_new_commit`) ;
 * - `violation` : la garde échoue après le premier tour, `failed` sans
 *   retour à la session.
 */
export type MockImproveOutcome =
  | "merged"
  | "restart_required"
  | "failed"
  | "fix"
  | "exhausted"
  | "no_commit"
  | "violation";

/**
 * Étapes simulées pour chaque issue, sans intervention de l'utilisateur
 * (relance requise : pas de build ni de bascule, décision 18).
 */
export function mockImproveSteps(outcome: MockImproveOutcome): ImproveStep[] {
  const checks: ImproveStep[] = ["checking", "testing"];
  const fix: ImproveStep[] = ["checking", "testing", "fixing"];
  switch (outcome) {
    case "merged":
      return MOCK_IMPROVE_STEPS;
    case "restart_required":
    case "failed":
      return ["preparing", "working", ...checks, outcome];
    case "fix":
      return ["preparing", "working", ...fix, ...MOCK_IMPROVE_STEPS.slice(2)];
    case "exhausted":
      return ["preparing", "working", ...fix, ...fix, ...fix, ...checks, "awaiting_user"];
    case "no_commit":
      return ["preparing", "working", "awaiting_user"];
    case "violation":
      return ["preparing", "working", "failed"];
  }
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

/** Détail d'un échec définitif simulé (issue `failed`, après les tests ; rien n'est renvoyé à la session). */
export const MOCK_FAIL_DETAIL = "npm ci : registre npm injoignable (simulé)";

/** Raison d'un échec de tests simulé, corrigible : renvoyée à la session (issues `fix`, `exhausted`). */
export const MOCK_TEST_FAIL = "npm test : 2 tests en échec (e2e/unit/resync.test.ts)";

/** Raison d'un contrôle simulé en échec quand la branche n'a aucun commit (« Valider » après un tour sans commit). */
export const MOCK_NO_COMMIT_FAIL = "aucun commit sur la branche";

/** Violation de la garde simulée (issue `violation`) : visible par l'utilisateur, jamais renvoyée à la session. */
export const MOCK_VIOLATION = "garde : .git/config modifié pendant le tour";

/** Plafond des tours de correction automatique (`MAX_AUTO_ATTEMPTS` côté Rust, J16). */
export const MOCK_MAX_ATTEMPTS = 3;

/**
 * Retour d'erreur simulé envoyé à la session (format de
 * `selfupdate/feedback.rs` : en-tête, étape, raison, extrait).
 */
export function mockFeedback(attempt: number, max: number, step: string, reason: string): string {
  return [
    `[Contrôle automatique — tentative ${attempt}/${max}]`,
    `Étape : ${step}`,
    `Raison : ${reason}`,
    "",
    "```",
    " FAIL  e2e/unit/resync.test.ts > rattachement > rejoue le journal",
    "AssertionError: expected 41 to be 40",
    " ❯ e2e/unit/resync.test.ts:88:24",
    "```",
    "Corrige puis commite ; les contrôles et les tests repartent à la fin du tour.",
  ].join("\n");
}

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
 * Paramètres J14 et J16 du mode simulé lus dans l'URL :
 * `improve=merged|restart|failed|fix|exhausted|nocommit|violation` (issue du
 * cycle, voir `MockImproveOutcome`), `push=pushed|failed` (envoi de la branche
 * après la fusion, propriétaire), `repo=disabled|cloning`, `attach=<lignes>`
 * (sessions vivantes au montage), `attachFail=<n>` (les n premiers
 * rattachements échouent), `step=<ms>` (durée d'une étape et d'un tour).
 */
export function mockSelfUpdateFromUrl(search: string): MockSelfUpdateOptions & { stepMs?: number } {
  const p = new URLSearchParams(search);
  const out: MockSelfUpdateOptions & { stepMs?: number } = {};
  const imp = p.get("improve");
  if (imp === "restart" || imp === "restart_required") out.outcome = "restart_required";
  else if (imp === "failed" || imp === "fail") out.outcome = "failed";
  else if (imp === "fix" || imp === "fixing") out.outcome = "fix";
  else if (imp === "exhausted" || imp === "attempts_exhausted") out.outcome = "exhausted";
  else if (imp === "nocommit" || imp === "no_commit" || imp === "no_new_commit") out.outcome = "no_commit";
  else if (imp === "violation" || imp === "guard") out.outcome = "violation";
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

/** Étapes finales d'un cycle (plus de tour ni de contrôle ; `improve_dismiss` permis). */
const FINAL_STEPS: ReadonlySet<ImproveStep> = new Set([
  "merged",
  "restart_required",
  "failed",
  "cancelled",
  "rolled_back",
]);

/** Cycle simulé : ce que le pipeline (`selfupdate/pipeline.rs`) tient en mémoire. */
export type MockCycle = {
  n: number;
  id: string;
  session: string;
  branch: string;
  request: string;
  step: ImproveStep;
  attempt: number;
  turn: number;
  waiting: ImproveWaiting | null;
  commit: string | null;
  commits: number;
  detail: string | null;
  push: PushResult | null;
  /** Journal de la conversation du cycle (un `Outlet` pour tous les tours, `seq` continu). */
  journal: CoreEvent[];
  seq: number;
  /** Messages reçus pendant un tour, livrés au tour suivant. */
  queued: string[];
  /** Tour en cours, et s'il commitera. */
  inTurn: boolean;
  turnCommits: boolean;
  /** Échecs de tests encore à simuler, tours sans commit encore à simuler. */
  failsLeft: number;
  noCommitLeft: number;
  violation: boolean;
  end: "merged" | "restart_required" | "failed";
  /** Onglet fermé après un état final (`improve_dismiss`) : plus rendu par `attachSessions`. */
  dismissed: boolean;
};

/** Clé du `sessionStorage` où le cœur simulé garde son état à travers un rechargement. */
export const MOCK_SELFUPDATE_KEY = "orchestrator.mock.selfupdate.v1";

/**
 * Stockage du cœur simulé : seulement dans la page de l'app en mode simulé
 * (`?mock=` dans l'URL), jamais dans les tests unitaires. Le vrai cœur survit
 * au rechargement de l'interface ; le simulé, qui vit dans la page, garde son
 * état ici pour le reprendre.
 */
function mockStorage(): Storage | null {
  try {
    if (typeof sessionStorage === "undefined" || typeof location === "undefined") return null;
    return new URLSearchParams(location.search).has("mock") ? sessionStorage : null;
  } catch {
    return null;
  }
}

/**
 * Commandes J14 et J16 simulées (`attach_sessions`, `ui_ready`, `get_ui_info`,
 * `improve_start`, `improve_cancel`, `improve_send`, `improve_validate`,
 * `improve_interrupt`, `improve_dismiss`, `ui_rollback`) : réponses conformes
 * au contrat.
 *
 * Le cycle suit le pipeline itératif (décision 17) : `preparing`, puis des
 * tours (`working`, ou `fixing` pour une correction automatique) suivis de la
 * garde, puis, si le tour a commité, `checking` et `testing` ; un échec
 * corrigible renvoie l'erreur à la session (message `[Contrôle automatique —
 * tentative n/3]`) et relance un tour `fixing`, au plus `MOCK_MAX_ATTEMPTS`
 * fois, puis `awaiting_user`. Chaque étape dure `stepMs` ms, un tour aussi.
 *
 * La conversation du cycle est une vraie session (rôle `improve`, clé
 * `improve-<n>`) : ses événements (`starting`, message, texte, outils,
 * `turn_result`, `exited` attendu entre deux tours, et les lignes d'état
 * `notice` de code `improve.*`) partent sur le canal de `attachSessions`, et
 * `attachSessions` la rend tant que l'onglet n'est pas fermé
 * (`improveDismiss`). Chaque `ImproveStatus` est passé à `onStatus` et aux
 * abonnés de `onImproveStatus` (l'événement global `improve_status`).
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
  /** Passages de la garde (un après chaque tour, avec ou sans commit). */
  guardRuns = 0;
  outcome: MockImproveOutcome;
  pushOutcome: MockPushOutcome | null;
  live: SessionResync[];
  attachFail: number;
  #timers: ReturnType<typeof setTimeout>[] = [];
  #n = 0;
  #cycle: MockCycle | null = null;
  #onStatus: (s: ImproveStatus) => void;
  #listeners = new Set<(s: ImproveStatus) => void>();
  #attached: ((batch: CoreEvent[]) => void) | null = null;
  #stepMs: number;
  #storage: Storage | null;

  constructor(onStatus: (s: ImproveStatus) => void = () => {}, stepMs = 300, opts: MockSelfUpdateOptions = {}) {
    this.#onStatus = onStatus;
    this.#stepMs = stepMs;
    this.outcome = opts.outcome ?? "merged";
    this.pushOutcome = opts.push ?? null;
    if (this.pushOutcome) this.info.push = { state: "enabled", url: "git@example.invalid:mock.git" };
    this.live = opts.live ?? [];
    this.attachFail = opts.attachFail ?? 0;
    if (opts.repo) this.info.repo = opts.repo;
    this.#storage = mockStorage();
    this.#restore();
  }

  /** Cycle en cours ou dernier cycle (lecture seule, pour les tests). */
  get cycle(): Readonly<MockCycle> | null {
    return this.#cycle;
  }

  attachSessions(onEvents?: (batch: CoreEvent[]) => void): Promise<SessionResync[]> {
    this.calls.push({ cmd: "attach_sessions", args: {} });
    if (this.attachFail > 0) {
      this.attachFail--;
      return Promise.reject({ kind: "internal", message: "canal de rattachement refusé (simulé)" });
    }
    this.#attached = onEvents ?? null;
    const c = this.#cycle;
    const own: SessionResync[] =
      c && !c.dismissed && c.journal.length > 0
        ? [
            {
              session: c.session,
              role: "improve",
              cwd: this.#cwd(),
              pid: this.#pid(),
              resumed_from: null,
              history: null,
              events: c.journal,
              running: c.inTurn,
            },
          ]
        : [];
    return Promise.resolve(structuredClone([...this.live, ...own]));
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

  improveStart(request: string): Promise<ImproveStatus> {
    this.calls.push({ cmd: "improve_start", args: { request } });
    const repo = this.info.repo;
    if (repo.state !== "ready") {
      const why = repo.state === "disabled" ? repo.reason : "clone indisponible";
      return Promise.reject({ kind: "unsupported", message: why });
    }
    const cur = this.#cycle;
    if (cur && !FINAL_STEPS.has(cur.step))
      return Promise.reject({ kind: "rejected", message: "une amélioration est déjà en cours" });
    const n = ++this.#n;
    const slug = request.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 32) || "demande";
    const o = this.outcome;
    // Un seul cycle à la fois : le précédent (final) est remplacé.
    this.#cycle = {
      n,
      id: `improve-${n}`,
      session: `improve-${n}`,
      branch: `auto/20261117-1200-${slug}`,
      request,
      step: "preparing",
      attempt: 0,
      turn: 0,
      waiting: null,
      commit: null,
      commits: 0,
      detail: null,
      push: null,
      journal: [],
      seq: 0,
      queued: [],
      inTurn: false,
      turnCommits: false,
      failsLeft: o === "fix" ? 1 : o === "exhausted" ? MOCK_MAX_ATTEMPTS + 1 : 0,
      noCommitLeft: o === "no_commit" ? 1 : 0,
      violation: o === "violation",
      end: o === "restart_required" || o === "failed" ? o : "merged",
      dismissed: false,
    };
    const first = this.#status();
    this.info.improve = first;
    this.#note("Info", "improve.preparing", `✻ Préparation de la branche ${this.#cycle.branch}`);
    this.#after(this.#stepMs, () => this.#startTurn("working", request));
    return Promise.resolve(first);
  }

  /**
   * Message de l'utilisateur à la session du cycle. Pendant un tour (ou la
   * préparation) : mis en file, livré comme tour suivant (garde entre les
   * deux, pas de tests). Pendant les contrôles ou les tests : ceux-ci sont
   * abandonnés et un tour part. En attente : un tour part. Chaque message
   * remet `attempt` à 0 (au départ du tour qui le porte).
   */
  improveSend(id: string, text: string): Promise<void> {
    this.calls.push({ cmd: "improve_send", args: { id, text } });
    const c = this.#cycle;
    if (!c || c.id !== id) return Promise.reject({ kind: "invalid_argument", message: id });
    const msg = text.trim();
    if (!msg) return Promise.reject({ kind: "invalid_argument", message: "message vide" });
    switch (c.step) {
      case "preparing":
      case "working":
      case "fixing":
        c.queued.push(msg);
        return Promise.resolve();
      case "checking":
      case "testing":
        this.#clearTimers();
        this.#note("Info", "improve.aborted", "✻ Contrôles abandonnés : nouveau message");
        this.#startTurn("working", msg);
        return Promise.resolve();
      case "awaiting_user":
        this.#startTurn("working", msg);
        return Promise.resolve();
      default:
        return Promise.reject({ kind: "rejected", message: `pas de message à l'étape ${c.step}` });
    }
  }

  /** « Valider » : seulement en attente ; garde, contrôles et tests ; `attempt` inchangé. */
  improveValidate(id: string): Promise<void> {
    this.calls.push({ cmd: "improve_validate", args: { id } });
    const c = this.#cycle;
    if (!c || c.id !== id) return Promise.reject({ kind: "invalid_argument", message: id });
    if (c.step !== "awaiting_user")
      return Promise.reject({ kind: "rejected", message: `rien à valider à l'étape ${c.step}` });
    this.#checks();
    return Promise.resolve();
  }

  /** Échap dans l'onglet : le tour en cours s'arrête, le cycle attend (`interrupted`), sans tests. */
  improveInterrupt(id: string): Promise<void> {
    this.calls.push({ cmd: "improve_interrupt", args: { id } });
    const c = this.#cycle;
    if (!c || c.id !== id) return Promise.reject({ kind: "invalid_argument", message: id });
    if (!c.inTurn) return Promise.reject({ kind: "rejected", message: "aucun tour en cours" });
    this.#clearTimers();
    this.#endTurn(true);
    return Promise.resolve();
  }

  /** Fermeture de l'onglet après un état final : la conversation du cycle n'est plus rattachée. */
  improveDismiss(id: string): Promise<void> {
    this.calls.push({ cmd: "improve_dismiss", args: { id } });
    const c = this.#cycle;
    if (!c || c.id !== id) return Promise.resolve();
    if (!FINAL_STEPS.has(c.step))
      return Promise.reject({ kind: "rejected", message: "amélioration en cours : l'annuler d'abord" });
    c.dismissed = true;
    this.#save();
    return Promise.resolve();
  }

  /** Annulation (bouton ou fermeture de l'onglet) : branche gardée non fusionnée ; impossible à partir de `switching`. */
  improveCancel(id: string): Promise<void> {
    this.calls.push({ cmd: "improve_cancel", args: { id } });
    const c = this.#cycle;
    if (!c || c.id !== id) return Promise.reject({ kind: "invalid_argument", message: id });
    if (c.step === "switching" || FINAL_STEPS.has(c.step))
      return Promise.reject({ kind: "rejected", message: `annulation impossible à l'étape ${c.step}` });
    this.#clearTimers();
    if (c.inTurn) {
      c.inTurn = false;
      this.#events([
        { kind: "status", state: "Idle" },
        { kind: "exited", code: 1, during_turn: true, stderr_tail: [] },
      ]);
    }
    this.#note("Warn", "improve.cancelled", `✗ Amélioration annulée : branche ${c.branch} gardée non fusionnée`);
    this.#set({ step: "cancelled", waiting: null });
    return Promise.resolve();
  }

  uiRollback(): Promise<UiInfo> {
    this.calls.push({ cmd: "ui_rollback", args: {} });
    const from = this.info.active_hash;
    this.info.active_hash = this.info.previous_hash;
    this.info.previous_hash = null;
    this.info.source = this.info.active_hash ? "disk" : "embedded";
    const c = this.#cycle;
    if (c) this.#set({ step: "rolled_back", commit: from ?? c.commit, detail: null, push: null, waiting: null });
    return this.getUiInfo();
  }

  dispose(): void {
    this.#clearTimers();
    this.#listeners.clear();
  }

  // --- déroulement du cycle --------------------------------------------------

  #after(ms: number, fn: () => void): void {
    this.#timers.push(setTimeout(fn, ms));
  }

  #clearTimers(): void {
    this.#timers.forEach(clearTimeout);
    this.#timers = [];
  }

  #pid(): number {
    const c = this.#cycle;
    return c ? 6000 + c.n * 10 + c.turn : 6000;
  }

  #cwd(): string {
    return `C:\\mock\\work\\${this.#cycle!.branch.replace("/", "-")}`;
  }

  #status(): ImproveStatus {
    const c = this.#cycle!;
    return {
      id: c.id,
      branch: c.branch,
      request: c.request,
      step: c.step,
      session: c.step === "preparing" ? null : c.session,
      commit: c.commit,
      detail: c.detail,
      push: c.push,
      t: Date.now(),
      attempt: c.attempt,
      max_attempts: MOCK_MAX_ATTEMPTS,
      turn: c.turn,
      waiting: c.step === "awaiting_user" ? c.waiting : null,
    };
  }

  /** Nouvel état du cycle, publié comme `improve_status`. */
  #set(patch: Partial<MockCycle>): void {
    const c = this.#cycle!;
    Object.assign(c, patch);
    const s = this.#status();
    this.info.improve = s;
    this.#save();
    this.#onStatus(s);
    for (const l of this.#listeners) l(s);
  }

  /** Événements de la conversation du cycle : journal, puis canal du rattachement. */
  #events(bodies: EventBody[]): void {
    const c = this.#cycle!;
    const evs = bodies.map((b) => ({ ...b, seq: c.seq++, t: Date.now(), session: c.session }) as CoreEvent);
    c.journal.push(...evs);
    this.#save();
    this.#attached?.(evs);
  }

  /** Ligne d'état dans le fil (`PipelineHost::note` : `notice` de code `improve.*`). */
  #note(level: "Info" | "Warn" | "Error", code: string, message: string): void {
    this.#events([{ kind: "notice", level, code, message, detail: null }]);
  }

  /** Un tour de la session : premier lancement (`--session-id`), puis `--resume` du même identifiant. */
  #startTurn(kind: "working" | "fixing", message: string): void {
    const c = this.#cycle!;
    const commits = kind === "fixing" || c.noCommitLeft <= 0;
    if (!commits) c.noCommitLeft--;
    c.inTurn = true;
    c.turnCommits = commits;
    const msg = `msg_improve_${c.n}_${c.turn}`;
    this.#set({ step: kind, waiting: null, detail: null, ...(kind === "working" ? { attempt: 0 } : {}) });
    if (kind === "fixing")
      this.#note("Info", "improve.fixing", `✻ Correction automatique (tentative ${c.attempt}/${MOCK_MAX_ATTEMPTS})`);
    this.#events([
      {
        kind: "starting",
        pid: this.#pid(),
        cwd: this.#cwd(),
        args: c.turn === 0 ? ["--session-id", c.id] : ["--resume", c.id],
      },
      { kind: "user_message", thread: "main", uuid: null, text: message, origin: "Typed" },
      { kind: "status", state: "Streaming" },
    ]);
    this.#after(this.#stepMs / 2, () => this.#turnBody(kind, commits, msg));
    this.#after(this.#stepMs, () => this.#endTurn(false));
  }

  /** Milieu du tour : une question en texte (sans commit), ou une modification commitée. */
  #turnBody(kind: "working" | "fixing", commits: boolean, msg: string): void {
    const c = this.#cycle!;
    const text = (index: number, t: string): EventBody[] => [
      { kind: "block_start", thread: "main", msg, index, block: { type: "Text" } },
      { kind: "block_done", thread: "main", msg, index, block: { type: "Text", text: t } },
    ];
    const tool = (index: number, id: string, name: string, input: Record<string, string>, out: string): EventBody[] => [
      { kind: "block_start", thread: "main", msg, index, block: { type: "ToolUse", tool_use_id: id, name } },
      { kind: "block_done", thread: "main", msg, index, block: { type: "ToolUse", tool_use_id: id, name, input } },
      { kind: "tool_result", thread: "main", tool_use_id: id, content: [{ type: "Text", text: out }], is_error: false, meta: null },
    ];
    if (!commits) {
      this.#events(
        text(
          0,
          "Avant de modifier quoi que ce soit : faut-il garder l'ancien comportement derrière un réglage, " +
            "ou le remplacer partout ? Je ne commite rien tant que je n'ai pas ta réponse.",
        ),
      );
      return;
    }
    const base = `toolu_improve_${c.n}_${c.turn}`;
    this.#events([
      ...text(
        0,
        kind === "fixing"
          ? "Le contrôle automatique signale des tests en échec : je corrige `src/App.svelte`, puis je commite."
          : "Je modifie `src/App.svelte`, puis je commite.",
      ),
      ...tool(1, `${base}_e`, "Edit", { file_path: "src/App.svelte" }, "The file src/App.svelte has been updated."),
      ...tool(
        2,
        `${base}_g`,
        "Bash",
        { command: "git commit -F .git/MSG -- src/App.svelte" },
        `[${c.branch} ${this.#nextCommit()}] ${c.request}`,
      ),
    ]);
  }

  #nextCommit(): string {
    const c = this.#cycle!;
    const k = c.commits + 1;
    return (`mock${c.n}` + (k > 1 ? String.fromCharCode(96 + k) : "")).padEnd(7, "0");
  }

  /** Fin d'un tour : arrêt du processus (`exited` attendu), garde, puis suite selon le tour. */
  #endTurn(interrupted: boolean): void {
    const c = this.#cycle!;
    const committed = c.turnCommits && !interrupted;
    if (committed) {
      c.commit = this.#nextCommit();
      c.commits++;
    }
    c.inTurn = false;
    c.turn++;
    this.#events([
      {
        kind: "turn_result",
        subtype: interrupted ? "error_during_execution" : "success",
        is_error: false,
        terminal_reason: interrupted ? "interrupted" : null,
        duration_ms: this.#stepMs,
        cost_usd: 0.01,
        num_turns: 1,
        context_used: null,
        context_window: null,
        permission_denials: [],
        result: null,
      },
      { kind: "status", state: "Idle" },
      { kind: "exited", code: 0, during_turn: false, stderr_tail: [] },
    ]);
    // Garde après chaque tour, avec ou sans commit ; une violation n'est jamais renvoyée à la session.
    this.guardRuns++;
    if (c.violation && c.turn === 1) {
      this.#set({ step: "failed", detail: MOCK_VIOLATION });
      this.#note("Error", "improve.failed", `✗ Échec (Checking) : ${MOCK_VIOLATION} ; branche ${c.branch} gardée non fusionnée`);
      return;
    }
    this.#note("Info", "improve.guard", `✓ Garde de sécurité (tour ${c.turn})`);
    if (c.queued.length > 0) {
      this.#startTurn("working", c.queued.splice(0).join("\n\n"));
      return;
    }
    if (interrupted) {
      this.#set({ step: "awaiting_user", waiting: "interrupted", detail: "tour interrompu" });
      this.#note("Info", "improve.awaiting", "⏸ Tour interrompu : en attente de votre message");
      return;
    }
    if (!committed) {
      this.#set({
        step: "awaiting_user",
        waiting: "no_new_commit",
        detail: "tour terminé sans nouveau commit : rien à tester",
      });
      this.#note("Info", "improve.awaiting", "⏸ Tour terminé sans nouveau commit : répondez, ou « Valider »");
      return;
    }
    this.#checks();
  }

  /** Contrôles déterministes puis tests ; un échec corrigible repart vers la session. */
  #checks(): void {
    const S = this.#stepMs;
    const c = this.#cycle!;
    this.#set({ step: "checking", waiting: null, detail: null });
    this.#note("Info", "improve.checking", "✻ Contrôles…");
    this.#after(S, () => {
      if (c.commit === null) {
        this.#failed("checking", "contrôles", MOCK_NO_COMMIT_FAIL);
        return;
      }
      this.#set({ step: "testing" });
      this.#note("Info", "improve.testing", "✻ Tests…");
      this.#after(S, () => {
        if (c.failsLeft > 0) {
          c.failsLeft--;
          this.#failed("testing", "tests", MOCK_TEST_FAIL);
          return;
        }
        this.#passed();
      });
    });
  }

  /** Échec corrigible : retour à la session (tour `fixing`), ou attente une fois les tentatives épuisées. */
  #failed(step: "checking" | "testing", label: string, reason: string): void {
    const c = this.#cycle!;
    const head = step === "testing" ? "Tests en échec" : "Contrôles en échec";
    if (c.attempt < MOCK_MAX_ATTEMPTS) {
      const attempt = c.attempt + 1;
      this.#note("Warn", "improve.fault", `✗ ${head} (tentative ${attempt}/${MOCK_MAX_ATTEMPTS}) : erreur renvoyée à la session`);
      c.attempt = attempt;
      this.#startTurn("fixing", mockFeedback(attempt, MOCK_MAX_ATTEMPTS, label, reason));
      return;
    }
    this.#note(
      "Warn",
      "improve.fault",
      `✗ ${head} : ${MOCK_MAX_ATTEMPTS} corrections automatiques sans succès, à vous de reprendre`,
    );
    this.#set({ step: "awaiting_user", waiting: "attempts_exhausted", detail: reason });
    this.#note("Info", "improve.awaiting", "⏸ Corrections automatiques épuisées : en attente de votre message");
  }

  /** Contrôles et tests verts : fusion (build, attente du repos, bascule), relance requise, ou échec définitif. */
  #passed(): void {
    const S = this.#stepMs;
    const c = this.#cycle!;
    if (c.end === "failed") {
      this.#set({ step: "failed", detail: MOCK_FAIL_DETAIL });
      this.#note("Error", "improve.failed", `✗ Échec (Testing) : ${MOCK_FAIL_DETAIL} ; branche ${c.branch} gardée non fusionnée`);
      return;
    }
    if (c.end === "restart_required") {
      this.info.restart_required = c.commit;
      this.#set({ step: "restart_required", detail: null });
      this.#note("Info", "improve.restart_required", `✓ Fusionné ${c.commit} : relance requise (src-tauri/ modifié)`);
      this.#push();
      return;
    }
    this.#set({ step: "building", detail: null });
    this.#note("Info", "improve.building", "✻ Construction de l'interface…");
    this.#after(S, () => {
      this.#set({ step: "waiting_idle" });
      this.#note("Info", "improve.waiting_idle", "✻ Attente du repos de la session principale…");
      this.#after(S, () => {
        this.#set({ step: "switching" });
        this.#note("Info", "improve.switching", "✻ Bascule vers la nouvelle interface…");
        this.#after(S, () => {
          this.info.previous_hash = this.info.active_hash;
          this.info.active_hash = c.commit;
          this.info.source = "disk";
          this.#set({ step: "merged" });
          this.#note("Info", "improve.merged", `✓ Fusionné ${c.commit}`);
          this.#push();
        });
      });
    });
  }

  /** Envoi simulé de la branche après la fusion (propriétaire) : « en cours », puis son issue une étape plus tard. */
  #push(): void {
    const outcome = this.pushOutcome;
    const c = this.#cycle!;
    if (!outcome) return;
    const branch = c.branch;
    this.#set({ push: { state: "pushing", branch } });
    this.#after(this.#stepMs, () => {
      if (this.#cycle !== c) return;
      this.#set({
        push: outcome === "pushed" ? { state: "pushed", branch } : { state: "failed", branch, reason: MOCK_PUSH_FAIL },
      });
    });
  }

  // --- état gardé à travers un rechargement (mode simulé seulement) ----------

  #save(): void {
    if (!this.#storage) return;
    const { source, active_hash, previous_hash, restart_required } = this.info;
    try {
      this.#storage.setItem(
        MOCK_SELFUPDATE_KEY,
        JSON.stringify({
          n: this.#n,
          guardRuns: this.guardRuns,
          info: { source, active_hash, previous_hash, restart_required },
          cycle: this.#cycle,
        }),
      );
    } catch {
      // Stockage plein ou refusé : l'état ne survivra pas au rechargement.
    }
  }

  /**
   * Reprise après un rechargement de la page : un état final ou une attente
   * restent tels quels ; une étape en cours reprend au début de sa phase
   * (fin du tour, contrôles, ou suite de la fusion), une étape plus tard.
   */
  #restore(): void {
    if (!this.#storage) return;
    type Saved = { n?: number; guardRuns?: number; info?: Partial<UiInfo>; cycle?: MockCycle | null };
    let saved: Saved | null = null;
    try {
      saved = JSON.parse(this.#storage.getItem(MOCK_SELFUPDATE_KEY) ?? "null") as Saved | null;
    } catch {
      saved = null;
    }
    if (!saved || typeof saved !== "object") return;
    this.#n = saved.n ?? 0;
    this.guardRuns = saved.guardRuns ?? 0;
    Object.assign(this.info, saved.info ?? {});
    const c = saved.cycle;
    if (!c) return;
    this.#cycle = c;
    this.info.improve = this.#status();
    const S = this.#stepMs;
    switch (c.step) {
      case "preparing":
        this.#after(S, () => this.#startTurn("working", c.request));
        break;
      case "working":
      case "fixing":
        this.#after(S, () => this.#endTurn(false));
        break;
      case "checking":
      case "testing":
        this.#after(S, () => this.#checks());
        break;
      case "building":
      case "waiting_idle":
      case "switching":
        this.#after(S, () => this.#passed());
        break;
      default:
        break;
    }
  }
}
