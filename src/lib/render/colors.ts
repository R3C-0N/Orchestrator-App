// Couleurs du TUI 2.1.325 (thème `dark`, Windows Terminal, jeu Campbell),
// mesurées sur les pixels des captures de J1. Les mêmes valeurs sont posées
// en variables CSS dans `app.css` (`--t-*`) ; ce module sert au code qui
// choisit une couleur (agents).

export const PALETTE = {
  /** Fond du terminal (Campbell). */
  bg: "#0C0C0C",
  /** Texte courant (Campbell « foreground »), `03b`. */
  fg: "#CCCCCC",
  /** Texte gras (blanc vif), `02`, `04`. */
  bold: "#F2F2F2",
  /** Texte du message utilisateur et puce de réponse, `02`. */
  white: "#FFFFFF",
  /** Éléments secondaires : regroupements, `⎿`, fin de tour, horodatage, `03b`. */
  dim: "#999999",
  /** Bande du message utilisateur, `02`. */
  band: "#373737",
  /** Code en ligne, sélection, `02`. */
  lavender: "#B1B9F9",
  /** Puce d'un outil réussi, `04`. */
  ok: "#4EBA65",
  /** Puce et texte d'erreur, `09`. */
  error: "#FF6B80",
  /** Indicateur de tour (glyphe), `k1`. */
  spinner: "#D77757",
  /** Indicateur de tour (verbe), `k1`. */
  spinnerText: "#DF8666",
  /** Règle de la saisie (`02`) et barre de la vue détaillée (`03b`). */
  rule: "#888888",
  ruleDetail: "#666666",
} as const;

/**
 * Fond du nom d'un agent (texte noir dessus), d'après sa couleur
 * (`AgentEvent.color`, ou le nom `agent-<couleur>` après `trim()`).
 * Mesurés : bleu `#6A9BCC`, rouge `#DC2626` (`05`). Les autres viennent
 * de la même gamme du thème `dark` et ne sont pas capturés.
 */
const AGENT_BG: Record<string, string> = {
  blue: "#6A9BCC",
  red: "#DC2626",
  green: "#16A34A",
  yellow: "#CA8A04",
  purple: "#9333EA",
  orange: "#EA580C",
  pink: "#DB2777",
  cyan: "#0891B2",
};

const FR: Record<string, string> = {
  bleu: "blue",
  rouge: "red",
  vert: "green",
  jaune: "yellow",
  violet: "purple",
  orange: "orange",
  rose: "pink",
  cyan: "cyan",
};

/** Agent inconnu : gris neutre (décision 4). */
export const AGENT_NEUTRAL = "#767676";

function norm(name: string | null | undefined): string | null {
  if (!name) return null;
  // U+00A0 et espaces : `subagent_type` arrive parfois entouré d'espaces (spec 4.4).
  const n = name.replace(/ /g, " ").trim().toLowerCase();
  if (AGENT_BG[n]) return n;
  if (FR[n]) return FR[n];
  const m = /^agent-(.+)$/.exec(n);
  if (m) return norm(m[1]);
  return null;
}

/** Couleur de fond du nom d'un agent. */
export function agentColor(color: string | null | undefined, agentType?: string | null): string {
  const k = norm(color) ?? norm(agentType);
  return k ? AGENT_BG[k] : AGENT_NEUTRAL;
}

/** Nom affiché de l'agent : `subagent_type` sans espaces autour. */
export function agentName(agentType: string): string {
  return agentType.replace(/ /g, " ").trim();
}
