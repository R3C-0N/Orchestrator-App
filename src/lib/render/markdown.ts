// Markdown « terminal » (plan 1.7, capture `02`) : marked pour l'analyse,
// un rendu maison qui n'émet que des balises connues et échappe tout le
// texte, puis DOMPurify en défense (le texte vient du modèle).
//
// - titres : gras blanc, même taille, sans `#` ;
// - listes `-` et `1.` affichées telles quelles (marqueur en texte), suite
//   des lignes alignée sous le texte de l'élément ;
// - gras, italique ; code en ligne bleu lavande, sans fond ;
// - blocs de code sans cadre ni fond, colorés (thème ANSI, `.hl-ansi`) ;
// - un bloc par ligne de grille, une ligne vide entre deux blocs.

import DOMPurify from "dompurify";
import { Lexer, type Token, type Tokens } from "marked";
import { escapeHtml, highlight, knownLanguage } from "./highlight";

function inline(tokens: Token[] | undefined): string {
  if (!tokens) return "";
  let s = "";
  for (const t of tokens) s += inlineToken(t);
  return s;
}

function inlineToken(t: Token): string {
  switch (t.type) {
    case "text": {
      const x = t as Tokens.Text;
      return x.tokens ? inline(x.tokens) : escapeHtml(x.text);
    }
    case "escape":
      return escapeHtml((t as Tokens.Escape).text);
    case "strong":
      return `<b>${inline((t as Tokens.Strong).tokens)}</b>`;
    case "em":
      return `<i>${inline((t as Tokens.Em).tokens)}</i>`;
    case "del":
      return `<s>${inline((t as Tokens.Del).tokens)}</s>`;
    case "codespan":
      return `<span class="md-code">${escapeHtml((t as Tokens.Codespan).text)}</span>`;
    case "br":
      return "\n";
    case "link": {
      const l = t as Tokens.Link;
      return `<span class="md-link" title="${escapeHtml(l.href)}">${inline(l.tokens)}</span>`;
    }
    case "image":
      return escapeHtml((t as Tokens.Image).text);
    case "html":
      return escapeHtml((t as Tokens.HTML).text);
    default:
      return "raw" in t && typeof t.raw === "string" ? escapeHtml(t.raw) : "";
  }
}

function plainInline(tokens: Token[] | undefined): string {
  if (!tokens) return "";
  let s = "";
  for (const t of tokens) {
    if ("tokens" in t && Array.isArray(t.tokens)) s += plainInline(t.tokens);
    else if ("text" in t && typeof t.text === "string") s += t.text;
  }
  return s;
}

function list(l: Tokens.List): string {
  const start = typeof l.start === "number" ? l.start : 1;
  let s = `<div class="md-list">`;
  l.items.forEach((item, i) => {
    const mark = l.ordered ? `${start + i}.` : "-";
    const task = item.task ? (item.checked ? "[x] " : "[ ] ") : "";
    s += `<div class="md-li"><span class="md-mark">${mark} </span><div class="md-lic">${task}${blocks(item.tokens, true)}</div></div>`;
  });
  return s + "</div>";
}

/** Table en texte, colonnes alignées, filets `│` `─` comme le TUI. */
function table(t: Tokens.Table): string {
  const head = t.header.map((c) => plainInline(c.tokens));
  const rows = t.rows.map((r) => r.map((c) => plainInline(c.tokens)));
  const w = head.map((h, i) => Math.max(h.length, ...rows.map((r) => (r[i] ?? "").length)));
  const pad = (s: string, i: number) => {
    const n = w[i] - s.length;
    return t.align[i] === "right" ? " ".repeat(n) + s : t.align[i] === "center"
      ? " ".repeat(Math.floor(n / 2)) + s + " ".repeat(Math.ceil(n / 2))
      : s + " ".repeat(n);
  };
  const line = (l: string, m: string, r: string) => l + w.map((n) => "─".repeat(n + 2)).join(m) + r;
  const row = (cells: string[], bold: boolean) =>
    "│" +
    cells
      .map((c, i) => ` ${bold ? `<b>${escapeHtml(pad(c, i))}</b>` : escapeHtml(pad(c, i))} `)
      .join("│") +
    "│";
  const out = [line("┌", "┬", "┐"), row(head, true), line("├", "┼", "┤")];
  for (const r of rows) out.push(row(head.map((_, i) => r[i] ?? ""), false));
  out.push(line("└", "┴", "┘"));
  return `<div class="md-table">${out.join("\n")}</div>`;
}

/**
 * Chaînes à substitutions (f-strings Python, gabarits JS) comme le TUI
 * (`02` : dans `f"Bonjour, {nom} !"`, seul `f"Bonjour, ` est rouge, la
 * suite à partir de la première substitution garde la couleur du texte).
 */
function ansiStrings(html: string): string {
  const OPEN = '<span class="hljs-string">';
  let out = "";
  let i = 0;
  for (;;) {
    const k = html.indexOf(OPEN, i);
    if (k < 0) return out + html.slice(i);
    out += html.slice(i, k);
    // Fin de la chaîne : `</span>` de même profondeur.
    let depth = 1;
    let j = k + OPEN.length;
    const re = /<span[^>]*>|<\/span>/g;
    re.lastIndex = j;
    let m: RegExpExecArray | null;
    while (depth > 0 && (m = re.exec(html))) {
      depth += m[0] === "</span>" ? -1 : 1;
      j = re.lastIndex;
    }
    const inner = html.slice(k + OPEN.length, j - "</span>".length);
    const sub = inner.indexOf('<span class="hljs-subst">');
    if (sub < 0) out += html.slice(k, j);
    else out += OPEN + inner.slice(0, sub) + "</span>" + inner.slice(sub).replace(/<span[^>]*>|<\/span>/g, "");
    i = j;
  }
}

function block(t: Token, tight: boolean): string {
  switch (t.type) {
    case "heading":
      return `<div class="md-h">${inline((t as Tokens.Heading).tokens)}</div>`;
    case "paragraph":
      return `<div class="md-p">${inline((t as Tokens.Paragraph).tokens)}</div>`;
    case "text": {
      const x = t as Tokens.Text;
      return `<div class="md-p">${x.tokens ? inline(x.tokens) : escapeHtml(x.text)}</div>`;
    }
    case "list":
      return list(t as Tokens.List);
    case "code": {
      const c = t as Tokens.Code;
      const lang = knownLanguage(c.lang);
      return `<pre class="md-pre hl-ansi">${ansiStrings(highlight(c.text, lang))}</pre>`;
    }
    case "blockquote":
      return `<div class="md-quote">${blocks((t as Tokens.Blockquote).tokens, tight)}</div>`;
    case "hr":
      return `<div class="md-hr">${"─".repeat(40)}</div>`;
    case "table":
      return table(t as Tokens.Table);
    case "html":
      return `<div class="md-p">${escapeHtml((t as Tokens.HTML).text)}</div>`;
    case "space":
      return "";
    default:
      return "raw" in t && typeof t.raw === "string" ? `<div class="md-p">${escapeHtml(t.raw)}</div>` : "";
  }
}

function blocks(tokens: Token[], tight = false): string {
  let s = "";
  for (const t of tokens) s += block(t, tight);
  return s;
}

function sanitize(html: string): string {
  // DOMPurify a besoin d'un DOM. Sans DOM, on ne fait confiance à aucun
  // HTML : tout est échappé et s'affiche en texte (jamais de balise active).
  return DOMPurify.isSupported ? DOMPurify.sanitize(html) : escapeHtml(html);
}

/** HTML d'un texte markdown (sans cache). */
export function renderMarkdown(text: string): string {
  const tokens = Lexer.lex(text.replace(/\r\n?/g, "\n"), { gfm: true, breaks: false });
  return sanitize(blocks(tokens));
}

const CACHE_MAX = 600;
const cache = new Map<string, string>();

/**
 * HTML d'un bloc terminé, mis en cache par texte (plan 1.7 : « HTML mis en
 * cache »). Le texte d'un bloc terminé ne change plus : même clé, même HTML,
 * aussi après un démontage par la liste virtualisée.
 */
export function markdownCached(text: string): string {
  const hit = cache.get(text);
  if (hit !== undefined) {
    // LRU : remis en fin de Map.
    cache.delete(text);
    cache.set(text, hit);
    return hit;
  }
  const html = renderMarkdown(text);
  cache.set(text, html);
  if (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value as string);
  return html;
}
