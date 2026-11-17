// Libellés et résultats des appels d'outils (plan 1.7, captures `03b`, `04`,
// `08`, `09`, `09b`, `k1`).

import type { JsonValue } from "../core/bindings/serde_json/JsonValue";
import type { ToolCallRow } from "../state/session.svelte";

type Obj = Record<string, unknown>;

function obj(x: JsonValue | null | unknown): Obj {
  return typeof x === "object" && x !== null && !Array.isArray(x) ? (x as Obj) : {};
}

function s(x: unknown): string | null {
  return typeof x === "string" ? x : null;
}

/** Chemin relatif au dossier de travail quand il est dessous (« Update(config.py) », `04`). */
export function relPath(path: string, cwd: string | null): string {
  if (!cwd) return path;
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "");
  const p = norm(path);
  const c = norm(cwd);
  if (p.toLowerCase().startsWith(c.toLowerCase() + "\\")) return path.slice(c.length + 1);
  return path;
}

/** Nom affiché de l'outil : Edit → « Update » (`04`), Grep et Glob → « Search ». */
export function toolTitle(name: string, input?: JsonValue | null): string {
  switch (name) {
    case "Edit":
    case "MultiEdit":
      return "Update";
    case "Write":
      return "Write";
    case "Grep":
    case "Glob":
      return "Search";
    case "WebFetch":
      return "Fetch";
    case "WebSearch":
      return "Web Search";
    default:
      if (name.startsWith("mcp__")) {
        const [, server, tool] = name.split("__");
        return `${server} - ${tool ?? ""} (MCP)`;
      }
      void input;
      return name;
  }
}

function quoted(k: string, v: unknown): string | null {
  if (v === undefined || v === null || v === "") return null;
  return typeof v === "string" ? `${k}: "${v}"` : `${k}: ${JSON.stringify(v)}`;
}

/** Argument entre parenthèses : chemin, commande, motif… (`03b` : chemin absolu en vue détaillée). */
export function toolArg(row: ToolCallRow, cwd: string | null, verbose: boolean): string {
  const inp = obj(row.input);
  const path = s(inp.file_path) ?? s(inp.notebook_path);
  switch (row.name) {
    case "Read":
    case "Edit":
    case "MultiEdit":
    case "Write":
    case "NotebookRead":
    case "NotebookEdit":
      return path ? (verbose ? path : relPath(path, cwd)) : "";
    case "Bash":
    case "PowerShell": {
      const cmd = s(inp.command) ?? "";
      if (verbose) return cmd;
      const first = cmd.split("\n")[0];
      return first.length < cmd.length ? first + "…" : first;
    }
    case "Grep":
      return [quoted("pattern", inp.pattern), quoted("path", inp.path), quoted("glob", inp.glob)]
        .filter(Boolean)
        .join(", ");
    case "Glob":
      return [quoted("pattern", inp.pattern), quoted("path", inp.path)].filter(Boolean).join(", ");
    case "WebFetch":
      return s(inp.url) ?? "";
    case "WebSearch":
      return s(inp.query) ?? "";
    default: {
      const parts = Object.entries(inp)
        .slice(0, 4)
        .map(([k, v]) => quoted(k, typeof v === "string" && v.length > 80 ? v.slice(0, 80) + "…" : v))
        .filter(Boolean);
      return parts.join(", ");
    }
  }
}

/** Texte d'erreur sans les balises `<tool_use_error>` du CLI, préfixé « Error: » (`09b`). */
export function errorText(text: string): string {
  const t = text.replace(/<\/?tool_use_error>/g, "").trim();
  return /^Error\b/.test(t) ? t : `Error: ${t}`;
}

/** Nombre de lignes d'une sortie de Read (« 1\t… » numérotées par le CLI). */
export function lineCount(text: string): number {
  if (!text) return 0;
  const t = text.endsWith("\n") ? text.slice(0, -1) : text;
  return t.split("\n").length;
}

/** Ligne de résultat d'un appel réussi en vue normale (outils hors Edit/Write et hors groupes). */
export function shortResult(row: ToolCallRow): string {
  const text = row.result?.text ?? "";
  switch (row.name) {
    case "Read":
      return `Read ${lineCount(text)} lines`;
    case "Glob":
    case "Grep": {
      const n = text.trim() ? lineCount(text.trim()) : 0;
      return `Found ${n} ${n === 1 ? "file" : "files"}`;
    }
    default:
      return text;
  }
}

/** Description d'un Bash en cours (« Running ping for 90 requests to localhost », `k1`). */
export function runningLabel(row: ToolCallRow): string {
  const inp = obj(row.input);
  switch (row.name) {
    case "Bash":
    case "PowerShell": {
      const d = s(inp.description);
      const c = s(inp.command) ?? "";
      if (d) return `Running ${d.charAt(0).toLowerCase()}${d.slice(1)}`;
      return `Running ${c.split("\n")[0]}`;
    }
    case "Read":
      return "Reading 1 file…";
    case "Grep":
    case "Glob":
      return "Searching for 1 pattern…";
    default:
      return `Running ${row.name}…`;
  }
}

/** Commande d'un Bash (« ⎿  $ ping -n 90 127.0.0.1 », `08`, `k1`), sinon `null`. */
export function bashCommand(row: ToolCallRow): string | null {
  if (row.name !== "Bash" && row.name !== "PowerShell") return null;
  const c = s(obj(row.input).command);
  return c === null ? null : c.split("\n")[0];
}

/** Lignes au plus `max`, plus « … +N lines » (vue normale). */
export function truncateLines(text: string, max: number): { lines: string[]; more: number } {
  const all = text.replace(/\n+$/, "").split("\n");
  if (all.length <= max) return { lines: all, more: 0 };
  return { lines: all.slice(0, max), more: all.length - max };
}

/** Durée « 6s », « 1m 3s » (fin de tour, agents). */
export function seconds(ms: number): string {
  const sec = Math.max(0, Math.round(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ${sec % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** « 12:20 » (fin de tour, `02`). */
export function hhmm(at: number): string {
  const d = new Date(at);
  // Heure sans zéro initial : « done 8:08 » (`w1`), « done 13:13 » (`03`).
  return `${d.getHours()}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** « 01:13 PM » (vue détaillée, `03b`). */
export function clock12(at: number): string {
  const d = new Date(at);
  const h = d.getHours();
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return `${String(h12).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")} ${h < 12 ? "AM" : "PM"}`;
}

/** « 39.0k » (tokens). */
export function tokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

function esc(x: string): string {
  return x.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Vrai si le `tool_use_result` porte un `structuredPatch` non vide (Write qui écrase, Edit). */
export function hasPatch(meta: unknown): boolean {
  const m = obj(meta);
  return Array.isArray(m.structuredPatch) && m.structuredPatch.length > 0;
}

/**
 * Write d'un nouveau fichier (`type: "create"`, `t13`), d'après la capture
 * `w1`/`w1b` du TUI : « Wrote N lines to <chemin> » (nombre en gras blanc vif,
 * chemin en gras souligné en pointillé comme dans `Write(<chemin>)`), puis
 * les premières lignes numérotées (10 en vue normale, toutes en vue
 * détaillée) : numéro gris foncé (`#666666`) aligné à droite sur
 * (chiffres + 1) colonnes, une espace, la ligne en couleur du texte. Chemin
 * relatif en vue normale, absolu en vue détaillée. HTML échappé.
 */
export function writeSummary(
  input: JsonValue | null | unknown,
  cwd: string | null,
  verbose: boolean,
): { head: string; lines: string[]; more: number } {
  const inp = obj(input);
  const content = (s(inp.content) ?? "").replace(/\r\n?/g, "\n");
  const path = s(inp.file_path) ?? "";
  const n = lineCount(content);
  const all = content === "" ? [] : content.replace(/\n$/, "").split("\n");
  const shown = verbose ? all : all.slice(0, 10);
  const w = String(Math.max(1, all.length)).length + 1;
  return {
    head: `Wrote <span class="b">${n}</span> ${n === 1 ? "line" : "lines"} to <span class="b link">${esc(verbose ? path : relPath(path, cwd))}</span>`,
    lines: shown.map((l, i) => `<span class="ln">${String(i + 1).padStart(w)}</span> ${esc(l)}`),
    more: all.length - shown.length,
  };
}
