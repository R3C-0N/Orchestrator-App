// Scénarios de la page de démonstration de la colonne des agents (J9).
//
// - `t3` : la fixture `fixtures/ui-events/t3.json` (deux agents, bleu et
//   rouge), rejouée en temps réel. Ses horodatages sont serrés (600 ms en
//   tout) : ils sont étirés (`stretch`, 20 par défaut → 12 s) et recalés sur
//   l'instant du rejeu, `started_at`/`ended_at` compris, pour voir la durée
//   avancer puis se figer.
// - `four` : un orchestrateur avec 4 agents en parallèle (bleu, vert,
//   orange, violet) plus un agent de couleur inconnue (gris), avec des
//   outils, un échec et des descriptions longues : lisibilité de la colonne.
// - `sort` : tri de la colonne ; 4 agents lancés dans l'ordre bleu, vert,
//   rouge, orange ; vert pose une question (groupe « en attente »), rouge
//   finit puis orange échoue (panneau des terminés, replié).

import type { AgentEvent } from "../../../core/bindings/AgentEvent";
import type { CoreEvent } from "../../../core/bindings/CoreEvent";
import type { EventBody } from "../../../core/bindings/EventBody";

export type Timed = { at: number; ev: CoreEvent };

/** Recale une suite d'événements : `t` (et les heures d'agent) = base + (t − t0) × stretch. */
export function retime(events: readonly CoreEvent[], base: number, stretch: number): Timed[] {
  if (events.length === 0) return [];
  const t0 = events[0].t;
  const map = (x: number) => Math.round(base + (x - t0) * stretch);
  return events.map((e) => {
    const ev = { ...e, t: map(e.t) } as CoreEvent;
    if (ev.kind === "agent") {
      if (ev.started_at !== null) ev.started_at = map(ev.started_at);
      if (ev.ended_at !== null) ev.ended_at = map(ev.ended_at);
    }
    return { at: ev.t - base, ev };
  });
}

/** Joue des événements recalés à leur heure ; renvoie une fonction d'arrêt. */
export function play(timed: readonly Timed[], push: (b: CoreEvent[]) => void, onEnd?: () => void): () => void {
  const timers = timed.map(({ at, ev }) => setTimeout(() => push([ev]), Math.max(0, at)));
  const last = timed.length ? timed[timed.length - 1].at : 0;
  const end = setTimeout(() => onEnd?.(), last + 50);
  return () => {
    for (const t of timers) clearTimeout(t);
    clearTimeout(end);
  };
}

type AgentSpec = {
  id: string;
  type: string;
  color: string | null;
  description: string;
  tools: { name: string; input: Record<string, unknown>; result: string; isError?: boolean }[];
  answer: string;
  /** Durée totale (ms). */
  ms: number;
  fail?: boolean;
};

const AGENTS: AgentSpec[] = [
  {
    id: "toolu_four_bleu",
    type: "agent-bleu",
    color: "blue",
    description: "Recherche des appels au réducteur dans le cœur Rust",
    tools: [
      { name: "Grep", input: { pattern: "reduce\\(", path: "src-tauri/src" }, result: "src-tauri/src/state/reducer.rs\nsrc-tauri/tests/fixtures_replay.rs" },
      { name: "Read", input: { file_path: "C:\\projet\\src-tauri\\src\\state\\reducer.rs" }, result: "1\tuse crate::api::events::*;\n2\t\n3\tpub fn reduce() {}" },
    ],
    answer: "Deux appelants : le superviseur et le test de rejeu.",
    ms: 9000,
  },
  {
    id: "toolu_four_vert",
    type: "agent-vert",
    color: "green",
    description: "Tests Vitest du magasin",
    tools: [
      { name: "Bash", input: { command: "npm test", description: "Run unit tests" }, result: "Tests  92 passed (92)" },
    ],
    answer: "Tous les tests passent.",
    ms: 6000,
  },
  {
    id: "toolu_four_orange",
    type: "agent-orange",
    color: "orange",
    description: "Correction du libellé de fin de tour",
    tools: [
      { name: "Read", input: { file_path: "C:\\projet\\src\\lib\\render\\tools.ts" }, result: "1\t// …" },
      { name: "Edit", input: { file_path: "C:\\projet\\src\\lib\\render\\tools.ts", old_string: "a", new_string: "b" }, result: "String to replace not found in file.", isError: true },
    ],
    answer: "Échec : la chaîne à remplacer est introuvable.",
    ms: 7000,
    fail: true,
  },
  {
    id: "toolu_four_violet",
    type: "agent-violet",
    color: "purple",
    description: "Relecture de sécurité du parseur de transcripts, avec une description longue qui déborde",
    tools: [
      { name: "Glob", input: { pattern: "src-tauri/src/sessions/**/*.rs" }, result: "transcript.rs\nmod.rs" },
      { name: "Read", input: { file_path: "C:\\projet\\src-tauri\\src\\sessions\\transcript.rs" }, result: "1\t//" },
      { name: "Bash", input: { command: "cargo test transcripts", description: "Run transcript tests" }, result: "ok" },
    ],
    answer: "Rien d'exploitable.",
    ms: 14000,
  },
  {
    id: "toolu_four_inconnu",
    type: "general-purpose",
    color: null,
    description: "Agent sans couleur (gris)",
    tools: [{ name: "WebSearch", input: { query: "tauri channel ordering" }, result: "3 results" }],
    answer: "Ordre garanti par le canal.",
    ms: 11000,
  },
];

/**
 * Événements du scénario `four`, horodatés depuis `base` (ms epoch) ;
 * `scale` étire le temps (2 = deux fois plus lent).
 */
export function fourAgents(base: number, scale = 1): Timed[] {
  const out: { at: number; body: EventBody }[] = [];
  const add = (at: number, body: EventBody) => out.push({ at: Math.round(at * scale), body });
  add(0, { kind: "status", state: "Requesting" });
  add(50, { kind: "user_message", thread: "main", uuid: null, text: "/orchestrateur audit en 4 agents", origin: "Typed" });
  add(100, { kind: "status", state: "Streaming" });
  AGENTS.forEach((a, k) => {
    const start = 300 + k * 250;
    const agent = (at: number, patch: Partial<AgentEvent>) => {
      add(at, {
        kind: "agent",
        tool_use_id: a.id,
        task_id: `task_${a.id}`,
        agent_type: a.type,
        color: a.color,
        description: a.description,
        status: "Running",
        background: true,
        started_at: base + Math.round(start * scale),
        ended_at: null,
        last_tool: null,
        tokens: null,
        tool_uses: null,
        summary: null,
        ...patch,
      } as EventBody);
    };
    add(start, {
      kind: "block_done",
      thread: "main",
      msg: "msg_four_main",
      index: k,
      block: { type: "ToolUse", tool_use_id: a.id, name: "Agent", input: { description: a.description, subagent_type: a.type } },
    });
    agent(start, { status: "Starting" });
    agent(start + 100, {});
    add(start + 150, { kind: "user_message", thread: a.id, uuid: null, text: `Mission : ${a.description}.`, origin: "Typed" });
    const step = (a.ms - 1000) / (a.tools.length + 1);
    let tok = 18000 + k * 1700;
    a.tools.forEach((tool, j) => {
      const at = start + 300 + j * step;
      const tid = `${a.id}_t${j}`;
      add(at, { kind: "block_done", thread: a.id, msg: `msg_${a.id}_${j}`, index: 0, block: { type: "ToolUse", tool_use_id: tid, name: tool.name, input: tool.input as never } });
      tok += 4200 + j * 900;
      agent(at + 10, { last_tool: tool.name, tokens: tok, tool_uses: j });
      add(at + step * 0.7, {
        kind: "tool_result",
        thread: a.id,
        tool_use_id: tid,
        content: [{ type: "Text", text: tool.result }],
        is_error: tool.isError ?? false,
        meta: null,
      });
      agent(at + step * 0.7 + 10, { last_tool: tool.name, tokens: tok + 600, tool_uses: j + 1 });
    });
    const end = start + a.ms;
    add(end - 200, { kind: "block_done", thread: a.id, msg: `msg_${a.id}_end`, index: 0, block: { type: "Text", text: a.answer } });
    agent(end, {
      status: a.fail ? "Failed" : "Completed",
      ended_at: base + Math.round(end * scale),
      last_tool: a.tools[a.tools.length - 1]?.name ?? null,
      tokens: tok + 1500,
      tool_uses: a.tools.length,
      summary: a.answer,
    });
  });
  const lastEnd = Math.max(...AGENTS.map((a, k) => 300 + k * 250 + a.ms));
  add(lastEnd + 300, { kind: "status", state: "Idle" });
  out.sort((x, y) => x.at - y.at);
  return out.map((x, seq) => ({
    at: x.at,
    ev: { seq: seq + 1000, t: base + x.at, session: "demo", ...x.body } as CoreEvent,
  }));
}

/**
 * Événements du scénario `sort` (tri de la colonne), horodatés depuis
 * `base` ; `scale` étire le temps.
 */
export function sortAgents(base: number, scale = 1): Timed[] {
  const out: { at: number; body: EventBody }[] = [];
  const add = (at: number, body: EventBody) => out.push({ at: Math.round(at * scale), body });
  const specs = [
    { id: "toolu_sort_bleu", type: "agent-bleu", color: "blue", description: "Lecture du réducteur" },
    { id: "toolu_sort_vert", type: "agent-vert", color: "green", description: "Choix du format du rapport" },
    { id: "toolu_sort_rouge", type: "agent-rouge", color: "red", description: "Tests Vitest" },
    { id: "toolu_sort_orange", type: "agent-orange", color: "orange", description: "Correction d'un libellé" },
  ];
  const agent = (k: number, at: number, patch: Partial<AgentEvent>) => {
    const a = specs[k];
    add(at, {
      kind: "agent",
      tool_use_id: a.id,
      task_id: `task_${a.id}`,
      agent_type: a.type,
      color: a.color,
      description: a.description,
      status: "Running",
      background: true,
      started_at: base + Math.round((100 + k * 50) * scale),
      ended_at: null,
      last_tool: null,
      tokens: null,
      tool_uses: null,
      summary: null,
      ...patch,
    } as EventBody);
  };
  add(0, { kind: "status", state: "Streaming" });
  specs.forEach((_, k) => agent(k, 100 + k * 50, { last_tool: "Read" }));
  // Vert pose une question dans son fil : la carte de question s'ouvre.
  add(800, {
    kind: "block_done",
    thread: specs[1].id,
    msg: "msg_sort_ask",
    index: 0,
    block: {
      type: "ToolUse",
      tool_use_id: "toolu_sort_ask",
      name: "AskUserQuestion",
      input: { questions: [{ question: "Quel format ?", header: "Format", options: [], multiSelect: false }] },
    },
  });
  add(800, {
    kind: "question",
    request_id: "req_sort_q",
    tool_use_id: "toolu_sort_ask",
    thread: specs[1].id,
    questions: [
      {
        question: "Quel format ?",
        header: "Format",
        options: [
          { label: "Court", description: "Une ligne" },
          { label: "Long", description: "Un paragraphe" },
        ],
        multi_select: false,
      },
    ],
    can_skip: true,
  });
  agent(2, 1500, { status: "Completed", ended_at: base + Math.round(1500 * scale), tool_uses: 1, summary: "ok" });
  agent(3, 2500, { status: "Failed", ended_at: base + Math.round(2500 * scale), tool_uses: 2, summary: "échec" });
  out.sort((x, y) => x.at - y.at);
  return out.map((x, seq) => ({
    at: x.at,
    ev: { seq: seq + 1000, t: base + x.at, session: "demo", ...x.body } as CoreEvent,
  }));
}
