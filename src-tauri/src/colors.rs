//! Couleur d'un sous-agent d'après son nom (spec, décision 4 ; plan 1.7).
//!
//! Le nom vient de `subagent_type` (flux) ou de `input.subagent_type` (appel
//! `Agent`). Dans le flux, il est entouré de U+00A0 (spec 4.8) : on applique
//! `str::trim`, qui retire tout blanc Unicode (U+00A0 compris), jamais un trim
//! ASCII.
//!
//! La valeur rendue est un nom de couleur neutre (`blue`, `red`, …), le même
//! vocabulaire que le champ `color` du frontmatter des agents de Claude Code ;
//! l'interface le traduit en couleur du thème terminal. Un agent inconnu n'a
//! pas de couleur (`None`) : l'interface l'affiche en gris.

/// Table nom d'agent -> couleur. Les noms français sont ceux des agents de
/// l'orchestrateur (`agent-bleu`, …) ; les noms anglais sont acceptés aussi.
const TABLE: &[(&str, &str)] = &[
    ("bleu", "blue"),
    ("blue", "blue"),
    ("vert", "green"),
    ("green", "green"),
    ("orange", "orange"),
    ("violet", "purple"),
    ("purple", "purple"),
    ("rose", "pink"),
    ("pink", "pink"),
    ("cyan", "cyan"),
    ("jaune", "yellow"),
    ("yellow", "yellow"),
    ("rouge", "red"),
    ("red", "red"),
];

/// Couleurs possibles, dans l'ordre de la décision 4.
pub const COLORS: &[&str] = &[
    "blue", "green", "orange", "purple", "pink", "cyan", "yellow", "red",
];

/// Couleur de l'agent `name` : `agent-<couleur>` (insensible à la casse,
/// blancs Unicode retirés autour du nom). `None` pour un agent inconnu.
pub fn agent_color(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    let suffix = name.strip_prefix("agent-")?;
    TABLE.iter().find(|(k, _)| *k == suffix).map(|(_, c)| *c)
}

/// Nom d'agent normalisé : `trim()` Unicode (U+00A0 compris).
pub fn agent_name(raw: &str) -> String {
    raw.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbsp_is_trimmed_before_lookup() {
        assert_eq!(agent_color("\u{a0}agent-bleu\u{a0}"), Some("blue"));
        assert_eq!(agent_color(" agent-rouge "), Some("red"));
        assert_eq!(agent_name("\u{a0}agent-bleu\u{a0}"), "agent-bleu");
    }

    #[test]
    fn every_orchestrator_agent_has_a_color() {
        for (fr, c) in [
            ("agent-bleu", "blue"),
            ("agent-vert", "green"),
            ("agent-orange", "orange"),
            ("agent-violet", "purple"),
            ("agent-rose", "pink"),
            ("agent-cyan", "cyan"),
            ("agent-jaune", "yellow"),
            ("agent-rouge", "red"),
        ] {
            assert_eq!(agent_color(fr), Some(c), "{fr}");
            assert!(COLORS.contains(&c));
        }
        assert_eq!(agent_color("Agent-Bleu"), Some("blue"));
    }

    #[test]
    fn unknown_agents_have_no_color() {
        assert_eq!(agent_color("general-purpose"), None);
        assert_eq!(agent_color("Explore"), None);
        assert_eq!(agent_color("agent-"), None);
        assert_eq!(agent_color("agent-magenta"), None);
        assert_eq!(agent_color(""), None);
    }
}
