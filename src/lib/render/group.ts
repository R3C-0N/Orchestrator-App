// Lignes affichées à partir des `Row` du magasin (plan 1.7).
//
// Vue normale (référence) :
// - les appels de lecture, de recherche et de commande consécutifs sont
//   regroupés en une ligne grise : « Read 1 file, ran 1 shell command »
//   (`03`), « Searched for 1 pattern, read 1 file » (`04`), « Ran 2 shell
//   commands » (`09`, erreurs comprises) ;
// - Edit, Write et les autres outils gardent leur ligne `● Nom(argument)` ;
// - les appels Agent consécutifs donnent « ● N background agents launched » ;
// - un groupe sans résultat suivi d'une interruption devient « No result
//   recorded for N call » (`08`).
// Vue détaillée (Ctrl+O, `03b`, `09b`) : un appel par ligne, sortie complète.
// Dans les deux vues, l'appel `AskUserQuestion` dont la carte est ouverte
// n'est pas affiché : la carte le remplace (`07`). Sans carte (historique
// repris, processus arrêté), il reste visible.
//
// Les objets produits sont réutilisés d'un calcul à l'autre tant que leurs
// lignes sources sont les mêmes objets (`rows` est immuable par ligne) :
// Svelte ne redessine que les lignes qui ont changé.

import type { AgentCallRow, Row, ToolCallRow } from "../state/session.svelte";

export type Item =
  | { kind: "user"; key: string; row: Extract<Row, { kind: "user" }> }
  | { kind: "assistant"; key: string; row: Extract<Row, { kind: "assistant" }> }
  | { kind: "group"; key: string; rows: ToolCallRow[]; noResult: boolean }
  | { kind: "tool"; key: string; row: ToolCallRow; noResult: boolean }
  | { kind: "agents"; key: string; rows: AgentCallRow[] }
  | { kind: "agentDone"; key: string; row: Extract<Row, { kind: "agentDone" }> }
  | { kind: "turn"; key: string; row: Extract<Row, { kind: "turn" }> }
  | { kind: "interrupted"; key: string; row: Extract<Row, { kind: "interrupted" }>; attached: boolean }
  | { kind: "notice"; key: string; row: Extract<Row, { kind: "notice" }> }
  | { kind: "plain"; key: string; row: Extract<Row, { kind: "plain" }> };

/** Catégories regroupables, dans l'ordre où le TUI les nomme. */
type Cat = "search" | "read" | "bash" | "fetch" | "web" | "list";

const CAT_OF: Record<string, Cat> = {
  Read: "read",
  NotebookRead: "read",
  Bash: "bash",
  PowerShell: "bash",
  BashOutput: "bash",
  Grep: "search",
  Glob: "search",
  LS: "list",
  WebFetch: "fetch",
  WebSearch: "web",
};

/** Vrai si l'outil se range dans une ligne regroupée en vue normale. */
export function isGroupable(name: string): boolean {
  return name in CAT_OF;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

const PHRASE: Record<Cat, (n: number) => [string, string]> = {
  // [début de phrase, suite] : « Read 1 file » / « read 1 file ».
  read: (n) => ["Read", "read"].map((v) => `${v} ${plural(n, "file", "files")}`) as [string, string],
  bash: (n) => ["Ran", "ran"].map((v) => `${v} ${plural(n, "shell command", "shell commands")}`) as [string, string],
  search: (n) =>
    ["Searched for", "searched for"].map((v) => `${v} ${plural(n, "pattern", "patterns")}`) as [string, string],
  list: (n) => ["Listed", "listed"].map((v) => `${v} ${plural(n, "directory", "directories")}`) as [string, string],
  fetch: (n) => ["Fetched", "fetched"].map((v) => `${v} ${plural(n, "URL", "URLs")}`) as [string, string],
  web: (n) =>
    ["Searched the web", "searched the web"].map((v) => `${v} ${plural(n, "time", "times")}`) as [string, string],
};

/** « Read 1 file, ran 1 shell command » : catégories dans l'ordre d'apparition. */
export function groupLabel(rows: readonly { name: string }[]): string {
  const counts = new Map<Cat, number>();
  for (const r of rows) {
    const c = CAT_OF[r.name];
    if (c) counts.set(c, (counts.get(c) ?? 0) + 1);
  }
  const parts: string[] = [];
  for (const [c, n] of counts) parts.push(PHRASE[c](n)[parts.length === 0 ? 0 : 1]);
  return parts.join(", ");
}

/**
 * Appel `AskUserQuestion` dont la carte est ouverte : sans résultat, et
 * `tool_use_id` égal à celui de la question en attente du magasin.
 */
export function isOpenQuestion(r: { name: string; result: unknown; toolUseId: string }, openQuestion: string | null): boolean {
  return openQuestion !== null && r.name === "AskUserQuestion" && r.result === null && r.toolUseId === openQuestion;
}

/** Résultat d'un appel coupé par une interruption (« [Request interrupted by user for tool use] »). */
export function isInterruptedResult(text: string): boolean {
  return /^\[Request interrupted by user/.test(text.trim());
}

function sameRows<T>(a: readonly T[], b: readonly T[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** Cache des objets `Item` : même clé et mêmes lignes sources → même objet. */
export class ItemCache {
  #prev = new Map<string, Item>();

  /** `openQuestion` : `tool_use_id` de la question dont la carte est ouverte, ou `null`. */
  build(rows: readonly Row[], verbose: boolean, openQuestion: string | null = null): Item[] {
    const prev = this.#prev;
    const next = new Map<string, Item>();
    const out: Item[] = [];
    const v = verbose ? "v:" : "";

    const keep = <I extends Item>(item: I, same: (old: I) => boolean): I => {
      const old = prev.get(item.key) as I | undefined;
      const res = old && old.kind === item.kind && same(old) ? old : item;
      next.set(res.key, res);
      out.push(res);
      return res;
    };

    // Le tour d'un appel est clos quand une ligne `interrupted`, `turn` ou
    // `user` le suit : un appel encore sans résultat ne l'aura plus.
    // D'autres lignes (texte, appels parallèles, `agent`, `agentDone`)
    // n'en disent rien : l'appel peut être encore en cours.
    // `closedAfter[i]` : une ligne de clôture existe à l'index ≥ i.
    const closedAfter = new Uint8Array(rows.length + 1);
    for (let k = rows.length - 1; k >= 0; k--) {
      const kind = rows[k].kind;
      closedAfter[k] = closedAfter[k + 1] || kind === "interrupted" || kind === "turn" || kind === "user" ? 1 : 0;
    }
    /** Appel resté sans résultat alors que son tour est clos (interruption, fin de tour). */
    const unanswered = (r: ToolCallRow, after: number) =>
      (r.result === null && closedAfter[after] === 1) || (r.result !== null && isInterruptedResult(r.result.text));

    let i = 0;
    while (i < rows.length) {
      const r = rows[i];
      switch (r.kind) {
        case "user":
          keep({ kind: "user", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
        case "assistant":
          keep({ kind: "assistant", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
        case "tool": {
          // Carte de question ouverte (`07`) : le TUI n'affiche pas l'appel,
          // seule la carte compte. Il réapparaît avec son résultat, ou dès
          // que la carte se ferme sans réponse.
          if (isOpenQuestion(r, openQuestion)) {
            i++;
            break;
          }
          if (!verbose && isGroupable(r.name)) {
            let j = i;
            const g: ToolCallRow[] = [];
            while (j < rows.length) {
              const x = rows[j];
              if (x.kind !== "tool" || !isGroupable(x.name)) break;
              g.push(x);
              j++;
            }
            const noResult = g.every((x) => unanswered(x, j));
            keep({ kind: "group", key: v + r.id, rows: g, noResult }, (o) => o.noResult === noResult && sameRows(o.rows, g));
            i = j;
          } else {
            const noResult = unanswered(r, i + 1);
            keep({ kind: "tool", key: v + r.id, row: r, noResult }, (o) => o.row === r && o.noResult === noResult);
            i++;
          }
          break;
        }
        case "agent": {
          let j = i;
          const g: AgentCallRow[] = [];
          while (j < rows.length) {
            const x = rows[j];
            if (x.kind !== "agent") break;
            g.push(x);
            j++;
          }
          keep({ kind: "agents", key: v + r.id, rows: g }, (o) => sameRows(o.rows, g));
          i = j;
          break;
        }
        case "agentDone":
          keep({ kind: "agentDone", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
        case "turn":
          // Après une interruption, le TUI n'affiche pas de fin de tour (`08`).
          if (i > 0 && rows[i - 1].kind === "interrupted") {
            i++;
            break;
          }
          keep({ kind: "turn", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
        case "interrupted": {
          // Collée à la ligne précédente (« ⎿  Interrupted · … » sous l'appel, `08`).
          const attached = i > 0 && rows[i - 1].kind !== "user" && rows[i - 1].kind !== "turn";
          keep({ kind: "interrupted", key: v + r.id, row: r, attached }, (o) => o.row === r && o.attached === attached);
          i++;
          break;
        }
        case "notice":
          keep({ kind: "notice", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
        case "plain":
          keep({ kind: "plain", key: v + r.id, row: r }, (o) => o.row === r);
          i++;
          break;
      }
    }
    this.#prev = next;
    return out;
  }
}

/** Index de l'item qui contient la ligne source `rowId` (bascule Ctrl+O sans perdre la lecture). */
export function itemIndexOfRow(items: readonly Item[], rowId: string): number {
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if ("row" in it && it.row.id === rowId) return i;
    if ("rows" in it && it.rows.some((r) => r.id === rowId)) return i;
  }
  return -1;
}

/** Première ligne source d'un item. */
export function firstRowId(item: Item): string {
  return "row" in item ? item.row.id : item.rows[0].id;
}
