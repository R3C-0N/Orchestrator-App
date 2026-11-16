// Modèle d'affichage de la colonne des agents (jalon J9, décision 4).
//
// Le style reprend le panneau d'agents du TUI 2.1.325 (capture `05`) :
//   ● main
//   ◯ agent-bleu   Ping bleu agent              6s · ↓ 38.9k tokens
// plus, dans l'app, la couleur de l'agent (nom sur fond de sa couleur, comme
// les sous-lignes `├` / `└` du fil principal), son état et `last_tool`.
//
// Fonctions pures : testées sans DOM (`e2e/unit/agents.test.ts`).

import type { AgentEvent } from "../../core/bindings/AgentEvent";
import type { AgentStatus } from "../../core/bindings/AgentStatus";
import { BULLET, DOT } from "../../render/glyphs";
import { seconds, shortResult, tokens, toolArg, toolTitle } from "../../render/tools";
import type { Row, ToolCallRow } from "../../state/session.svelte";

/** Agent en cours dans le panneau du TUI (`05`). */
export const RUNNING_GLYPH = "◯";

export type AgentTone = "dim" | "ok" | "err";

const OVER = new Set<AgentStatus>(["Completed", "Failed", "Stopped"]);

/** Vrai tant que l'agent tourne (sa durée avance). */
export function isLive(a: Pick<AgentEvent, "status">): boolean {
  return !OVER.has(a.status);
}

/**
 * Glyphe d'état : `◯` gris pendant l'exécution (`05`) ; une fois fini,
 * `●` vert (terminé), rouge (échec) ou gris (arrêté). Le TUI retire la ligne
 * d'un agent fini ; la colonne la garde (écart assumé, voir le rapport J9).
 */
export function agentGlyph(status: AgentStatus): { glyph: string; tone: AgentTone } {
  switch (status) {
    case "Completed":
      return { glyph: BULLET, tone: "ok" };
    case "Failed":
      return { glyph: BULLET, tone: "err" };
    case "Stopped":
      return { glyph: BULLET, tone: "dim" };
    default:
      return { glyph: RUNNING_GLYPH, tone: "dim" };
  }
}

/** Durée écoulée (ms) : figée à `ended_at`, sinon jusqu'à `now`. `null` sans début connu. */
export function agentElapsed(a: Pick<AgentEvent, "started_at" | "ended_at" | "status">, now: number): number | null {
  if (a.started_at === null) return null;
  const end = a.ended_at ?? (isLive(a) ? now : null);
  if (end === null) return null;
  return Math.max(0, end - a.started_at);
}

/** Partie droite de la ligne : « 6s · ↓ 38.9k tokens » (`05`), sans les morceaux inconnus. */
export function agentStats(a: AgentEvent, now: number): string {
  const parts: string[] = [];
  const ms = agentElapsed(a, now);
  if (ms !== null) parts.push(seconds(ms));
  if (a.tokens !== null) parts.push(`↓ ${tokens(a.tokens)} tokens`);
  return parts.join(` ${DOT} `);
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * Ligne `⎿` sous l'agent : l'outil en cours pendant l'exécution
 * (`last_tool`, nom affiché comme dans le fil : Edit → Update, Grep → Search),
 * l'état final ensuite.
 */
export function agentDetail(a: AgentEvent): { text: string; tone: AgentTone } {
  const uses = a.tool_uses !== null ? ` ${DOT} ${plural(a.tool_uses, "tool use", "tool uses")}` : "";
  switch (a.status) {
    case "Starting":
      return { text: "Starting…", tone: "dim" };
    case "Running":
      return { text: a.last_tool ? `${toolTitle(a.last_tool)}…` : "Running…", tone: "dim" };
    case "Completed":
      return { text: `Done${uses}`, tone: "dim" };
    case "Failed":
      return { text: `Failed${uses}`, tone: "err" };
    case "Stopped":
      return { text: `Stopped${uses}`, tone: "dim" };
  }
}

/** Libellé d'accessibilité complet (le texte affiché est tronqué). */
export function agentLabel(a: AgentEvent, now: number): string {
  const stats = agentStats(a, now);
  return [a.agent_type.trim(), a.description, a.status, a.last_tool ?? "", stats].filter(Boolean).join(", ");
}

/** Une ligne du fil déroulé d'un agent (bonus J9). */
export type ThreadLine =
  | { key: string; kind: "prompt"; text: string }
  | { key: string; kind: "text"; text: string; done: boolean }
  | { key: string; kind: "tool"; title: string; arg: string; result: string | null; isError: boolean };

function firstLine(s: string, max = 400): string {
  const t = s.trim();
  return t.length > max ? `${t.slice(0, max)}…` : t;
}

/**
 * Fil d'un sous-agent (`SessionStore.threads`) mis à plat pour l'affichage :
 * consigne (`❯`), textes (`●`), appels d'outils (`● Read(a.txt)` puis
 * `⎿  Read 1 lines`). Des objets neufs à chaque appel : les lignes du fil
 * sont mutées sur place par le magasin, l'affichage doit voir le changement.
 */
export function threadLines(rows: readonly Row[], cwd: string | null): ThreadLine[] {
  const out: ThreadLine[] = [];
  for (const r of rows) {
    switch (r.kind) {
      case "user":
        out.push({ key: r.id, kind: "prompt", text: firstLine(r.text) });
        break;
      case "assistant":
        if (r.text.trim() || !r.done) out.push({ key: r.id, kind: "text", text: r.text.trim(), done: r.done });
        break;
      case "tool": {
        const t = r as ToolCallRow;
        const arg = t.input === null ? "" : toolArg(t, cwd, false);
        const res = t.result === null ? null : firstLine(t.result.isError ? t.result.text : shortResult(t), 200);
        out.push({
          key: r.id,
          kind: "tool",
          title: toolTitle(t.name, t.input),
          arg,
          result: res,
          isError: t.result?.isError ?? false,
        });
        break;
      }
      default:
        break;
    }
  }
  return out;
}
