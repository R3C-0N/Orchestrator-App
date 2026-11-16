// Diff d'un `Edit` (plan 1.7, capture `04`) :
// « ⎿  Added 3 lines, removed 2 lines », puis les lignes numérotées, avec
// 3 lignes de contexte avant et après ; suppressions numérotées dans
// l'ancien fichier, ajouts dans le nouveau.
//
// Source, par ordre de préférence :
// 1. `tool_use_result.structuredPatch` (forme relevée dans `t13`) ;
// 2. `tool_use_result.originalFile` + `old_string`/`new_string` : diff
//    recalculé avec jsdiff, 3 lignes de contexte, vrais numéros ;
// 3. `old_string`/`new_string` seuls : diff sans contexte, numéros relatifs.

import { diffWordsWithSpace, structuredPatch } from "diff";
import { escapeHtml, highlightLines, languageForPath } from "./highlight";

export type Hunk = { oldStart: number; oldLines: number; newStart: number; newLines: number; lines: string[] };

export type DiffLine =
  | { kind: " " | "-" | "+"; no: number; html: string }
  | { kind: "gap" };

export type EditDiff = {
  added: number;
  removed: number;
  /** Largeur de la colonne des numéros (en colonnes). */
  gutter: number;
  lines: DiffLine[];
};

type Obj = Record<string, unknown>;

function isObj(x: unknown): x is Obj {
  return typeof x === "object" && x !== null && !Array.isArray(x);
}

function str(x: unknown): string | null {
  return typeof x === "string" ? x : null;
}

function hunksFrom(x: unknown): Hunk[] | null {
  if (!Array.isArray(x)) return null;
  const out: Hunk[] = [];
  for (const h of x) {
    if (!isObj(h) || !Array.isArray(h.lines)) return null;
    out.push({
      oldStart: Number(h.oldStart) || 1,
      oldLines: Number(h.oldLines) || 0,
      newStart: Number(h.newStart) || 1,
      newLines: Number(h.newLines) || 0,
      lines: h.lines.filter((l): l is string => typeof l === "string"),
    });
  }
  return out;
}

/** Hunks d'un Edit, d'après son entrée et le `tool_use_result` (`meta`). */
export function editHunks(input: unknown, meta: unknown): Hunk[] {
  const inp = isObj(input) ? input : {};
  const m = isObj(meta) ? meta : {};
  const patch = hunksFrom(m.structuredPatch);
  if (patch && patch.length > 0) return patch;
  const oldS = str(m.oldString) ?? str(inp.old_string) ?? "";
  const newS = str(m.newString) ?? str(inp.new_string) ?? "";
  const original = str(m.originalFile);
  if (original !== null && oldS && original.includes(oldS)) {
    const replaced =
      inp.replace_all === true || m.replaceAll === true
        ? original.split(oldS).join(newS)
        : original.replace(oldS, () => newS);
    return structuredPatch("a", "b", original, replaced, "", "", { context: 3 }).hunks;
  }
  if (!oldS && !newS) return [];
  const a = oldS.endsWith("\n") ? oldS : oldS + "\n";
  const b = newS.endsWith("\n") ? newS : newS + "\n";
  return structuredPatch("a", "b", a, b, "", "", { context: 3 }).hunks;
}

/**
 * Entoure les plages [a, b) (en caractères du texte) d'un `<span class="dw">`
 * dans un HTML déjà coloré, sans casser l'imbrication des balises.
 */
function markRanges(html: string, ranges: [number, number][]): string {
  if (ranges.length === 0) return html;
  const inRange = (p: number) => ranges.some(([a, b]) => p >= a && p < b);
  let out = "";
  let pos = 0;
  const re = /(<[^>]+>)|(&[a-zA-Z#0-9]+;|[^<&])/g;
  let m: RegExpExecArray | null;
  let open = false;
  while ((m = re.exec(html))) {
    if (m[1]) {
      if (open) {
        out += "</span>";
        open = false;
      }
      out += m[1];
      continue;
    }
    const want = inRange(pos);
    if (want !== open) {
      out += want ? '<span class="dw">' : "</span>";
      open = want;
    }
    out += m[2];
    pos++;
  }
  if (open) out += "</span>";
  return out;
}

/** Plages modifiées de `a` et de `b` (diff par mots, espaces compris). */
function wordRanges(a: string, b: string): { del: [number, number][]; add: [number, number][] } {
  const del: [number, number][] = [];
  const add: [number, number][] = [];
  let pa = 0;
  let pb = 0;
  for (const c of diffWordsWithSpace(a, b)) {
    const n = c.value.length;
    if (c.removed) {
      del.push([pa, pa + n]);
      pa += n;
    } else if (c.added) {
      add.push([pb, pb + n]);
      pb += n;
    } else {
      pa += n;
      pb += n;
    }
  }
  // Ligne réécrite presque entièrement : pas de surlignage (le TUI ne
  // surligne que les petits changements).
  const share = (r: [number, number][], len: number) => r.reduce((s, [x, y]) => s + y - x, 0) / Math.max(1, len);
  if (share(del, a.length) > 0.6 || share(add, b.length) > 0.6) return { del: [], add: [] };
  return { del, add };
}

/** Lignes prêtes à afficher (HTML échappé, coloration du langage du fichier). */
export function buildDiff(hunks: Hunk[], path: string | null): EditDiff {
  const lang = languageForPath(path);
  let added = 0;
  let removed = 0;
  let maxNo = 1;
  const lines: DiffLine[] = [];
  hunks.forEach((h, hi) => {
    if (hi > 0) lines.push({ kind: "gap" });
    // Texte des lignes de contexte et d'ajout coloré d'un bloc (état des
    // chaînes multilignes conservé) ; suppressions sans coloration (`04`).
    const body = h.lines.filter((l) => !l.startsWith("\\"));
    const newText = body.filter((l) => l[0] !== "-").map((l) => l.slice(1));
    const colored = highlightLines(newText.join("\n"), lang);
    let ci = 0;
    let o = h.oldStart;
    let n = h.newStart;
    for (let i = 0; i < body.length; i++) {
      const l = body[i];
      const k = l[0];
      if (k === "-") {
        // Bloc de suppressions suivi d'ajouts : paires pour le surlignage par mots.
        let j = i;
        while (j < body.length && body[j][0] === "-") j++;
        let e = j;
        while (e < body.length && body[e][0] === "+") e++;
        const dels = body.slice(i, j).map((x) => x.slice(1));
        const adds = body.slice(j, e).map((x) => x.slice(1));
        dels.forEach((d, q) => {
          const r = q < adds.length ? wordRanges(d, adds[q]).del : [];
          lines.push({ kind: "-", no: o++, html: markRanges(escapeHtml(d), r) });
          removed++;
        });
        adds.forEach((a, q) => {
          const r = q < dels.length ? wordRanges(dels[q], a).add : [];
          lines.push({ kind: "+", no: n++, html: markRanges(colored[ci++] ?? escapeHtml(a), r) });
          added++;
        });
        i = e - 1;
      } else if (k === "+") {
        lines.push({ kind: "+", no: n++, html: colored[ci++] ?? escapeHtml(l.slice(1)) });
        added++;
      } else {
        lines.push({ kind: " ", no: n, html: colored[ci++] ?? escapeHtml(l.slice(1)) });
        o++;
        n++;
      }
      maxNo = Math.max(maxNo, o - 1, n - 1);
    }
  });
  return { added, removed, gutter: String(maxNo).length + 1, lines };
}

/** « Added 3 lines, removed 2 lines » en HTML (nombres en gras, `04`). */
export function diffSummaryHtml(added: number, removed: number): string {
  const part = (verb: string, n: number) => `${verb} <b>${n}</b> ${n === 1 ? "line" : "lines"}`;
  const parts: string[] = [];
  if (added > 0) parts.push(part("Added", added));
  if (removed > 0) parts.push(part(parts.length ? "removed" : "Removed", removed));
  return parts.length ? parts.join(", ") : "No changes";
}

const cache = new WeakMap<object, EditDiff>();

/**
 * Diff d'une ligne d'appel, mis en cache par objet (les `Row` sont
 * immuables) : remonter une ligne pendant le défilement ne recalcule ni le
 * diff ni la coloration.
 */
export function diffForRow(row: { input: unknown; result: { meta: unknown } | null }, path: string | null): EditDiff {
  const hit = cache.get(row);
  if (hit) return hit;
  const d = buildDiff(editHunks(row.input, row.result?.meta ?? null), path);
  cache.set(row, d);
  return d;
}
