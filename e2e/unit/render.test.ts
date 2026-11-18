// Tests du rendu « terminal » (jalon J5) : regroupement des appels, diff
// d'Edit, markdown, couleurs, libellés. Les règles viennent des captures de
// référence du TUI 2.1.325 (plan 1.7).
import { describe, expect, it } from "vitest";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import type { EventBody } from "../../src/lib/core/bindings/EventBody";
import { EventFactory } from "../../src/lib/core/mock";
import { agentColor, agentName } from "../../src/lib/render/colors";
import { buildDiff, diffSummaryHtml, editHunks, editPatch } from "../../src/lib/render/diff";
import { ItemCache, groupLabel, itemIndexOfRow } from "../../src/lib/render/group";
import { highlightLines } from "../../src/lib/render/highlight";
import { renderMarkdown } from "../../src/lib/render/markdown";
import {
  clock12,
  errorText,
  hasPatch,
  hhmm,
  relPath,
  runningLabel,
  seconds,
  toolArg,
  writeSummary,
} from "../../src/lib/render/tools";
import { SessionStore, type ToolCallRow } from "../../src/lib/state/session.svelte";

function store(...bodies: EventBody[]) {
  const f = new EventFactory();
  const s = new SessionStore(() => {
    throw new Error("planificateur non attendu");
  });
  s.apply(bodies.map((b) => f.make(b, 1000)) as CoreEvent[]);
  return s;
}

let n = 0;
function tool(name: string, input: Record<string, unknown>, result?: { text: string; isError?: boolean; meta?: unknown }) {
  const id = `toolu_${n++}`;
  const msg = `msg_${n++}`;
  const out: EventBody[] = [
    { kind: "block_start", thread: "main", msg, index: 0, block: { type: "ToolUse", tool_use_id: id, name } },
    { kind: "block_done", thread: "main", msg, index: 0, block: { type: "ToolUse", tool_use_id: id, name, input } as never },
  ];
  if (result)
    out.push({
      kind: "tool_result",
      thread: "main",
      tool_use_id: id,
      content: [{ type: "Text", text: result.text }],
      is_error: result.isError ?? false,
      meta: (result.meta ?? null) as never,
    });
  return out;
}

function text(t: string): EventBody[] {
  const msg = `msg_${n++}`;
  return [
    { kind: "block_start", thread: "main", msg, index: 0, block: { type: "Text" } },
    { kind: "block_done", thread: "main", msg, index: 0, block: { type: "Text", text: t } },
  ];
}

const user = (t: string): EventBody => ({ kind: "user_message", thread: "main", uuid: null, text: t, origin: "Typed" });
const turn = (subtype = "success"): EventBody => ({
  kind: "turn_result",
  subtype,
  is_error: false,
  terminal_reason: null,
  duration_ms: 6000,
  cost_usd: 0,
  num_turns: 1,
  context_used: null,
  context_window: null,
  permission_denials: [],
  result: null,
});

describe("regroupement des appels (vue normale)", () => {
  it("03 : Read puis Bash → une ligne « Read 1 file, ran 1 shell command »", () => {
    const s = store(user("lis"), ...tool("Read", { file_path: "a" }, { text: "1\tx" }), ...tool("Bash", { command: "seq 1 5" }, { text: "1" }), ...text("ok"), turn());
    const items = new ItemCache().build(s.rows, false);
    expect(items.map((i) => i.kind)).toEqual(["user", "group", "assistant", "turn"]);
    const g = items[1];
    expect(g.kind === "group" && groupLabel(g.rows)).toBe("Read 1 file, ran 1 shell command");
  });

  it("04 : Glob + Read → « Searched for 1 pattern, read 1 file » ; Edit garde sa ligne", () => {
    const s = store(...tool("Glob", { pattern: "config.py" }, { text: "config.py" }), ...tool("Read", { file_path: "c" }, { text: "1\tx" }), ...tool("Edit", { file_path: "c", old_string: "a", new_string: "b" }, { text: "ok" }));
    const items = new ItemCache().build(s.rows, false);
    expect(items.map((i) => i.kind)).toEqual(["group", "tool"]);
    expect(items[0].kind === "group" && groupLabel(items[0].rows)).toBe("Searched for 1 pattern, read 1 file");
  });

  it("09 : Edit en erreur sur sa ligne, puis 2 Bash en erreur regroupés « Ran 2 shell commands »", () => {
    const s = store(
      ...tool("Edit", { file_path: "c" }, { text: "<tool_use_error>String to replace not found in file.</tool_use_error>", isError: true }),
      ...tool("Bash", { command: "mkdir x/y" }, { text: "Exit code 1", isError: true }),
      ...tool("Bash", { command: "exit 3" }, { text: "Exit code 3", isError: true }),
    );
    const items = new ItemCache().build(s.rows, false);
    expect(items.map((i) => i.kind)).toEqual(["tool", "group"]);
    expect(items[1].kind === "group" && groupLabel(items[1].rows)).toBe("Ran 2 shell commands");
  });

  it("vue détaillée : un appel par ligne, aucun groupe", () => {
    const s = store(...tool("Read", { file_path: "a" }, { text: "x" }), ...tool("Bash", { command: "ls" }, { text: "y" }));
    expect(new ItemCache().build(s.rows, true).map((i) => i.kind)).toEqual(["tool", "tool"]);
  });

  it("08 : appel sans résultat puis interruption → « No result recorded », interruption collée, pas de fin de tour", () => {
    const s = store(
      user("ping"),
      ...tool("Bash", { command: "ping -n 90 127.0.0.1" }),
      { kind: "user_message", thread: "main", uuid: null, text: "[Request interrupted by user]", origin: "Interrupted" },
      turn("error_during_execution"),
    );
    const items = new ItemCache().build(s.rows, false);
    expect(items.map((i) => i.kind)).toEqual(["user", "group", "interrupted"]);
    expect(items[1].kind === "group" && items[1].noResult).toBe(true);
    expect(items[2].kind === "interrupted" && items[2].attached).toBe(true);
  });

  it("appel en cours (k1) : pas de « No result » tant que rien ne suit", () => {
    const s = store(user("ping"), ...tool("Bash", { command: "ping", description: "Ping for 90 requests to localhost" }));
    const items = new ItemCache().build(s.rows, false);
    expect(items[1].kind === "group" && items[1].noResult).toBe(false);
    expect(runningLabel(s.rows[1] as ToolCallRow)).toBe("Running ping for 90 requests to localhost");
  });

  /** `tool_use_id` d'un appel construit par `tool()`. */
  const idOf = (ev: EventBody[]) => (ev[0] as { block: { tool_use_id: string } }).block.tool_use_id;

  it("07 : carte ouverte pour cet AskUserQuestion → appel masqué (vues normale et détaillée)", () => {
    const ask = tool("AskUserQuestion", { questions: [{ question: "Q ?", header: "H", options: [], multiSelect: true }] });
    const s = store(user("demande"), ...ask);
    expect(new ItemCache().build(s.rows, false, idOf(ask)).map((i) => i.kind)).toEqual(["user"]);
    expect(new ItemCache().build(s.rows, true, idOf(ask)).map((i) => i.kind)).toEqual(["user"]);
  });

  it("AskUserQuestion sans résultat mais sans carte (historique repris, processus arrêté) → visible", () => {
    const ask = tool("AskUserQuestion", { questions: [] });
    const s = store(user("demande"), ...ask);
    expect(new ItemCache().build(s.rows, false, null).map((i) => i.kind)).toEqual(["user", "tool"]);
    // Carte ouverte pour une autre question : celui-ci reste visible.
    expect(new ItemCache().build(s.rows, false, "toolu_autre").map((i) => i.kind)).toEqual(["user", "tool"]);
  });

  it("07 : AskUserQuestion répondu → réaffiché avec son résultat", () => {
    const ask = tool("AskUserQuestion", { questions: [] }, { text: "Your questions have been answered" });
    const s = store(user("demande"), ...ask, turn());
    expect(new ItemCache().build(s.rows, false, idOf(ask)).map((i) => i.kind)).toEqual(["user", "tool", "turn"]);
  });

  it("AskUserQuestion resté sans réponse dans un tour clos → réaffiché (« No result »)", () => {
    const s = store(user("demande"), ...tool("AskUserQuestion", { questions: [] }), turn());
    const items = new ItemCache().build(s.rows, false, null);
    expect(items.map((i) => i.kind)).toEqual(["user", "tool", "turn"]);
    expect(items[1].kind === "tool" && items[1].noResult).toBe(true);
  });

  it("AskUserQuestion suivi d'une interruption → réaffiché, « No result », interruption collée", () => {
    const s = store(
      user("demande"),
      ...tool("AskUserQuestion", { questions: [] }),
      { kind: "user_message", thread: "main", uuid: null, text: "[Request interrupted by user]", origin: "Interrupted" },
      turn("error_during_execution"),
    );
    const items = new ItemCache().build(s.rows, false, null);
    expect(items.map((i) => i.kind)).toEqual(["user", "tool", "interrupted"]);
    expect(items[1].kind === "tool" && items[1].noResult).toBe(true);
    expect(items[2].kind === "interrupted" && items[2].attached).toBe(true);
  });

  it("Bash sans résultat dans un tour en cours → reste visible (même avec une carte ouverte ailleurs)", () => {
    const s = store(user("ping"), ...tool("Bash", { command: "ping" }));
    expect(new ItemCache().build(s.rows, false, "toolu_x").map((i) => i.kind)).toEqual(["user", "group"]);
    expect(new ItemCache().build(s.rows, true, "toolu_x").map((i) => i.kind)).toEqual(["user", "tool"]);
  });

  it("deux appels dont un seul AskUserQuestion (carte ouverte) : seul celui-là est masqué", () => {
    const ask = tool("AskUserQuestion", { questions: [] });
    const s = store(user("demande"), ...tool("Edit", { file_path: "a", old_string: "x", new_string: "y" }), ...ask);
    const items = new ItemCache().build(s.rows, false, idOf(ask));
    expect(items.map((i) => i.kind)).toEqual(["user", "tool"]);
    expect(items[1].kind === "tool" && items[1].row.name).toBe("Edit");
  });

  it("ligne « plain » (historique omis) : un item à part, dans le fil", () => {
    const s = store(user("a"));
    s.addPlain("Début de l'historique omis (3 lignes)");
    const items = new ItemCache().build(s.rows, false);
    expect(items.map((i) => i.kind)).toEqual(["user", "plain"]);
  });

  it("réutilise les objets des lignes inchangées (Svelte ne redessine que ce qui change)", () => {
    const s = store(user("a"), ...text("b"));
    const c = new ItemCache();
    const a = c.build(s.rows, false);
    s.apply([new EventFactory().make(user("c"))] as CoreEvent[]);
    const b = c.build(s.rows, false);
    expect(b[0]).toBe(a[0]);
    expect(b[1]).toBe(a[1]);
    expect(b).toHaveLength(3);
  });

  it("retrouve l'item d'une ligne source dans l'autre vue (Ctrl+O)", () => {
    const s = store(...tool("Read", { file_path: "a" }, { text: "x" }), ...tool("Bash", { command: "ls" }, { text: "y" }), ...text("fin"));
    const normal = new ItemCache().build(s.rows, false);
    const verbose = new ItemCache().build(s.rows, true);
    expect(itemIndexOfRow(normal, s.rows[1].id)).toBe(0);
    expect(itemIndexOfRow(verbose, s.rows[1].id)).toBe(1);
  });
});

describe("sous-agents", () => {
  const agent = (id: string, status: "Running" | "Completed"): EventBody => ({
    kind: "agent",
    tool_use_id: id,
    task_id: null,
    agent_type: " agent-rouge ",
    color: "red",
    description: "Ping rouge",
    status,
    background: true,
    started_at: 0,
    ended_at: status === "Completed" ? 42_000 : null,
    last_tool: null,
    tokens: null,
    tool_uses: null,
    summary: null,
  });

  it("05/06 : lancement regroupé, « Waiting for N », puis une ligne de fin par agent", () => {
    const s = store(agent("a1", "Running"), agent("a2", "Running"), turn(), agent("a1", "Completed"), agent("a1", "Completed"));
    expect(s.rows.map((r) => r.kind)).toEqual(["agent", "agent", "turn", "agentDone"]);
    const t = s.rows[2];
    expect(t.kind === "turn" && t.waiting).toBe(2);
    expect(new ItemCache().build(s.rows, false).map((i) => i.kind)).toEqual(["agents", "turn", "agentDone"]);
  });

  it("couleur de l'agent : nom anglais, nom français ou `agent-<couleur>` après trim()", () => {
    expect(agentColor("blue")).toBe("#8FB8E8");
    expect(agentColor(null, " agent-rouge ")).toBe("#FF7F86");
    expect(agentColor("vert")).toBe("#9BD17F");
    expect(agentColor("inconnue", "Explore")).toBe("#8C8C88");
    expect(agentName("\u00a0agent-bleu ")).toBe("agent-bleu");
  });
});

describe("diff d'Edit (04)", () => {
  const CONFIG = 'doc\n\nHOST = "localhost"\nPORT = 8080\n\nTIMEOUT = 30\nRETRIES = 3\n\nLOG_LEVEL = "info"\n\n\ndef url():\n    pass\n';
  const input = { file_path: "C:\\p\\config.py", old_string: "TIMEOUT = 30\nRETRIES = 3", new_string: "TIMEOUT = 60\nRETRIES = 5\nBACKOFF = 1.5" };

  function check(hunks: ReturnType<typeof editHunks>) {
    const d = buildDiff(hunks, "config.py");
    expect([d.added, d.removed]).toEqual([3, 2]);
    expect(d.lines.map((l) => (l.kind === "gap" ? "…" : `${l.no}${l.kind}`))).toEqual([
      "3 ", "4 ", "5 ", "6-", "7-", "6+", "7+", "8+", "9 ", "10 ", "11 ",
    ]);
    expect(d.gutter).toBe(3);
    return d;
  }

  it("calculé depuis originalFile : 3 lignes de contexte, anciens numéros pour -, nouveaux pour +", () => {
    check(editHunks(input, { originalFile: CONFIG }));
  });

  it("structuredPatch du CLI (forme de t13) utilisé tel quel", () => {
    const fromOriginal = editHunks(input, { originalFile: CONFIG });
    check(editHunks(input, { structuredPatch: fromOriginal }));
  });

  it("mots changés surlignés, chaînes colorées, aucun HTML du fichier", () => {
    const d = check(editHunks(input, { originalFile: CONFIG }));
    const add60 = d.lines.find((l) => l.kind === "+" && l.no === 6);
    expect(add60 && "html" in add60 && add60.html).toContain('<span class="dw">');
    const host = d.lines[0];
    expect("html" in host && host.html).toContain("hljs-string");
    const evil = buildDiff(editHunks({ old_string: "<b>", new_string: "<i>" }, null), "x.html");
    expect(JSON.stringify(evil.lines)).not.toMatch(/<b>|<i>/);
  });

  it("résumé « Added 3 lines, removed 2 lines », nombres en gras", () => {
    expect(diffSummaryHtml(3, 2)).toBe("Added <b>3</b> lines, removed <b>2</b> lines");
    expect(diffSummaryHtml(1, 0)).toBe("Added <b>1</b> line");
    expect(diffSummaryHtml(0, 2)).toBe("Removed <b>2</b> lines");
  });

  it("coloration par ligne : les span ouverts sont refermés à chaque ligne", () => {
    const lines = highlightLines('x = """a\nb"""', "python");
    expect(lines).toHaveLength(2);
    for (const l of lines) expect((l.match(/<span/g) ?? []).length).toBe((l.match(/<\/span>/g) ?? []).length);
  });
});

describe("markdown sans DOM (Node)", () => {
  // Le rendu avec DOM (DOMPurify actif) est testé dans markdown.dom.test.ts.
  it("sans DOMPurify, aucune balise n'est émise : tout est échappé", () => {
    const h = renderMarkdown('## Titre\n\n<img src=x onerror="alert(1)">');
    expect(h).not.toMatch(/<[a-z]/i);
    expect(h).toContain("img src=x");
  });
});

describe("diff : CRLF, MultiEdit, sans originalFile", () => {
  it("fichier CRLF : aucun retour chariot dans les lignes, mêmes numéros qu'en LF", () => {
    const crlf = 'a\r\nb\r\nc\r\nTIMEOUT = 30\r\nd\r\ne\r\nf\r\n';
    const p = editPatch({ old_string: "TIMEOUT = 30", new_string: "TIMEOUT = 60" }, { originalFile: crlf });
    expect(p.numbered).toBe(true);
    const d = buildDiff(p.hunks, "x.py", p.numbered);
    expect(JSON.stringify(d.lines)).not.toContain("\\r");
    expect(d.lines.map((l) => (l.kind === "gap" ? "…" : `${l.no}${l.kind}`))).toEqual(["1 ", "2 ", "3 ", "4-", "4+", "5 ", "6 ", "7 "]);
    expect(highlightLines("x = 1\r\ny = 2", "python")).toHaveLength(2);
    const fromPatch = editPatch({}, { structuredPatch: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-a\r", "+b\r"] }] });
    expect(fromPatch.hunks[0].lines).toEqual(["-a", "+b"]);
  });

  it("MultiEdit sans structuredPatch : les edits[] sont appliqués à originalFile", () => {
    const original = "un\ndeux\ntrois\nquatre\ncinq\nsix\nsept\nhuit\n";
    const input = {
      file_path: "f.txt",
      edits: [
        { old_string: "deux", new_string: "DEUX" },
        { old_string: "sept", new_string: "SEPT\nsept bis" },
      ],
    };
    const p = editPatch(input, { originalFile: original });
    const d = buildDiff(p.hunks, "f.txt", p.numbered);
    expect([d.added, d.removed]).toEqual([3, 2]);
    expect(diffSummaryHtml(d.added, d.removed)).not.toBe("No changes");
    const changed = d.lines.filter((l) => l.kind === "-" || l.kind === "+").map((l) => ("no" in l ? `${l.no}${l.kind}` : ""));
    expect(changed).toEqual(["2-", "2+", "7-", "7+", "8+"]);
  });

  it("MultiEdit : structuredPatch du CLI prioritaire", () => {
    const patch = [{ oldStart: 4, oldLines: 1, newStart: 4, newLines: 1, lines: ["-x", "+y"] }];
    const p = editPatch({ edits: [{ old_string: "zzz", new_string: "w" }] }, { structuredPatch: patch });
    expect(p).toEqual({ hunks: patch, numbered: true });
  });

  it("MultiEdit sans originalFile : un bloc par edit, sans numéros", () => {
    const p = editPatch({ edits: [{ old_string: "a", new_string: "b" }, { old_string: "c", new_string: "d" }] }, null);
    const d = buildDiff(p.hunks, null, p.numbered);
    expect(p.numbered).toBe(false);
    expect(d.lines.map((l) => (l.kind === "gap" ? "…" : `${l.no ?? "_"}${l.kind}`))).toEqual(["_-", "_+", "…", "_-", "_+"]);
  });

  it("Edit sans originalFile ni structuredPatch : lignes sans numéro (position inconnue)", () => {
    const p = editPatch({ old_string: "x = 1", new_string: "x = 2" }, null);
    expect(p.numbered).toBe(false);
    const d = buildDiff(p.hunks, "a.py", p.numbered);
    expect(d.lines.every((l) => l.kind === "gap" || l.no === null)).toBe(true);
    expect(d.gutter).toBe(3);
  });

  it("Write d'un nouveau fichier : « Wrote N lines to <chemin> », lignes numérotées, HTML échappé", () => {
    const w = writeSummary({ file_path: "C:\\p\\notes2.txt", content: "alpha\r\n<b>beta</b>\r\n" }, "C:\\p", false);
    expect(w.head).toBe('Wrote <span class="b">2</span> lines to <span class="b link">notes2.txt</span>');
    expect(w.lines).toEqual(['<span class="ln"> 1</span> alpha', '<span class="ln"> 2</span> &lt;b&gt;beta&lt;/b&gt;']);
    expect(w.more).toBe(0);
    const long = writeSummary({ file_path: "x", content: Array.from({ length: 25 }, (_, i) => `l${i}`).join("\n") }, null, false);
    expect([long.lines.length, long.more]).toEqual([10, 15]);
    // Capture w1/w1b : 12 lignes, numéros sur 3 colonnes (« ⎿  » puis «   1 alpha », «  10 kappa »),
    // 10 lignes et 2 masquées en vue normale, chemin absolu et 12 lignes en vue détaillée.
    const twelve = { file_path: "C:\\p\\liste.txt", content: "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\nlambda\nmu\n" };
    const n = writeSummary(twelve, "C:\\p", false);
    expect(n.head).toBe('Wrote <span class="b">12</span> lines to <span class="b link">liste.txt</span>');
    expect([n.lines[0], n.lines[9], n.lines.length, n.more]).toEqual([
      '<span class="ln">  1</span> alpha',
      '<span class="ln"> 10</span> kappa',
      10,
      2,
    ]);
    const v = writeSummary(twelve, "C:\\p", true);
    expect(v.head).toContain("C:\\p\\liste.txt");
    expect([v.lines.length, v.more, v.lines[11]]).toEqual([12, 0, '<span class="ln"> 12</span> mu']);
    expect(hasPatch({ type: "create", structuredPatch: [] })).toBe(false);
    expect(hasPatch({ type: "update", structuredPatch: [{ lines: [] }] })).toBe(true);
  });
});

describe("« No result recorded » (08)", () => {
  const agentEv = (status: "Running" | "Completed"): EventBody => ({
    kind: "agent",
    tool_use_id: "ag",
    task_id: null,
    agent_type: "agent-bleu",
    color: "blue",
    description: "d",
    status,
    background: true,
    started_at: 0,
    ended_at: status === "Completed" ? 1000 : null,
    last_tool: null,
    tokens: null,
    tool_uses: null,
    summary: null,
  });

  it("un appel en cours suivi d'une ligne agent ou agentDone n'est pas « sans résultat »", () => {
    const s = store(user("go"), ...tool("Bash", { command: "sleep 9" }), agentEv("Running"), agentEv("Completed"));
    expect(s.rows.map((r) => r.kind)).toEqual(["user", "tool", "agent", "agentDone"]);
    const g = new ItemCache().build(s.rows, false)[1];
    expect(g.kind === "group" && g.noResult).toBe(false);
  });

  it("le même appel devient « sans résultat » une fois le tour clos", () => {
    const s = store(user("go"), ...tool("Bash", { command: "sleep 9" }), agentEv("Running"), turn());
    const g = new ItemCache().build(s.rows, false)[1];
    expect(g.kind === "group" && g.noResult).toBe(true);
  });
});

describe("réflexion (k1)", () => {
  it("« thought for Ns » : durée des blocs Thinking du tour, remise à zéro au message suivant", () => {
    const f = new EventFactory();
    const s = new SessionStore(() => {});
    s.apply([
      f.make({ kind: "block_start", thread: "main", msg: "m", index: 0, block: { type: "Thinking" } }, 1000),
      f.make({ kind: "block_done", thread: "main", msg: "m", index: 0, block: { type: "Thinking", empty: true } }, 2500),
    ] as CoreEvent[]);
    expect(s.thinkingMs).toBe(1500);
    expect(s.rows).toHaveLength(0);
    s.apply([f.make(user("suite"), 3000)] as CoreEvent[]);
    expect(s.thinkingMs).toBe(0);
  });
});

describe("libellés", () => {
  it("chemins relatifs au dossier de travail en vue normale, absolus en vue détaillée", () => {
    expect(relPath("C:\\p\\sub\\config.py", "C:\\p")).toBe("sub\\config.py");
    expect(relPath("D:\\autre\\x", "C:\\p")).toBe("D:\\autre\\x");
    const row = { kind: "tool", id: "t", toolUseId: "u", name: "Edit", input: { file_path: "C:\\p\\config.py" }, result: null, at: 0 } as ToolCallRow;
    expect(toolArg(row, "C:\\p", false)).toBe("config.py");
    expect(toolArg(row, "C:\\p", true)).toBe("C:\\p\\config.py");
  });

  it("erreurs : balises du CLI ôtées, préfixe « Error: »", () => {
    expect(errorText("<tool_use_error>String to replace not found in file.\nString: X</tool_use_error>")).toBe(
      "Error: String to replace not found in file.\nString: X",
    );
    expect(errorText("Exit code 3")).toBe("Error: Exit code 3");
    expect(errorText("Error: déjà")).toBe("Error: déjà");
  });

  it("durées et heures", () => {
    expect(seconds(6000)).toBe("6s");
    expect(seconds(63_000)).toBe("1m 3s");
    const t = new Date(2026, 10, 16, 13, 13).getTime();
    expect(hhmm(t)).toBe("13:13");
    // Sans zéro initial (« done 8:08 », capture w1) ; l'horodatage de la vue détaillée garde le sien.
    const m = new Date(2026, 10, 17, 8, 8).getTime();
    expect([hhmm(m), clock12(m)]).toEqual(["8:08", "08:08 AM"]);
    // Midi et minuit : format 24 h sans zéro de tête. Midi suit les captures (« done 12:20 », 02) ;
    // minuit (« 0:05 ») n'a pas été vérifié contre le vrai TUI.
    expect(hhmm(new Date(2026, 10, 17, 12, 5).getTime())).toBe("12:05");
    expect(hhmm(new Date(2026, 10, 17, 0, 5).getTime())).toBe("0:05");
    expect(clock12(t)).toBe("01:13 PM");
  });
});
