// Tests du rendu « terminal » (jalon J5) : regroupement des appels, diff
// d'Edit, markdown, couleurs, libellés. Les règles viennent des captures de
// référence du TUI 2.1.325 (plan 1.7).
import { describe, expect, it } from "vitest";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import type { EventBody } from "../../src/lib/core/bindings/EventBody";
import { EventFactory } from "../../src/lib/core/mock";
import { agentColor, agentName } from "../../src/lib/render/colors";
import { buildDiff, diffSummaryHtml, editHunks } from "../../src/lib/render/diff";
import { ItemCache, groupLabel, itemIndexOfRow } from "../../src/lib/render/group";
import { highlightLines } from "../../src/lib/render/highlight";
import { markdownCached, renderMarkdown } from "../../src/lib/render/markdown";
import { clock12, errorText, hhmm, relPath, runningLabel, seconds, toolArg } from "../../src/lib/render/tools";
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
    expect(agentColor("blue")).toBe("#6A9BCC");
    expect(agentColor(null, " agent-rouge ")).toBe("#DC2626");
    expect(agentColor("vert")).toBe("#16A34A");
    expect(agentColor("inconnue", "Explore")).toBe("#767676");
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

describe("markdown (02)", () => {
  const md = renderMarkdown("## Titre\n\n### Sous-titre\n\n- un **gras** et `code`\n\n1. Premier\n\n```python\ndef f():\n    return 1\n```");

  it("titres sans #, en bloc gras de même taille", () => {
    expect(md).toContain('<div class="md-h">Titre</div>');
    expect(md).not.toContain("#");
    expect(md).not.toMatch(/<h[1-6]/);
  });

  it("listes avec leur marqueur en texte, code en ligne lavande, bloc de code coloré sans cadre", () => {
    expect(md).toContain('<span class="md-mark">- </span>');
    expect(md).toContain('<span class="md-mark">1. </span>');
    expect(md).toContain('<span class="md-code">code</span>');
    expect(md).toContain('<pre class="md-pre hl-ansi">');
    expect(md).toContain('<span class="hljs-keyword">def</span>');
  });

  it("le HTML du texte est échappé", () => {
    const h = renderMarkdown('<img src=x onerror="alert(1)"> et `<script>`');
    expect(h).not.toContain("<img");
    expect(h).not.toContain("<script>");
    expect(h).toContain("&lt;script&gt;");
  });

  it("cache : même texte, même chaîne", () => {
    expect(markdownCached("a **b**")).toBe(markdownCached("a **b**"));
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
    expect(clock12(t)).toBe("01:13 PM");
  });
});
