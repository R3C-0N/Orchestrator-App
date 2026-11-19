// Onglets de conversation (barre fine sous la barre de titre) : une
// conversation ouverte par onglet, chacune avec son orchestrateur
// (`claude.exe`) ; « + » ouvre un onglet sur le sélecteur des projets.
// Fonctions pures, testées dans `e2e/unit/tabs.test.ts`.
// J16 : onglet d'amélioration (`/ameliorer`), modèle dans `improve.ts`.

import type { ImproveStep } from "../core/bindings/ImproveStep";
import type { ImproveStatus } from "../core/bindings/ImproveStatus";
import { isFinal, type ImproveBadge } from "./improve";

/** Ce qu'il faut d'un onglet pour le nommer. */
export type TabNaming = {
  phase: "start" | "projects" | "sessions";
  cwd: string;
  projectPath: string | null;
};

export const NEW_TAB_LABEL = "Nouvel onglet";

/** Dernier segment d'un chemin Windows ou POSIX (`C:\a\b\` → `b`). */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((p) => p !== "");
  return parts.at(-1) ?? path;
}

/** Libellé d'un onglet : le dossier du projet, sinon « Nouvel onglet ». */
export function tabLabel(t: TabNaming): string {
  const path = t.phase === "projects" ? null : t.phase === "sessions" ? t.projectPath : t.cwd;
  const name = path ? baseName(path.trim()) : "";
  return name || NEW_TAB_LABEL;
}

/** Infobulle d'un onglet : le chemin complet quand il y en a un. */
export function tabTitle(t: TabNaming): string {
  const path = t.phase === "projects" ? null : t.phase === "sessions" ? t.projectPath : t.cwd;
  return path?.trim() || NEW_TAB_LABEL;
}

/**
 * Onglet à afficher après la fermeture de `closing` : inchangé si ce n'est
 * pas l'onglet actif, sinon son voisin de droite, à défaut celui de gauche ;
 * `null` s'il ne reste aucun onglet.
 */
export function afterClose<T>(ids: readonly T[], active: T, closing: T): T | null {
  const i = ids.indexOf(closing);
  const rest = ids.filter((x) => x !== closing);
  if (rest.length === 0) return null;
  if (closing !== active && rest.includes(active)) return active;
  if (i < 0) return rest[0];
  return rest[Math.min(i, rest.length - 1)];
}

/**
 * Onglet tel que la barre l'affiche (`TabBar.svelte`). `improve` : onglet
 * d'amélioration (J16), avec le badge d'étape de son cycle (`null` : état
 * pas encore connu).
 */
export type TabView = {
  id: number;
  label: string;
  title: string;
  busy: boolean;
  waiting: boolean;
  improve?: { badge: ImproveBadge | null; step: ImproveStep | null } | null;
};

/** Genre d'onglet : conversation de l'utilisateur, ou conversation d'une amélioration. */
export type TabKind = "conversation" | "improve";

/**
 * Onglet à activer pour un `/ameliorer` : l'onglet d'amélioration qui tient
 * déjà un cycle en cours (un seul cycle à la fois), sinon `null`.
 */
export function runningImproveTab(
  tabs: readonly { id: number; kind: TabKind; improve: Pick<ImproveStatus, "step"> | null }[],
): number | null {
  return tabs.find((t) => t.kind === "improve" && (t.improve === null || !isFinal(t.improve.step)))?.id ?? null;
}

/**
 * Onglet dont l'état est gardé à travers un rechargement (`UiSnapshot` :
 * session principale, brouillon, historique ↑↓) : l'onglet actif si c'est
 * une conversation ; sinon (onglet d'amélioration actif, J16) la dernière
 * conversation affichée (`lastConversation`), à défaut la première
 * conversation de la barre ; `null` s'il n'y en a aucune.
 */
export function snapshotTab<T extends { id: number; kind: TabKind }>(
  tabs: readonly T[],
  active: number,
  lastConversation: number | null,
): T | null {
  const conv = tabs.filter((t) => t.kind === "conversation");
  return (
    conv.find((t) => t.id === active) ??
    conv.find((t) => t.id === lastConversation) ??
    conv[0] ??
    null
  );
}
