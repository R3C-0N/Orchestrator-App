// Échappement du rendu Write : la ligne de résultat passe par `{@html}`
// (`writeSummary`), le libellé `Write(…)` par une interpolation Svelte. Un
// chemin ou un contenu hostile doit rester du texte dans les deux. Le rendu
// du composant dans un vrai DOM est vérifié par Playwright (fidelity.spec.ts).
import { describe, expect, it } from "vitest";
import { toolArg, writeSummary } from "../../src/lib/render/tools";
import type { ToolCallRow } from "../../src/lib/state/session.svelte";

const PATH = "C:\\p\\<img src=x onerror=alert(1)> & co.txt";
const CONTENT = "a < b && c > d\n<script>alert(2)</script>\n";

function row(): ToolCallRow {
  return {
    kind: "tool",
    id: "r1",
    toolUseId: "t1",
    name: "Write",
    input: { file_path: PATH, content: CONTENT },
    result: { text: "File created successfully", isError: false, meta: { type: "create", structuredPatch: [] } },
    at: 0,
  };
}

describe("Write : échappement", () => {
  it("writeSummary échappe <, > et & dans le chemin et le contenu", () => {
    const w = writeSummary({ file_path: PATH, content: CONTENT }, "C:\\p", false);
    expect(w.head).toBe(
      'Wrote <span class="b">2</span> lines to <span class="b link">&lt;img src=x onerror=alert(1)&gt; &amp; co.txt</span>',
    );
    expect(w.lines).toEqual([
      '<span class="ln"> 1</span> a &lt; b &amp;&amp; c &gt; d',
      '<span class="ln"> 2</span> &lt;script&gt;alert(2)&lt;/script&gt;',
    ]);
    // Le libellé reçoit le chemin brut : c'est l'interpolation Svelte qui l'échappe.
    expect(toolArg(row(), "C:\\p", false)).toBe("<img src=x onerror=alert(1)> & co.txt");
  });
});
