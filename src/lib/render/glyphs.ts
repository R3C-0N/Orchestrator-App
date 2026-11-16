// Glyphes du TUI de Claude Code 2.1.325 (spec 2.1, plan 1.7), relevés dans
// les captures `.uia.txt` de J1.

/** Puce d'un message, d'un outil ou d'un agent (U+25CF). */
export const BULLET = "●";
/** Début d'une ligne de résultat : `  ⎿  ` (2 espaces, U+23BF, 2 espaces). */
export const ELBOW = "⎿";
/** Invite et entrée choisie (U+276F). */
export const PROMPT = "❯";
/** Fin de tour : « ✻ Crunched for 6s · done 12:20 ». */
export const TURN_DONE = "✻";
/** Message renvoyé par un sous-agent : « › Message from @agent… ». */
export const MESSAGE_FROM = "›";
/** Sous-lignes des agents lancés en arrière-plan. */
export const TREE_MID = "├";
export const TREE_END = "└";
/** Séparateur « · » (U+00B7). */
export const DOT = "·";

/**
 * Images de l'indicateur pendant le tour (`k1` : « ✽ Choreographing… »).
 * Le TUI fait tourner un glyphe en étoile ; la suite exacte n'est pas
 * capturée (une seule image), on garde la suite connue de Claude Code.
 */
export const SPINNER = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

/** Verbes de fin de tour observés (`02` Crunched, `03` Baked, `06` Worked, `09` Churned…). */
export const DONE_VERBS = ["Baked", "Brewed", "Churned", "Cogitated", "Cooked", "Crunched", "Sautéed", "Worked"];

/** Verbes de l'indicateur pendant le tour (`k1` : Choreographing). */
export const ACTIVE_VERBS = [
  "Choreographing",
  "Cogitating",
  "Brewing",
  "Crunching",
  "Pondering",
  "Simmering",
  "Percolating",
  "Noodling",
];

/** Choix stable d'un élément d'après une clé (même ligne → même verbe à chaque rendu). */
export function pickStable<T>(key: string, xs: readonly T[]): T {
  let h = 2166136261;
  for (let i = 0; i < key.length; i++) {
    h ^= key.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return xs[(h >>> 0) % xs.length];
}
