// Raccourcis globaux (jalon J7, spec décision 13, plan J7).
//
// Un seul gestionnaire de touches, qui tient compte de l'état : sélection,
// tour en cours, saisie vide ou non, palette ou sélecteur ouvert, carte de
// question. La décision est une fonction pure (`decide`), testée cas par cas
// (`e2e/unit/keys.test.ts`) ; `Keys` ne garde que l'armement du double
// Échap et l'indication « Esc again to clear ».
//
// Règles, mesurées sur le TUI 2.1.325 (captures `c*`, `a*`, `n*`, `v*`, `k*`) :
// - Ctrl+C : sélection → copie (comportement natif, rien n'est empêché) ;
//   pendant un tour → interrompt et **garde la saisie** (`c4`, `k2`) ;
//   au repos, saisie non vide → efface (`c2`) ; au repos, saisie vide → rien
//   (le TUI affiche « Press Ctrl-C again to exit », `c3` : choix du prototype).
// - Échap : palette ou sélecteur ouvert → le ferme ; carte de question →
//   l'ignore si `can_skip`, sinon rien (J8) ; pendant un tour → interrompt
//   (`08`) ; saisie non vide → 1er appui : « Esc again to clear » sans rien
//   effacer, 2e appui à moins de 0,8 s : efface (effacée à 736 ms, gardée à
//   843 ms) ; saisie vide → rien (Rewind hors du prototype 1, `v1`, `v4`).
// - Ctrl+K, et `/` en début de saisie (voir `Prompt.svelte`) : palette (J10).
// - Ctrl+O : vue détaillée (J5). Alt+P : sélecteur de modèle (comme le TUI).

/** Délai du double Échap qui efface la saisie (mesuré : 736 ms effacée, 843 ms gardée). */
export const ESC_CLEAR_MS = 800;

/** Couche ouverte au-dessus de la saisie, que Échap ferme en premier. */
export type Overlay = "palette" | "picker" | null;

/** État pris en compte par le gestionnaire au moment de l'appui. */
export type KeyContext = {
  /** Texte sélectionné dans la conversation ou dans la saisie. */
  hasSelection: boolean;
  overlay: Overlay;
  /** Carte de question ouverte (J8) ; `canSkip` : ignorable par Échap. */
  question: { canSkip: boolean } | null;
  /** Tour en cours (`Requesting`, `Streaming`). */
  busy: boolean;
  /** Interruption déjà demandée (`Interrupting`) : ne pas la renvoyer. */
  interrupting: boolean;
  draft: string;
  /** Horodatage de l'appui (ms, `performance.now()`). */
  now: number;
};

export type KeyAction =
  /** Laisser faire le navigateur (copie native, frappe normale). */
  | { type: "pass" }
  /** Touche consommée sans effet. */
  | { type: "none" }
  | { type: "copy" }
  | { type: "interrupt" }
  | { type: "clear" }
  /** Premier Échap sur une saisie non vide : indication, rien d'effacé. */
  | { type: "armClear" }
  | { type: "closeOverlay"; overlay: Exclude<Overlay, null> }
  | { type: "skipQuestion" }
  | { type: "openPalette" }
  | { type: "openModelPicker" }
  | { type: "toggleVerbose" };

/** Touche réduite à ce qui compte pour les raccourcis. */
export type KeyInput = { key: string; ctrl: boolean; alt: boolean; shift: boolean; meta: boolean };

export function keyInput(e: KeyboardEvent): KeyInput {
  return { key: e.key, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey, meta: e.metaKey };
}

const isLetter = (k: KeyInput, l: string) => k.key.toLowerCase() === l;

/**
 * Décision pure : quelle action pour cette touche dans cet état.
 * `armedAt` : horodatage du premier Échap encore armé, ou `null`.
 */
export function decide(k: KeyInput, ctx: KeyContext, armedAt: number | null): KeyAction {
  const onlyCtrl = k.ctrl && !k.alt && !k.shift && !k.meta;
  const plain = !k.ctrl && !k.alt && !k.shift && !k.meta;

  if (onlyCtrl && isLetter(k, "c")) {
    if (ctx.hasSelection) return { type: "copy" };
    if (ctx.busy) return ctx.interrupting ? { type: "none" } : { type: "interrupt" };
    if (ctx.draft.length > 0) return { type: "clear" };
    return { type: "none" };
  }

  if (plain && k.key === "Escape") {
    if (ctx.overlay) return { type: "closeOverlay", overlay: ctx.overlay };
    if (ctx.question) return ctx.question.canSkip ? { type: "skipQuestion" } : { type: "none" };
    if (ctx.busy) return ctx.interrupting ? { type: "none" } : { type: "interrupt" };
    if (ctx.draft.length === 0) return { type: "none" };
    if (armedAt !== null && ctx.now - armedAt < ESC_CLEAR_MS) return { type: "clear" };
    return { type: "armClear" };
  }

  if (onlyCtrl && isLetter(k, "k")) return { type: "openPalette" };
  if (onlyCtrl && isLetter(k, "o")) return { type: "toggleVerbose" };
  if (k.alt && !k.ctrl && !k.shift && !k.meta && isLetter(k, "p")) return { type: "openModelPicker" };
  return { type: "pass" };
}

/** Armement du double Échap et indication « Esc again to clear ». */
export class Keys {
  /** Horodatage du premier Échap armé, ou `null`. */
  armedAt: number | null = null;
  /** Indication affichée à droite au-dessus de la saisie (`a*`). */
  hint: string | null = $state(null);

  #timer: ReturnType<typeof setTimeout> | null = null;

  /** Décide, puis met à jour l'armement (seul effet de bord). */
  handle(k: KeyInput, ctx: KeyContext): KeyAction {
    const action = decide(k, ctx, this.armedAt);
    if (action.type === "armClear") this.#arm(ctx.now);
    else if (k.key !== "Shift" && k.key !== "Control" && k.key !== "Alt") this.disarm();
    return action;
  }

  /** Oublie le premier Échap (frappe, envoi, effacement). */
  disarm(): void {
    this.armedAt = null;
    this.hint = null;
    if (this.#timer) clearTimeout(this.#timer);
    this.#timer = null;
  }

  #arm(now: number): void {
    this.disarm();
    this.armedAt = now;
    this.hint = "Esc again to clear";
    // L'indication disparaît quand le second appui n'effacerait plus rien.
    this.#timer = setTimeout(() => this.disarm(), ESC_CLEAR_MS);
  }
}
