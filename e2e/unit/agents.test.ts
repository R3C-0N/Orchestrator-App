// Colonne des agents (jalon J9) : modèle d'affichage et magasin, sur la
// fixture `t3` (deux agents, bleu et rouge) et sur le scénario de démo à
// 4 agents + 1 agent gris.
import { describe, expect, it } from "vitest";
import t3 from "../../fixtures/ui-events/t3.json";
import type { AgentEvent } from "../../src/lib/core/bindings/AgentEvent";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import {
  RUNNING_GLYPH,
  agentDetail,
  agentElapsed,
  agentGlyph,
  agentStats,
  doneHeader,
  groupAgents,
  isLive,
  threadLines,
  waitingIds,
} from "../../src/lib/components/agents/agentView";
import { fourAgents, retime, sortAgents } from "../../src/lib/components/agents/demo/replay";
import { AGENT_NEUTRAL, agentColor } from "../../src/lib/render/colors";
import { SessionStore } from "../../src/lib/state/session.svelte";

const T3 = t3 as unknown as CoreEvent[];

function store(): SessionStore {
  return new SessionStore(() => {
    throw new Error("planificateur non attendu");
  });
}

function agent(p: Partial<AgentEvent> = {}): AgentEvent {
  return {
    tool_use_id: "toolu_x",
    task_id: null,
    agent_type: "agent-bleu",
    color: "blue",
    description: "Ping bleu agent",
    status: "Running",
    background: true,
    started_at: 1_000,
    ended_at: null,
    last_tool: null,
    tokens: null,
    tool_uses: null,
    summary: null,
    ...p,
  };
}

describe("modèle d'une ligne d'agent", () => {
  it("reprend « 6s · ↓ 38.9k tokens » du panneau du TUI (05)", () => {
    expect(agentStats(agent({ tokens: 38_900 }), 7_000)).toBe("6s · ↓ 38.9k tokens");
    expect(agentStats(agent(), 7_000)).toBe("6s");
    expect(agentStats(agent({ started_at: null, tokens: 512 }), 7_000)).toBe("↓ 512 tokens");
  });

  it("fait avancer la durée d'un agent en cours puis la fige à ended_at", () => {
    const a = agent();
    expect(agentElapsed(a, 4_000)).toBe(3_000);
    expect(agentElapsed(a, 9_000)).toBe(8_000);
    const done = agent({ status: "Completed", ended_at: 43_000 });
    expect(agentElapsed(done, 10_000)).toBe(42_000);
    expect(agentElapsed(done, 99_000)).toBe(42_000);
    // Fini sans heure de fin : pas de durée inventée.
    expect(agentElapsed(agent({ status: "Stopped" }), 99_000)).toBeNull();
  });

  it("donne l'état par le glyphe : ◯ gris en cours, ● vert / rouge / gris une fois fini", () => {
    expect(agentGlyph("Starting")).toEqual({ glyph: RUNNING_GLYPH, tone: "dim" });
    expect(agentGlyph("Running")).toEqual({ glyph: "◯", tone: "dim" });
    expect(agentGlyph("Completed")).toEqual({ glyph: "●", tone: "ok" });
    expect(agentGlyph("Failed")).toEqual({ glyph: "●", tone: "err" });
    expect(agentGlyph("Stopped")).toEqual({ glyph: "●", tone: "dim" });
    expect(isLive({ status: "Running" })).toBe(true);
    expect(isLive({ status: "Completed" })).toBe(false);
  });

  it("montre l'outil en cours puis l'état final sous ⎿", () => {
    expect(agentDetail(agent({ status: "Starting" })).text).toBe("Starting…");
    expect(agentDetail(agent()).text).toBe("Running…");
    expect(agentDetail(agent({ last_tool: "Read" })).text).toBe("Read…");
    // Même nom que dans le fil : Edit → Update, Grep → Search.
    expect(agentDetail(agent({ last_tool: "Edit" })).text).toBe("Update…");
    expect(agentDetail(agent({ status: "Completed", tool_uses: 1 })).text).toBe("Done · 1 tool use");
    expect(agentDetail(agent({ status: "Completed", tool_uses: 0 })).text).toBe("Done · 0 tool uses");
    expect(agentDetail(agent({ status: "Failed", tool_uses: 2 }))).toEqual({ text: "Failed · 2 tool uses", tone: "err" });
  });

  it("colore le nom d'après color, gris pour un agent inconnu", () => {
    expect(agentColor("blue", "agent-bleu")).toBe("#6A9BCC");
    expect(agentColor("red", " agent-rouge ")).toBe("#DC2626");
    expect(agentColor(null, "general-purpose")).toBe(AGENT_NEUTRAL);
  });
});

describe("t3 rejoué dans le magasin", () => {
  it("donne 2 agents, bleu puis rouge, terminés, avec last_tool et tokens", () => {
    const s = store();
    s.apply(T3);
    expect(s.gaps).toBe(0);
    expect(s.failed).toBe(0);
    expect(s.agents.map((a) => [a.agent_type, a.color, a.status])).toEqual([
      ["agent-bleu", "blue", "Completed"],
      ["agent-rouge", "red", "Completed"],
    ]);
    const [bleu, rouge] = s.agents;
    expect(bleu.last_tool).toBe("Read");
    expect(agentStats(bleu, 0)).toBe("0s · ↓ 35.6k tokens");
    expect(agentDetail(bleu).text).toBe("Done · 1 tool use");
    expect(agentStats(rouge, 0)).toBe("0s · ↓ 24.0k tokens");
    expect(agentColor(bleu.color, bleu.agent_type)).toBe("#6A9BCC");
    expect(agentColor(rouge.color, rouge.agent_type)).toBe("#DC2626");
  });

  it("étiré par la démo, la durée avance pendant l'exécution puis se fige", () => {
    const base = 1_800_000_000_000;
    const timed = retime(T3, base, 20);
    const s = store();
    // Jusqu'au `Running` de l'agent bleu avec `last_tool: Read` (seq 12).
    s.apply(timed.filter((x) => x.ev.seq <= 12).map((x) => x.ev));
    const bleu = s.agents[0];
    expect(bleu.status).toBe("Running");
    expect(agentDetail(bleu).text).toBe("Read…");
    const at12 = timed.find((x) => x.ev.seq === 12)!.ev.t;
    expect(agentElapsed(bleu, at12)).toBe((340 - 160) * 20);
    expect(agentElapsed(bleu, at12 + 2_000)).toBe((340 - 160) * 20 + 2_000);
    s.apply(timed.filter((x) => x.ev.seq > 12).map((x) => x.ev));
    const fin = s.agents[0];
    expect(fin.status).toBe("Completed");
    // (520 − 160) × 20 = 7,2 s, quel que soit `now`.
    expect(agentStats(fin, base + 1e9)).toBe("7s · ↓ 35.6k tokens");
  });

  it("garde le fil de chaque agent, et threadsRev avance à chaque lot qui le touche", () => {
    const s = store();
    const rev0 = s.threadsRev;
    s.apply(T3.filter((e) => e.seq <= 4));
    expect(s.threadsRev).toBe(rev0);
    s.apply(T3.filter((e) => e.seq === 5));
    expect(s.threadsRev).toBe(rev0 + 1);
    s.apply(T3.filter((e) => e.seq > 5));
    const bleu = threadLines(s.threads.get("toolu_01Fvzn2k9s2bVaPNYQBYGJJG") ?? [], null);
    expect(bleu.map((l) => l.kind)).toEqual(["prompt", "tool", "text"]);
    expect(bleu[0]).toMatchObject({ text: "lis a.txt et réponds en 3 mots" });
    expect(bleu[1]).toMatchObject({ title: "Read", isError: false });
    expect(bleu[2]).toMatchObject({ text: "Bonjour fichier", done: true });
    const rouge = threadLines(s.threads.get("toolu_01SDmyAGx26yC6ZqWR52vuwR") ?? [], null);
    expect(rouge.at(-1)).toMatchObject({ kind: "text" });
  });

  it("threadLines rend des objets neufs à chaque appel (le magasin mute ses lignes)", () => {
    const s = store();
    s.apply(T3);
    const rows = s.threads.get("toolu_01Fvzn2k9s2bVaPNYQBYGJJG")!;
    expect(threadLines(rows, null)[0]).not.toBe(threadLines(rows, null)[0]);
  });
});

describe("scénario de démo à 4 agents + 1 gris", () => {
  it("est conforme (pas de trou, aucun rejet) et finit avec un échec visible", () => {
    const s = store();
    s.apply(fourAgents(1_000_000).map((x) => x.ev));
    expect(s.gaps).toBe(0);
    expect(s.failed).toBe(0);
    expect(s.agents).toHaveLength(5);
    expect(s.agents.map((a) => a.status)).toEqual(["Completed", "Completed", "Failed", "Completed", "Completed"]);
    expect(agentColor(s.agents[4].color, s.agents[4].agent_type)).toBe(AGENT_NEUTRAL);
    const orange = threadLines(s.threads.get("toolu_four_orange")!, "C:\\projet");
    expect(orange.find((l) => l.kind === "tool" && l.isError)).toMatchObject({ title: "Update", arg: "src\\lib\\render\\tools.ts" });
  });
});

describe("tri de la colonne : en attente, en cours, terminés", () => {
  const ids = (xs: AgentEvent[]) => xs.map((a) => a.tool_use_id);
  const a1 = agent({ tool_use_id: "a1" });
  const a2 = agent({ tool_use_id: "a2", status: "Completed" });
  const a3 = agent({ tool_use_id: "a3" });
  const a4 = agent({ tool_use_id: "a4", status: "Failed" });
  const a5 = agent({ tool_use_id: "a5", status: "Starting" });
  const a6 = agent({ tool_use_id: "a6", status: "Stopped" });
  const all = [a1, a2, a3, a4, a5, a6];

  it("sans question : en cours puis terminés (échec et arrêt compris), dans l'ordre de lancement", () => {
    const g = groupAgents(all, new Set());
    expect(ids(g.waiting)).toEqual([]);
    expect(ids(g.running)).toEqual(["a1", "a3", "a5"]);
    expect(ids(g.over)).toEqual(["a2", "a4", "a6"]);
  });

  it("un agent qui attend une réponse passe en tête, les autres gardent leur ordre", () => {
    const g = groupAgents(all, new Set(["a5"]));
    expect(ids(g.waiting)).toEqual(["a5"]);
    expect(ids(g.running)).toEqual(["a1", "a3"]);
    expect(ids(g.over)).toEqual(["a2", "a4", "a6"]);
  });

  it("un agent terminé n'attend plus rien, même si une question porte son id", () => {
    const g = groupAgents(all, new Set(["a2"]));
    expect(ids(g.waiting)).toEqual([]);
    expect(ids(g.over)).toEqual(["a2", "a4", "a6"]);
  });

  it("change de groupe quand son état change, sans déranger l'ordre des autres", () => {
    const after = all.map((a) => (a.tool_use_id === "a1" ? { ...a, status: "Completed" as const } : a));
    const g = groupAgents(after, new Set());
    expect(ids(g.running)).toEqual(["a3", "a5"]);
    expect(ids(g.over)).toEqual(["a1", "a2", "a4", "a6"]);
  });

  it("la question du fil principal ne désigne aucun agent", () => {
    expect([...waitingIds([])]).toEqual([]);
    expect([...waitingIds([{ thread: "main" }])]).toEqual([]);
    expect([...waitingIds([{ thread: "main" }, { thread: "toolu_x" }, { thread: "toolu_y" }])]).toEqual(["toolu_x", "toolu_y"]);
  });

  it("ligne ⎿ « ☐ Waiting for your answer » tant qu'il attend et qu'il tourne", () => {
    expect(agentDetail(agent({ last_tool: "Read" }), true)).toEqual({ text: "☐ Waiting for your answer", tone: "ask" });
    expect(agentDetail(agent({ status: "Completed", tool_uses: 1 }), true).text).toBe("Done · 1 tool use");
  });

  it("en-tête du panneau : ▸ replié, ▾ ouvert", () => {
    expect(doneHeader(3, false)).toEqual({ glyph: "▸", text: "3 done" });
    expect(doneHeader(1, true)).toEqual({ glyph: "▾", text: "1 done" });
  });

  it("scénario sort : vert en attente, bleu en cours, rouge puis orange terminés", () => {
    const s = store();
    s.apply(sortAgents(1_000_000).map((x) => x.ev));
    expect(s.gaps).toBe(0);
    expect(s.failed).toBe(0);
    expect(s.question?.thread).toBe("toolu_sort_vert");
    const g = groupAgents(s.agents, waitingIds(s.questions));
    expect(ids(g.waiting)).toEqual(["toolu_sort_vert"]);
    expect(ids(g.running)).toEqual(["toolu_sort_bleu"]);
    expect(ids(g.over)).toEqual(["toolu_sort_rouge", "toolu_sort_orange"]);
    // La question une fois fermée, vert reprend sa place dans les agents en cours.
    s.apply([{ seq: 5000, t: 1_000_000, session: "demo", kind: "question_closed", request_id: "req_sort_q", reason: "Answered" } as CoreEvent]);
    expect(ids(groupAgents(s.agents, waitingIds(s.questions)).running)).toEqual(["toolu_sort_bleu", "toolu_sort_vert"]);
  });
});

describe("questions en file (main et sous-agents en parallèle)", () => {
  const q = (seq: number, id: string, thread: string) =>
    ({ seq, t: 0, session: "s", kind: "question", request_id: id, tool_use_id: `tu_${id}`, thread, questions: [], can_skip: true }) as CoreEvent;
  const closed = (seq: number, id: string) =>
    ({ seq, t: 0, session: "s", kind: "question_closed", request_id: id, reason: "Answered" }) as CoreEvent;

  it("affiche la plus ancienne, compte toutes les questions en attente, ferme par request_id", () => {
    const s = store();
    s.apply([q(0, "r_main", "main"), q(1, "r_a", "toolu_a"), q(2, "r_b", "toolu_b")]);
    expect(s.question?.requestId).toBe("r_main");
    expect([...waitingIds(s.questions)]).toEqual(["toolu_a", "toolu_b"]);
    // Fermer une question qui n'est pas affichée la retire quand même.
    s.apply([closed(3, "r_b")]);
    expect(s.questions.map((x) => x.requestId)).toEqual(["r_main", "r_a"]);
    s.apply([closed(4, "r_main")]);
    expect(s.question?.requestId).toBe("r_a");
    expect([...waitingIds(s.questions)]).toEqual(["toolu_a"]);
    // Même request_id : remplacée sur place, pas dupliquée.
    s.putQuestion({ ...s.question! });
    expect(s.questions).toHaveLength(1);
    s.apply([closed(5, "r_a")]);
    expect(s.question).toBeNull();
  });
});

describe("file de questions : fermeture et retrait par request_id", () => {
  const ask = (seq: number, id: string, thread: string) =>
    ({ seq, t: 0, session: "s", kind: "question", request_id: id, tool_use_id: `tu_${id}`, thread, questions: [], can_skip: true }) as CoreEvent;

  it("removeQuestion vise la question demandée, pas la question affichée", () => {
    const s = store();
    s.apply([ask(0, "r1", "main"), ask(1, "r2", "toolu_a")]);
    expect(s.question?.requestId).toBe("r1");
    // Retrait de la seconde (non affichée) : la première reste affichée.
    s.removeQuestion("r2");
    expect(s.questions.map((q) => q.requestId)).toEqual(["r1"]);
    expect([...waitingIds(s.questions)]).toEqual([]);
    // Id inconnu ou déjà retiré : rien ne bouge.
    s.removeQuestion("r2");
    s.removeQuestion("inconnu");
    expect(s.questions.map((q) => q.requestId)).toEqual(["r1"]);
  });

  it("une question arrivée entre-temps n'est pas retirée à la place de la bonne", () => {
    const s = store();
    s.apply([ask(0, "r1", "main")]);
    const shown = s.question!;
    // Une seconde question arrive avant la fermeture de la première.
    s.apply([ask(1, "r2", "toolu_a")]);
    s.apply([{ seq: 2, t: 0, session: "s", kind: "question_closed", request_id: shown.requestId, reason: "Answered" } as CoreEvent]);
    expect(s.question?.requestId).toBe("r2");
    expect([...waitingIds(s.questions)]).toEqual(["toolu_a"]);
    // putQuestion après une erreur d'envoi : remplacée sur place, ordre gardé.
    s.apply([ask(3, "r3", "main")]);
    const again = { ...s.question! };
    s.putQuestion(again);
    expect(s.questions.map((q) => q.requestId)).toEqual(["r2", "r3"]);
    expect(s.question).toBe(again);
  });

  it("store.question n'a plus d'écriture", () => {
    const s = store();
    expect(() => {
      (s as unknown as { question: null }).question = null;
    }).toThrow(TypeError);
  });
});
