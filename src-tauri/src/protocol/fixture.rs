//! Lignes des captures `fixtures/stream-json/*.jsonl` (README des fixtures).
//!
//! Le pilote de capture mêle à la sortie du CLI ses propres lignes :
//! `>>> {json}` (ce qu'il a écrit sur stdin), `STDERR …` et `EXIT n`. Le rejeu
//! (plan 1.10, point 1) ne passe au décodeur que les lignes du CLI ; les
//! lignes `>>> ` servent de référence à la sérialisation sortante (point 2).

/// Une ligne de capture classée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureLine<'a> {
    /// Sortie du CLI (une ligne JSON).
    Cli(&'a str),
    /// Ligne écrite sur stdin par le pilote (sans le préfixe `>>> `).
    Sent(&'a str),
    /// `STDERR …` (texte après le préfixe).
    Stderr(&'a str),
    /// `EXIT n` ; `None` si le code est illisible.
    Exit(Option<i32>),
    /// Ligne vide ou blanche.
    Blank,
}

/// Classe une ligne (déjà débarrassée de sa fin de ligne).
pub fn classify(line: &str) -> FixtureLine<'_> {
    if let Some(rest) = line.strip_prefix(">>> ") {
        FixtureLine::Sent(rest)
    } else if let Some(rest) = line.strip_prefix("EXIT") {
        FixtureLine::Exit(rest.trim().parse().ok())
    } else if let Some(rest) = line.strip_prefix("STDERR") {
        FixtureLine::Stderr(rest.strip_prefix(' ').unwrap_or(rest))
    } else if line.trim().is_empty() {
        FixtureLine::Blank
    } else {
        FixtureLine::Cli(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_lines() {
        assert_eq!(classify(r#">>> {"a":1}"#), FixtureLine::Sent(r#"{"a":1}"#));
        assert_eq!(classify("EXIT 0"), FixtureLine::Exit(Some(0)));
        assert_eq!(classify("EXIT 1"), FixtureLine::Exit(Some(1)));
        assert_eq!(classify("EXIT ?"), FixtureLine::Exit(None));
        assert_eq!(classify("STDERR oups"), FixtureLine::Stderr("oups"));
        assert_eq!(classify("  "), FixtureLine::Blank);
        assert_eq!(
            classify(r#"{"type":"x"}"#),
            FixtureLine::Cli(r#"{"type":"x"}"#)
        );
    }
}
