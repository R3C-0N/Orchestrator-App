// Diff d'un `Edit` (plan 1.7, capture `04`) :
// « ⎿  Added 3 lines, removed 2 lines », puis les lignes numérotées, avec
// 3 lignes de contexte avant et après ; suppressions numérotées dans
// l'ancien fichier, ajouts dans le nouveau.
//
// Source, par ordre de préférence :
// 1. `tool_use_result.structuredPatch` (forme relevée dans `t13`), pour
//    Edit, MultiEdit et Write (écrasement d'un fichier existant) ;
// 2. `tool_use_result.originalFile` + les remplacements de l'entrée
//    (`old_string`/`new_string`, ou `edits[]` d'un MultiEdit) : diff
//    recalculé avec jsdiff, 3 lignes de contexte, vrais numéros ;
// 3. les remplacements seuls : on ne connaît pas leur place dans le
//    fichier. Affichage choisi : les lignes `-` et `+` **sans numéro**
//    (colonne des numéros laissée vide), plutôt que des numéros relatifs
//    qui laisseraient croire à une position dans le fichier.
// Les fins de ligne CRLF sont ramenées à LF avant tout calcul.

import { diffWordsWithSpace, structuredPatch } from "diff";
import { escapeHtml, highlightLines, languageForPath } from "./highlight";

export type Hunk = { oldStart: number; oldLines: number; newStart: number; newLines: number; lines: string[] };

export type DiffLine =
  /** `no` : numéro dans l'ancien (`-`) ou le nouveau fichier ; `null` si inconnu. */
  | { kind: " " | "-" | "+"; no: number | null; html: string }
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

const lf = (x: string) => x.replace(/\r\n?/g, "\n");

type Repl = { old: string; new: string; all: boolean };

/** Remplacements de l'entrée : `edits[]` (MultiEdit) ou `old_string`/`new_string`. */
function replacements(inp: Obj, m: Obj): Repl[] {
  if (Array.isArray(inp.edits)) {
    return inp.edits.filter(isObj).map((e) => ({
      old: lf(str(e.old_string) ?? ""),
      new: lf(str(e.new_string) ?? ""),
      all: e.replace_all === true,
    }));
  }
  const old = str(m.oldString) ?? str(inp.old_string);
  const nw = str(m.newString) ?? str(inp.new_string);
  if (old === null && nw === null) return [];
  return [{ old: lf(old ?? ""), new: lf(nw ?? ""), all: inp.replace_all === true || m.replaceAll === true }];
}

export type Patch = { hunks: Hunk[]; numbered: boolean };

/** Hunks d'un Edit, MultiEdit ou Write, d'après son entrée et le `tool_use_result` (`meta`). */
export function editPatch(input: unknown, meta: unknown): Patch {
  const inp = isObj(input) ? input : {};
  const m = isObj(meta) ? meta : {};
  const patch = hunksFrom(m.structuredPatch);
  if (patch && patch.length > 0) {
    return { hunks: patch.map((h) => ({ ...h, lines: h.lines.map((l) => l.replace(/\r$/, "")) })), numbered: true };
  }
  const repl = replacements(inp, m);
  const originalRaw = str(m.originalFile);
  if (originalRaw !== null && repl.length > 0) {
    const original = lf(originalRaw);
    let after = original;
    let ok = true;
    for (const r of repl) {
      if (!r.old || !after.includes(r.old)) {
        ok = false;
        break;
      }
      after = r.all ? after.split(r.old).join(r.new) : after.replace(r.old, () => r.new);
    }
    if (ok) return { hunks: structuredPatch("a", "b", original, after, "", "", { context: 3 }).hunks, numbered: true };
  }
  const hunks: Hunk[] = [];
  for (const r of repl) {
    if (!r.old && !r.new) continue;
    const a = r.old === "" || r.old.endsWith("\n") ? r.old : r.old + "\n";
    const b = r.new === "" || r.new.endsWith("\n") ? r.new : r.new + "\n";
    hunks.push(...structuredPatch("a", "b", a, b, "", "", { context: 3 }).hunks);
  }
  return { hunks, numbered: false };
}

/** Hunks seuls (voir `editPatch`). */
export function editHunks(input: unknown, meta: unknown): Hunk[] {
  return editPatch(input, meta).hunks;
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
export function buildDiff(hunks: Hunk[], path: string | null, numbered = true): EditDiff {
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
          lines.push({ kind: "-", no: numbered ? o : null, html: markRanges(escapeHtml(d), r) });
          o++;
          removed++;
        });
        adds.forEach((a, q) => {
          const r = q < dels.length ? wordRanges(dels[q], a).add : [];
          lines.push({ kind: "+", no: numbered ? n : null, html: markRanges(colored[ci++] ?? escapeHtml(a), r) });
          n++;
          added++;
        });
        i = e - 1;
      } else if (k === "+") {
        lines.push({ kind: "+", no: numbered ? n : null, html: colored[ci++] ?? escapeHtml(l.slice(1)) });
        n++;
        added++;
      } else {
        lines.push({ kind: " ", no: numbered ? n : null, html: colored[ci++] ?? escapeHtml(l.slice(1)) });
        o++;
        n++;
      }
      maxNo = Math.max(maxNo, o - 1, n - 1);
    }
  });
  // Sans numéros : colonne vide de 3 (la largeur du cas courant, `04`).
  return { added, removed, gutter: numbered ? String(maxNo).length + 1 : 3, lines };
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
  const p = editPatch(row.input, row.result?.meta ?? null);
  const d = buildDiff(p.hunks, path, p.numbered);
  cache.set(row, d);
  return d;
}
