//! Emplacement des transcripts du CLI (plan 1.8, décision 11).
//!
//! `~/.claude/projects/<cwd encodé>/<session_id>.jsonl` ; l'encodage remplace
//! chaque caractère hors `[A-Za-z0-9]` par `-`. Il n'est pas réversible
//! (`H:\a-b` et `H:\a\b` donnent le même nom) : le vrai chemin d'un projet se
//! lit dans le champ `cwd` de ses transcripts (`list::read_project_cwd`).
//!
//! **Lecture seule** : rien dans ce module n'écrit sous `~/.claude`.

use std::path::{Path, PathBuf};

/// Variable du CLI qui déplace son dossier de configuration (`~/.claude`).
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// Dossier de configuration du CLI : `CLAUDE_CONFIG_DIR`, sinon
/// `%USERPROFILE%\.claude` (`$HOME/.claude` hors Windows).
pub fn claude_config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(CONFIG_DIR_ENV).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("USERPROFILE")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|h| !h.is_empty()))?;
    Some(PathBuf::from(home).join(".claude"))
}

/// `~/.claude/projects`.
pub fn projects_root() -> Option<PathBuf> {
    claude_config_dir().map(|d| d.join("projects"))
}

/// Nom du dossier d'un projet : tout caractère hors `[A-Za-z0-9]` devient `-`
/// (un caractère non ASCII compte pour un seul `-`).
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Dossier des transcripts de `cwd` sous `root`. Nom exact d'abord, puis sans
/// tenir compte de la casse (sous Windows, `h:\x` et `H:\x` sont le même
/// dossier mais pas le même nom encodé). `None` si aucun dossier ne correspond.
pub fn find_project_dir(root: &Path, cwd: &str) -> Option<PathBuf> {
    let cwd = cwd.trim_end_matches(['\\', '/']);
    let name = encode_project_dir(cwd);
    let exact = root.join(&name);
    if exact.is_dir() {
        return Some(exact);
    }
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(&name))
                && e.path().is_dir()
        })
        .map(|e| e.path())
}

/// Un `session_id` utilisable : un UUID (règle de l'app, spec 4.6). Évite
/// aussi qu'un id serve à sortir du dossier du projet (`..\`).
pub fn is_session_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok() && id.len() == 36
}

/// Chemin du transcript d'une session.
pub fn transcript_path(project_dir: &Path, session_id: &str) -> PathBuf {
    project_dir.join(format!("{session_id}.jsonl"))
}

/// Dossier des sous-agents d'une session : `<session_id>/subagents/`.
pub fn subagents_dir(project_dir: &Path, session_id: &str) -> PathBuf {
    project_dir.join(session_id).join("subagents")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_the_cli() {
        // Constaté dans ~/.claude/projects (plan 1.8).
        assert_eq!(
            encode_project_dir(r"H:\user\Code\orchestrator"),
            "H--user-Code-orchestrator"
        );
        assert_eq!(
            encode_project_dir(r"H:\user\Code\orchestrator\.claude-work\j0-sandbox"),
            "H--user-Code-orchestrator--claude-work-j0-sandbox"
        );
        // Non réversible : deux chemins, un seul nom.
        assert_eq!(encode_project_dir(r"H:\a-b"), encode_project_dir(r"H:\a\b"));
        assert_eq!(encode_project_dir("C:\\é"), "C---");
    }

    #[test]
    fn session_id_must_be_a_uuid() {
        assert!(is_session_id("11111111-2222-4333-8444-555555555555"));
        assert!(!is_session_id("..\\..\\x"));
        assert!(!is_session_id("mon-nom"));
        assert!(!is_session_id("11111111222243338444555555555555"));
    }
}
