// Coloration syntaxique (plan 1.4 : highlight.js, noyau + langages déclarés).
//
// Deux thèmes, posés en CSS dans `app.css` :
// - `.hl-ansi` pour les blocs de code des réponses : couleurs ANSI du jeu
//   Campbell, comme le TUI (`02` : mots-clés bleu foncé, fonctions jaunes,
//   chaînes rouges, fonctions natives bleu clair, nombres verts) ;
// - `.hl-diff` pour les diffs d'Edit : thème en couleurs vraies (`04` :
//   chaînes jaunes, nombres violets, texte `#F8F8F2`).
// La sortie de highlight.js est déjà échappée (aucun HTML du texte source).

import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import plaintext from "highlight.js/lib/languages/plaintext";
import powershell from "highlight.js/lib/languages/powershell";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import scss from "highlight.js/lib/languages/scss";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const LANGS = {
  bash,
  c,
  cpp,
  csharp,
  css,
  diff,
  dockerfile,
  go,
  ini,
  java,
  javascript,
  json,
  kotlin,
  markdown,
  php,
  plaintext,
  powershell,
  python,
  ruby,
  rust,
  scss,
  sql,
  swift,
  typescript,
  xml,
  yaml,
};
for (const [name, lang] of Object.entries(LANGS)) hljs.registerLanguage(name, lang);
hljs.registerAliases(["sh", "shell", "zsh", "console"], { languageName: "bash" });
hljs.registerAliases(["ps", "ps1", "pwsh"], { languageName: "powershell" });
hljs.registerAliases(["toml"], { languageName: "ini" });
hljs.registerAliases(["svelte", "vue"], { languageName: "xml" });

/** Extension de fichier → langage (diffs d'Edit). */
const EXT: Record<string, string> = {
  sh: "bash",
  bash: "bash",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  hpp: "cpp",
  cs: "csharp",
  css: "css",
  go: "go",
  ini: "ini",
  toml: "ini",
  cfg: "ini",
  java: "java",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascript",
  json: "json",
  kt: "kotlin",
  md: "markdown",
  php: "php",
  ps1: "powershell",
  py: "python",
  rb: "ruby",
  rs: "rust",
  scss: "scss",
  sql: "sql",
  swift: "swift",
  ts: "typescript",
  tsx: "typescript",
  html: "xml",
  xml: "xml",
  svelte: "xml",
  vue: "xml",
  yml: "yaml",
  yaml: "yaml",
};

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Langage connu pour un nom donné (bloc ```lang), sinon `null`. */
export function knownLanguage(lang: string | null | undefined): string | null {
  if (!lang) return null;
  const l = lang.trim().toLowerCase().split(/\s+/)[0];
  return hljs.getLanguage(l) ? l : null;
}

/** Langage d'après l'extension d'un chemin, sinon `null`. */
export function languageForPath(path: string | null | undefined): string | null {
  if (!path) return null;
  const m = /\.([A-Za-z0-9]+)$/.exec(path);
  return m ? (EXT[m[1].toLowerCase()] ?? null) : null;
}

/** HTML coloré (échappé) ; texte simplement échappé si le langage est inconnu. */
export function highlight(code: string, lang: string | null): string {
  // Fins de ligne Windows : un retour chariot resterait visible dans le rendu.
  code = code.replace(/\r\n?/g, "\n");
  const l = knownLanguage(lang);
  if (!l) return escapeHtml(code);
  try {
    return hljs.highlight(code, { language: l, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(code);
  }
}

/**
 * Coloration ligne par ligne (diffs) : le texte est coloré d'un bloc, puis
 * découpé en lignes en refermant et rouvrant les `<span>` ouverts, pour que
 * les chaînes sur plusieurs lignes restent colorées.
 */
export function highlightLines(code: string, lang: string | null): string[] {
  const html = highlight(code, lang);
  const out: string[] = [];
  const open: string[] = [];
  let cur = "";
  const re = /(<span[^>]*>)|(<\/span>)|(\n)|([^<\n]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    if (m[1]) {
      open.push(m[1]);
      cur += m[1];
    } else if (m[2]) {
      open.pop();
      cur += m[2];
    } else if (m[3]) {
      out.push(cur + "</span>".repeat(open.length));
      cur = open.join("");
    } else cur += m[4];
  }
  out.push(cur + "</span>".repeat(open.length));
  return out;
}
