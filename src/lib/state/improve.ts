// Onglet d'amélioration (J16, décisions 17 et 22) : modèle pur, testable sans
// le DOM (`e2e/unit/improve.test.ts`). Libellés des étapes, textes de l'état
// (repris de l'ancien bandeau `ImproveBanner`, supprimé), badge du titre de
// l'onglet, droits des boutons et de la saisie, fermeture de l'onglet.

import type { ImproveStatus } from "../core/bindings/ImproveStatus";
import type { ImproveStep } from "../core/bindings/ImproveStep";
import type { ImproveWaiting } from "../core/bindings/ImproveWaiting";
import type { SessionKey } from "../core/bindings/SessionKey";
import type { SessionResync } from "../core/bindings/SessionResync";

/** États finaux d'un cycle : plus rien ne bouge, l'onglet se ferme sans confirmation. */
export const FINAL_STEPS: readonly ImproveStep[] = ["merged", "restart_required", "failed", "cancelled", "rolled_back"];

export const STEP_LABEL: Record<ImproveStep, string> = {
  preparing: "préparation du worktree",
  working: "Claude modifie le code (session dédiée)",
  fixing: "correction automatique (erreur renvoyée à la session)",
  awaiting_user: "en attente de l'utilisateur",
  checking: "contrôle de la branche",
  testing: "tests (check, unitaires, e2e)",
  building: "build de l'interface",
  waiting_idle: "attente de la fin du tour en cours",
  switching: "bascule vers la nouvelle interface",
  merged: "fusionné",
  restart_required: "fusionné, relance requise",
  failed: "échec",
  cancelled: "annulé",
  rolled_back: "retour à la version précédente",
};

/** Raison d'une attente (`ImproveStatus.waiting`). */
export const WAITING_LABEL: Record<ImproveWaiting, string> = {
  no_new_commit: "tour fini sans nouveau commit : répondre, ou « Valider » pour lancer contrôles et tests",
  interrupted: "tour interrompu : écrire la suite, ou « Valider »",
  attempts_exhausted: "corrections automatiques épuisées : préciser la demande, ou « Valider » pour réessayer",
  session_stopped: "session arrêtée avant la fin de son tour : écrire pour relancer, ou « Valider »",
};

/** Délai sans état final après « Annuler l'amélioration » avant de le signaler. */
export const CANCEL_TIMEOUT_MS = 15_000;

export function isFinal(step: ImproveStep): boolean {
  return FINAL_STEPS.includes(step);
}

/** Demande sur une ligne (les retours à la ligne deviennent des espaces), tronquée à `max`. */
export function oneLine(request: string, max = 120): string {
  const s = request.replace(/\s+/g, " ").trim();
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** Vrai si `next` doit remplacer `cur` : pas plus ancien (l'événement le plus récent l'emporte sur `get_ui_info`). */
export function isNewer(next: ImproveStatus, cur: ImproveStatus | null): boolean {
  return !cur || next.t >= cur.t;
}

/** Texte de l'état courant (ligne de la barre de contrôle de l'onglet). */
export function bannerText(s: ImproveStatus, repoPath: string | null): string {
  const commit = s.commit ? ` ${s.commit.slice(0, 7)}` : "";
  switch (s.step) {
    case "merged":
      return `Amélioration fusionnée${commit} dans main du clone ; interface active.`;
    case "restart_required":
      return (
        `Relance requise : le cœur Rust a changé (commit${commit || " ?"} fusionné, non activé). ` +
        `Recompiler avec \`npm run tauri build\` dans le clone${repoPath ? ` (${repoPath})` : ""}, ` +
        `réinstaller puis relancer l'app : la session sera reprise.`
      );
    case "failed":
      return `Échec de l'amélioration${s.detail ? ` : ${s.detail}` : ""}. Branche ${s.branch} laissée non fusionnée.`;
    case "cancelled":
      return `Amélioration annulée. Branche ${s.branch} laissée non fusionnée.`;
    case "rolled_back":
      return `Retour à la version précédente de l'interface${commit ? ` (annule${commit})` : ""}.`;
    case "awaiting_user":
      return `${STEP_LABEL.awaiting_user}${s.waiting ? ` · ${WAITING_LABEL[s.waiting]}` : ""}${s.detail ? ` · ${s.detail}` : ""}`;
    default:
      return `${STEP_LABEL[s.step]}${s.detail ? ` · ${s.detail}` : ""}`;
  }
}

/**
 * Envoi de la branche après la fusion (propriétaire de l'app seulement), en
 * ligne du fil ; `null` sans envoi (tous les autres utilisateurs).
 */
export function pushText(s: ImproveStatus): string | null {
  const p = s.push;
  if (!p) return null;
  switch (p.state) {
    case "pushing":
      return `envoi sur GitHub : ${p.branch}…`;
    case "pushed":
      return `poussé sur GitHub : ${p.branch}`;
    case "failed":
      return `fusionné localement, envoi échoué : ${p.reason} (nouvel essai au prochain cycle ou au prochain démarrage)`;
  }
}

// --- onglet -------------------------------------------------------------------

export const IMPROVE_TAB_PREFIX = "Amélioration";

/** Libellé de l'onglet : « Amélioration · <sujet> ». */
export function improveTabLabel(request: string | null, max = 32): string {
  const subject = request ? oneLine(request, max) : "";
  return subject ? `${IMPROVE_TAB_PREFIX} · ${subject}` : IMPROVE_TAB_PREFIX;
}

/** Infobulle de l'onglet : la demande complète et la branche. */
export function improveTabTitle(s: ImproveStatus | null): string {
  if (!s) return IMPROVE_TAB_PREFIX;
  return `/ameliorer ${oneLine(s.request, 400)}\n${s.branch} · ${STEP_LABEL[s.step]}`;
}

export type BadgeTone = "run" | "wait" | "ok" | "warn" | "err";
export type ImproveBadge = { text: string; tone: BadgeTone };

/**
 * Badge d'étape dans le titre de l'onglet : « tour 2 », « contrôles »,
 * « tests · 1/3 », « en attente », « fusionné », « échec »…
 * `n/3` : tentative de correction automatique en cours (`attempt` > 0).
 */
export function improveBadge(s: ImproveStatus): ImproveBadge {
  const tries = s.attempt > 0 && !isFinal(s.step) ? ` · ${s.attempt}/${s.max_attempts}` : "";
  switch (s.step) {
    case "preparing":
      return { text: "préparation", tone: "run" };
    case "working":
      return { text: `tour ${s.turn + 1}${tries}`, tone: "run" };
    case "fixing":
      return { text: `correction${tries || ` · ${s.attempt}/${s.max_attempts}`}`, tone: "run" };
    case "checking":
      return { text: `contrôles${tries}`, tone: "run" };
    case "testing":
      return { text: `tests${tries}`, tone: "run" };
    case "building":
      return { text: "build", tone: "run" };
    case "waiting_idle":
      return { text: "attente", tone: "run" };
    case "switching":
      return { text: "bascule", tone: "run" };
    case "awaiting_user":
      return { text: `en attente${tries}`, tone: "wait" };
    case "merged":
      return { text: "fusionné", tone: "ok" };
    case "restart_required":
      return { text: "relance requise", tone: "warn" };
    case "failed":
      return { text: "échec", tone: "err" };
    case "cancelled":
      return { text: "annulé", tone: "warn" };
    case "rolled_back":
      return { text: "retour arrière", tone: "warn" };
  }
}

/** « Valider » : actif seulement quand le cycle attend l'utilisateur. */
export function canValidate(s: ImproveStatus | null): boolean {
  return s !== null && s.step === "awaiting_user";
}

/** « Annuler l'amélioration » : cycle en cours, et pas encore à la bascule. */
export function canCancel(s: ImproveStatus | null): boolean {
  return s !== null && !isFinal(s.step) && s.step !== "switching";
}

/** Étapes où un message de l'utilisateur est accepté (tour en cours, contrôles, tests, attente). */
const SENDABLE: readonly ImproveStep[] = ["working", "fixing", "checking", "testing", "awaiting_user"];

/** Raison pour laquelle la saisie de l'onglet est fermée, ou `null` si on peut écrire. */
export function sendBlocked(s: ImproveStatus | null): string | null {
  if (!s) return "amélioration inconnue";
  if (SENDABLE.includes(s.step)) return null;
  if (isFinal(s.step)) return `amélioration terminée (${STEP_LABEL[s.step]})`;
  return STEP_LABEL[s.step];
}

/** Échap interrompt le tour de la session (`improve_interrupt`) seulement pendant un tour. */
export function canInterrupt(s: ImproveStatus | null): boolean {
  return s !== null && (s.step === "working" || s.step === "fixing");
}

/**
 * Fermeture de l'onglet : `dismiss` (état final : fermeture directe, puis
 * `improve_dismiss`), `confirm` (cycle en cours, ou état inconnu par
 * prudence : confirmation dans la page) ou `refuse` (bascule en cours).
 */
export type CloseAction = "dismiss" | "confirm" | "refuse";

export function closeAction(s: ImproveStatus | null): CloseAction {
  if (!s) return "confirm";
  if (isFinal(s.step)) return "dismiss";
  if (s.step === "switching") return "refuse";
  return "confirm";
}

/** « Abandonner » de la confirmation : possible tant que le cycle peut être annulé, ou si son état est inconnu. */
export function canAbandon(s: ImproveStatus | null): boolean {
  return s === null || canCancel(s);
}

/** Texte de la confirmation de fermeture (jamais `confirm()`). */
export function closeConfirmText(s: ImproveStatus | null): string {
  return s
    ? `Fermer abandonne l'amélioration ; la branche ${s.branch} est gardée non fusionnée.`
    : "Fermer abandonne l'amélioration ; sa branche auto/… est gardée non fusionnée.";
}

/**
 * Touche pendant la confirmation de fermeture : Échap vaut « Continuer »
 * (jamais une interruption de la session) ; `null` : touche laissée au
 * gestionnaire habituel.
 */
export function confirmKey(key: string, confirming: boolean): "continue" | null {
  return confirming && key === "Escape" ? "continue" : null;
}

/**
 * Montage : que faire de l'amélioration dont l'onglet a été fermé
 * (`UiSnapshot.improveDismissed`) ? `dismiss` : son état connu est final,
 * `improve_dismiss` tout de suite (la page a pu se recharger entre
 * « Abandonner » et l'état `cancelled`) ; `wait` : `improve_dismiss` à son
 * état final ; `null` : rien à faire.
 */
export function dismissOnMount(dismissed: string | null, known: ImproveStatus | null): "dismiss" | "wait" | null {
  if (dismissed === null) return null;
  // État encore inconnu : attendre l'événement.
  if (!known) return "wait";
  // Un cycle plus récent existe (un seul à la fois) : celui-ci est fini.
  if (known.id !== dismissed) return "dismiss";
  return isFinal(known.step) ? "dismiss" : "wait";
}

/** `/ameliorer` alors que le cycle abandonné n'a pas encore son état final. */
export const CANCELLING_TEXT = "Amélioration en cours d'annulation : réessayer à son état final";

export const CLOSE_REFUSED_TEXT = "Fermeture impossible pendant la bascule vers la nouvelle interface.";

/**
 * Lignes d'envoi à ajouter au fil pour un nouvel état : le texte de l'envoi
 * s'il a changé depuis `last` (la dernière ligne écrite pour ce cycle).
 */
export function pushLine(s: ImproveStatus, last: string | null): string | null {
  const text = pushText(s);
  return text !== null && text !== last ? text : null;
}

/** Préfixe des `notice` du cœur qui portent les étapes du pipeline (`PipelineHost::note`). */
export const IMPROVE_NOTICE_PREFIX = "improve.";

export function isImproveNotice(code: string): boolean {
  return code.startsWith(IMPROVE_NOTICE_PREFIX);
}

/**
 * Rechargement (rattachement J14) : conversation d'amélioration à rouvrir
 * dans un onglet, ou `null`. Il faut une conversation de rôle `improve`
 * rendue par `attach_sessions` qui soit celle de l'amélioration connue
 * (`UiInfo.improve.session`), et un onglet que l'utilisateur n'a pas fermé
 * (`dismissed`).
 */
export function improveToReopen(
  live: readonly Pick<SessionResync, "session" | "role">[],
  status: ImproveStatus | null,
  dismissed: string | null,
): { session: SessionKey; status: ImproveStatus } | null {
  if (!status || status.session === null || status.id === dismissed) return null;
  const r = live.find((x) => x.role === "improve" && x.session === status.session);
  return r ? { session: r.session, status } : null;
}

/** `notice` émise juste avant l'arrêt voulu du processus d'un tour (`detail` : `{turn, end, pid}`). */
export const IMPROVE_TURN_END = "improve.turn_end";
/** `notice` `warn` en tête du premier cycle : worktrees `auto/…` non fusionnés (`detail` : `{branches}`). */
export const IMPROVE_UNMERGED = "improve.unmerged_worktrees";

/**
 * Premier message du fil d'une amélioration : le prompt complet du pipeline
 * (`tools/self-improve/prompt.md`), replié en une ligne
 * « Consigne d'amélioration · <demande> ». La demande est la section
 * « ## Demande de l'utilisateur » ; sans elle, tout le texte sur une ligne.
 */
export function improvePromptLine(prompt: string, max = 160): string {
  const m = /##\s*Demande de l'utilisateur\s*\n([\s\S]*?)(?:\n##\s|$)/.exec(prompt);
  return `Consigne d'amélioration · ${oneLine(m ? m[1] : prompt, max)}`;
}
