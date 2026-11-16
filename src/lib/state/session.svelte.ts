// Magasin d'une session côté interface (plan 1.6).
//
// Il consomme les `CoreEvent` du cœur (ou du mode simulé) et tient le fil
// principal à plat, une `Row` par ligne virtualisée :
// - `rows` est en `$state.raw` : on remplace le tableau (une copie par image
//   au plus), on ne rend jamais réactif chaque objet. Le coût reste constant
//   sur une longue session.
// - `live` est la seule ligne réactive en profondeur : le bloc de texte en
//   cours de streaming. Elle est aussi dans `rows` (même objet), donc ses
//   `text_delta` ne touchent ni le tableau ni les autres lignes.
// - Les lots reçus sont mis en file et appliqués dans un seul
//   `requestAnimationFrame` par image (`push`). `apply` applique tout de suite
//   (tests, historique).
// - Les messages d'un sous-agent (`thread !== "main"`) vont dans `threads`,
//   jamais dans `rows`.

import type { AgentEvent } from "../core/bindings/AgentEvent";
import type { CommandInfo } from "../core/bindings/CommandInfo";
import type { CoreEvent } from "../core/bindings/CoreEvent";
import type { JsonValue } from "../core/bindings/serde_json/JsonValue";
import type { McpServer } from "../core/bindings/McpServer";
import type { ModelInfo } from "../core/bindings/ModelInfo";
import type { NoticeLevel } from "../core/bindings/NoticeLevel";
import type { QuestionItem } from "../core/bindings/QuestionItem";
import type { ResultPart } from "../core/bindings/ResultPart";
import type { SessionState } from "../core/bindings/SessionState";
import type { UserOrigin } from "../core/bindings/UserOrigin";

/** `at` : horodatage `t` de l'événement source (ms epoch), pour le rendu (heure, durée). */
export type UserRow = { kind: "user"; id: string; text: string; origin: UserOrigin; at: number };
export type AssistantTextRow = {
  kind: "assistant";
  id: string;
  /** `thread/msg/index` : clé du bloc dans le flux. */
  block: string;
  text: string;
  done: boolean;
  at: number;
};
export type ToolResult = { text: string; isError: boolean; meta: JsonValue | null };
export type ToolCallRow = {
  kind: "tool";
  id: string;
  toolUseId: string;
  name: string;
  input: JsonValue | null;
  result: ToolResult | null;
  at: number;
};
export type AgentCallRow = { kind: "agent"; id: string; toolUseId: string; agent: AgentEvent };
/** Fin d'un agent lancé en arrière-plan : ligne ajoutée au moment où il finit (`06`). */
export type AgentDoneRow = { kind: "agentDone"; id: string; toolUseId: string; agent: AgentEvent; at: number };
export type TurnResultRow = {
  kind: "turn";
  id: string;
  subtype: string;
  isError: boolean;
  durationMs: number;
  result: string | null;
  at: number;
  /** Agents d'arrière-plan encore en cours à la fin du tour (« Waiting for N background agents », `05`). */
  waiting: number;
};
export type NoticeRow = { kind: "notice"; id: string; level: NoticeLevel; code: string; message: string };
export type InterruptedRow = { kind: "interrupted"; id: string; text: string };

export type Row =
  | UserRow
  | AssistantTextRow
  | ToolCallRow
  | AgentCallRow
  | AgentDoneRow
  | TurnResultRow
  | NoticeRow
  | InterruptedRow;

/** `id` : unique dans la session (clé d'affichage), `seq` : événement source ou -1. */
export type Notice = { id: number; level: NoticeLevel; code: string; message: string; seq: number };
export type Question = {
  requestId: string;
  toolUseId: string | null;
  questions: QuestionItem[];
  canSkip: boolean;
};
export type SessionMeta = {
  /** Dossier de travail (`starting.cwd`) : chemins relatifs du rendu. */
  cwd: string | null;
  version: string | null;
  model: string | null;
  sessionId: string | null;
  permissionMode: string | null;
  models: ModelInfo[];
  commands: CommandInfo[];
  capabilities: string[];
  mcp: McpServer[];
};

/** Planificateur d'image ; remplaçable en test. */
export type FrameScheduler = (cb: () => void) => void;

const defaultScheduler: FrameScheduler = (cb) =>
  typeof requestAnimationFrame === "function" ? requestAnimationFrame(() => cb()) : setTimeout(cb, 16);

function blockKey(thread: string, msg: string, index: number): string {
  return `${thread}/${msg}/${index}`;
}

function resultText(parts: ResultPart[]): string {
  return parts
    .map((p) => (p.type === "Text" ? p.text : p.type === "Image" ? "[image]" : "[contenu]"))
    .join("\n");
}

/** États finaux d'un agent. */
const AGENT_OVER = new Set<AgentEvent["status"]>(["Completed", "Failed", "Stopped"]);

const INTERRUPTED_TEXT = "Interrupted · What should Claude do instead?";

export class SessionStore {
  /** Fil principal à plat (une ligne virtualisée par `Row`). */
  rows: Row[] = $state.raw([]);
  /** Bloc de texte en cours de streaming, réactif en profondeur. */
  live: AssistantTextRow | null = $state(null);
  agents: AgentEvent[] = $state([]);
  question: Question | null = $state(null);
  meta: SessionMeta = $state({
    cwd: null,
    version: null,
    model: null,
    sessionId: null,
    permissionMode: null,
    models: [],
    commands: [],
    capabilities: [],
    mcp: [],
  });
  status: SessionState | "Starting" = $state("Starting");
  banners: Notice[] = $state([]);
  /** Durée de réflexion (blocs Thinking) du tour en cours, en ms : « thought for Ns » (`k1`). */
  thinkingMs = $state(0);
  /** Dernier `seq` appliqué (détection de trou). */
  lastSeq = $state(-1);
  gaps = $state(0);
  /** Événements rejetés parce que leur application a levé une exception. */
  failed = $state(0);

  /** Fils des sous-agents (non réactifs : bonus du panneau, J7). */
  readonly threads = new Map<string, Row[]>();
  /** Notices Info (auto-allow…) : hors du fil, gardées pour les diagnostics. */
  readonly infos: Notice[] = [];
  /** `tool_use_id` → index dans `rows` (outil ou appel Agent). */
  readonly byToolUse = new Map<string, number>();

  /** Appelé après chaque application, avec le `t` de chaque `text_delta` du fil principal. */
  onApplied: ((info: { deltaTs: number[]; count: number }) => void) | null = null;

  #queue: CoreEvent[] = [];
  #scheduled = false;
  #schedule: FrameScheduler;
  /** Lignes du bloc vivant : index dans `rows`, par clé de bloc. */
  #blockIndex = new Map<string, number>();
  /** Copie de travail de `rows` pendant une application (copie paresseuse). */
  #next: Row[] | null = null;
  #idn = 0;
  #thinkStart: number | null = null;
  #noticeId = 0;

  constructor(schedule: FrameScheduler = defaultScheduler) {
    this.#schedule = schedule;
  }

  /** Reçoit un lot du canal ; il sera appliqué à la prochaine image. */
  push = (batch: CoreEvent[]): void => {
    for (const ev of batch) this.#queue.push(ev);
    if (!this.#scheduled) {
      this.#scheduled = true;
      this.#schedule(this.flush);
    }
  };

  /** Applique la file maintenant (appelé par l'image planifiée). */
  flush = (): void => {
    this.#scheduled = false;
    if (this.#queue.length === 0) return;
    const q = this.#queue;
    this.#queue = [];
    this.apply(q);
  };

  /**
   * Applique des événements tout de suite. Un événement qui lève une
   * exception est compté (`failed`) et ignoré ; les autres du lot sont
   * appliqués et la copie de travail est toujours publiée, pour que
   * `byToolUse` et les index de blocs restent alignés sur `rows`.
   */
  apply(events: CoreEvent[]): void {
    const deltaTs: number[] = [];
    this.#next = null;
    try {
      for (const ev of events) {
        if (this.lastSeq >= 0 && ev.seq !== this.lastSeq + 1) this.gaps++;
        this.lastSeq = ev.seq;
        if (ev.kind === "text_delta" && ev.thread === "main") deltaTs.push(ev.t);
        try {
          this.#reduce(ev);
        } catch (err) {
          this.failed++;
          console.error("événement rejeté", ev.kind, ev.seq, err);
        }
      }
    } finally {
      if (this.#next) {
        this.rows = this.#next;
        this.#next = null;
      }
    }
    this.onApplied?.({ deltaTs, count: events.length });
  }

  /** Bandeau en tête de la conversation (erreur locale de l'interface, par exemple). */
  addBanner(level: NoticeLevel, code: string, message: string, seq = -1): void {
    this.banners.push({ id: this.#noticeId++, level, code, message, seq });
  }

  /** Nombre d'événements en attente de la prochaine image. */
  get pending(): number {
    return this.#queue.length;
  }

  // --- interne -------------------------------------------------------------

  #work(): Row[] {
    if (!this.#next) this.#next = this.rows.slice();
    return this.#next;
  }

  #append(row: Row): number {
    const w = this.#work();
    w.push(row);
    return w.length - 1;
  }

  #replace(i: number, row: Row): void {
    this.#work()[i] = row;
  }

  #current(): Row[] {
    return this.#next ?? this.rows;
  }

  #id(prefix: string, seq: number): string {
    return `${prefix}${seq}-${this.#idn++}`;
  }

  #threadRows(thread: string): Row[] {
    let t = this.threads.get(thread);
    if (!t) {
      t = [];
      this.threads.set(thread, t);
    }
    return t;
  }

  #reduce(ev: CoreEvent): void {
    switch (ev.kind) {
      case "starting":
        this.status = "Starting";
        this.meta.cwd = ev.cwd;
        break;
      case "ready":
        this.meta.version = ev.version;
        this.meta.capabilities = ev.capabilities;
        this.meta.commands = ev.commands;
        this.meta.models = ev.models;
        if (ev.permission_mode) this.meta.permissionMode = ev.permission_mode;
        break;
      case "init":
        // Arrive à chaque tour : on fusionne, on ne réinitialise pas (spec 4.8).
        this.meta.sessionId = ev.session_id;
        this.meta.model = ev.model;
        this.meta.permissionMode = ev.permission_mode;
        if (ev.version) this.meta.version = ev.version;
        this.meta.capabilities = [...new Set([...this.meta.capabilities, ...ev.capabilities])];
        this.meta.mcp = ev.mcp_servers;
        break;
      case "status":
        this.status = ev.state;
        break;
      case "user_message": {
        // `Replay` = ré-émission d'un message déjà affiché à l'envoi.
        if (ev.origin === "Replay") break;
        // Nouveau tour : la durée de réflexion repart de zéro.
        if (ev.origin === "Typed" && ev.thread === "main") this.thinkingMs = 0;
        const row: Row =
          ev.origin === "Interrupted"
            ? { kind: "interrupted", id: this.#id("i", ev.seq), text: INTERRUPTED_TEXT }
            : { kind: "user", id: this.#id("u", ev.seq), text: ev.text, origin: ev.origin, at: ev.t };
        if (ev.thread === "main") this.#append(row);
        else this.#threadRows(ev.thread).push(row);
        break;
      }
      case "block_start": {
        const key = blockKey(ev.thread, ev.msg, ev.index);
        if (ev.block.type === "Text") {
          if (ev.thread !== "main") {
            this.#threadRows(ev.thread).push({
              kind: "assistant",
              id: this.#id("a", ev.seq),
              block: key,
              text: "",
              done: false,
              at: ev.t,
            });
            break;
          }
          // La ligne vivante : un proxy réactif, placé tel quel dans `rows`.
          this.live = {
            kind: "assistant",
            id: this.#id("a", ev.seq),
            block: key,
            text: "",
            done: false,
            at: ev.t,
          };
          this.#blockIndex.set(key, this.#append(this.live));
        } else if (ev.block.type === "ToolUse") {
          const row: ToolCallRow = {
            kind: "tool",
            id: this.#id("t", ev.seq),
            toolUseId: ev.block.tool_use_id,
            name: ev.block.name,
            input: null,
            result: null,
            at: ev.t,
          };
          if (ev.thread !== "main") {
            this.#threadRows(ev.thread).push(row);
            break;
          }
          // L'appel Agent est une seule ligne, mise à jour par les `agent`.
          if (this.byToolUse.has(row.toolUseId)) break;
          this.byToolUse.set(row.toolUseId, this.#append(row));
        }
        // Thinking : rien dans le fil (plan 1.7) ; seule sa durée est gardée.
        else if (ev.block.type === "Thinking" && ev.thread === "main") this.#thinkStart = ev.t;
        break;
      }
      case "text_delta": {
        if (ev.thread !== "main") {
          const rows = this.threads.get(ev.thread);
          const last = rows?.[rows.length - 1];
          if (last && last.kind === "assistant" && !last.done) last.text += ev.text;
          break;
        }
        const live = this.live;
        if (live && live.block === blockKey(ev.thread, ev.msg, ev.index)) {
          live.text += ev.text;
        }
        break;
      }
      case "block_done": {
        const key = blockKey(ev.thread, ev.msg, ev.index);
        if (ev.block.type === "Thinking" && ev.thread === "main" && this.#thinkStart !== null) {
          this.thinkingMs += Math.max(0, ev.t - this.#thinkStart);
          this.#thinkStart = null;
        }
        if (ev.thread !== "main") {
          this.#threadDone(ev.thread, key, ev);
          break;
        }
        if (ev.block.type === "Text") {
          // Le message complet fait foi et remplace le texte accumulé.
          const i = this.#blockIndex.get(key);
          const prev = i === undefined ? undefined : this.#current()[i];
          if (i !== undefined && prev && prev.kind === "assistant" && prev.block === key) {
            this.#replace(i, { kind: "assistant", id: prev.id, block: key, text: ev.block.text, done: true, at: prev.at });
            this.#blockIndex.delete(key);
          } else if (ev.block.text.length > 0) {
            this.#append({
              kind: "assistant",
              id: this.#id("a", ev.seq),
              block: key,
              text: ev.block.text,
              done: true,
              at: ev.t,
            });
          }
          if (this.live?.block === key) this.live = null;
        } else if (ev.block.type === "ToolUse") {
          const id = ev.block.tool_use_id;
          const i = this.byToolUse.get(id);
          const prev = i === undefined ? undefined : this.#current()[i];
          if (i !== undefined && prev && prev.kind === "tool" && prev.toolUseId === id) {
            this.#replace(i, { ...prev, name: ev.block.name, input: ev.block.input });
          } else if (i === undefined) {
            this.byToolUse.set(
              id,
              this.#append({
                kind: "tool",
                id: this.#id("t", ev.seq),
                toolUseId: id,
                name: ev.block.name,
                input: ev.block.input,
                result: null,
                at: ev.t,
              }),
            );
          }
        }
        break;
      }
      case "tool_result": {
        const result: ToolResult = { text: resultText(ev.content), isError: ev.is_error, meta: ev.meta };
        if (ev.thread !== "main") {
          const rows = this.threads.get(ev.thread);
          const row = rows?.find((r) => r.kind === "tool" && r.toolUseId === ev.tool_use_id);
          if (row && row.kind === "tool") row.result = result;
          break;
        }
        const i = this.byToolUse.get(ev.tool_use_id);
        const prev = i === undefined ? undefined : this.#current()[i];
        if (i !== undefined && prev && prev.kind === "tool" && prev.toolUseId === ev.tool_use_id) {
          this.#replace(i, { ...prev, result });
        }
        break;
      }
      case "agent": {
        const { kind: _k, seq: _s, t: _t, session: _se, ...agent } = ev;
        const a = agent as AgentEvent;
        const k = this.agents.findIndex((x) => x.tool_use_id === a.tool_use_id);
        const wasOver = k >= 0 && AGENT_OVER.has(this.agents[k].status);
        if (k >= 0) this.agents[k] = a;
        else this.agents.push(a);
        const i = this.byToolUse.get(a.tool_use_id);
        const prev = i === undefined ? undefined : this.#current()[i];
        // L'appel Agent (ligne `tool` puis `agent`) devient une ligne `agent` ;
        // jamais une autre ligne, même si l'index était faux.
        if (i !== undefined && prev && (prev.kind === "tool" || prev.kind === "agent") && prev.toolUseId === a.tool_use_id) {
          this.#replace(i, { kind: "agent", id: prev.id, toolUseId: a.tool_use_id, agent: a });
        } else if (i === undefined) {
          this.byToolUse.set(
            a.tool_use_id,
            this.#append({ kind: "agent", id: this.#id("g", ev.seq), toolUseId: a.tool_use_id, agent: a }),
          );
        }
        // Agent d'arrière-plan qui finit : sa propre ligne, à la suite du fil (`06`).
        if (a.background && AGENT_OVER.has(a.status) && !wasOver) {
          this.#append({ kind: "agentDone", id: this.#id("d", ev.seq), toolUseId: a.tool_use_id, agent: a, at: ev.t });
        }
        break;
      }
      case "question":
        this.question = {
          requestId: ev.request_id,
          toolUseId: ev.tool_use_id,
          questions: ev.questions,
          canSkip: ev.can_skip,
        };
        break;
      case "question_closed":
        if (this.question?.requestId === ev.request_id) this.question = null;
        break;
      case "turn_result":
        // Un bloc resté ouvert (tour interrompu) est figé tel quel.
        this.#closeLive();
        this.#append({
          kind: "turn",
          id: this.#id("r", ev.seq),
          subtype: ev.subtype,
          isError: ev.is_error,
          durationMs: ev.duration_ms,
          result: ev.result,
          at: ev.t,
          waiting: this.agents.filter((x) => x.background && !AGENT_OVER.has(x.status)).length,
        });
        break;
      case "notice": {
        if (ev.level === "Info") {
          this.infos.push({ id: this.#noticeId++, level: ev.level, code: ev.code, message: ev.message, seq: ev.seq });
        } else this.addBanner(ev.level, ev.code, ev.message, ev.seq);
        break;
      }
      case "rate_limit":
      case "stderr":
        break;
      case "exited":
        this.#closeLive();
        this.status = "Exited";
        this.addBanner("Error", "exited", `claude.exe s'est arrêté (code ${ev.code ?? "?"})`, ev.seq);
        break;
    }
  }

  #closeLive(): void {
    const live = this.live;
    if (!live) return;
    const i = this.#blockIndex.get(live.block);
    const prev = i === undefined ? undefined : this.#current()[i];
    if (i !== undefined && prev?.kind === "assistant" && prev.block === live.block) {
      this.#replace(i, { kind: "assistant", id: live.id, block: live.block, text: live.text, done: true, at: live.at });
      this.#blockIndex.delete(live.block);
    }
    this.live = null;
  }

  #threadDone(thread: string, key: string, ev: Extract<CoreEvent, { kind: "block_done" }>): void {
    const rows = this.#threadRows(thread);
    if (ev.block.type === "Text") {
      let r: Row | undefined;
      for (let i = rows.length - 1; i >= 0 && !r; i--) {
        const x = rows[i];
        if (x.kind === "assistant" && x.block === key) r = x;
      }
      if (r && r.kind === "assistant") {
        r.text = ev.block.text;
        r.done = true;
      } else {
        rows.push({
          kind: "assistant",
          id: this.#id("a", ev.seq),
          block: key,
          text: ev.block.text,
          done: true,
          at: ev.t,
        });
      }
    } else if (ev.block.type === "ToolUse") {
      const id = ev.block.tool_use_id;
      const r = rows.find((x) => x.kind === "tool" && x.toolUseId === id);
      if (r && r.kind === "tool") r.input = ev.block.input;
      else
        rows.push({
          kind: "tool",
          id: this.#id("t", ev.seq),
          toolUseId: id,
          name: ev.block.name,
          input: ev.block.input,
          result: null,
          at: ev.t,
        });
    }
  }
}
