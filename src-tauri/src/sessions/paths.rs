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

/// Le transcript de la session `session_id` lancée dans `cwd` existe sous
/// `root` (J16 : reprise d'un tour de la session d'amélioration). Un id qui
/// n'est pas un UUID : faux.
pub fn transcript_exists(root: &Path, cwd: &str, session_id: &str) -> bool {
    is_session_id(session_id)
        && find_project_dir(root, cwd).is_some_and(|d| transcript_path(&d, session_id).is_file())
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
        // Formes non canoniques que `uuid::Uuid::parse_str` accepte : refusées
        // (le fichier du CLI porte la forme canonique, spec 4.6).
        assert!(!is_session_id("{11111111-2222-4333-8444-555555555555}"));
        assert!(!is_session_id(
            "urn:uuid:11111111-2222-4333-8444-555555555555"
        ));
        assert!(!is_session_id(" 11111111-2222-4333-8444-555555555555"));
        assert!(!is_session_id("11111111-2222-4333-8444-55555555555g"));
        assert!(!is_session_id(""));
    }

    /// J16 : reprise possible seulement si le transcript existe.
    #[test]
    fn transcript_exists_for_resume() {
        let root = std::env::temp_dir().join(format!("orch-j16-tr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = r"C:\x\orchestrator\work\auto\demo";
        let id = "11111111-2222-4333-8444-555555555555";
        assert!(!transcript_exists(&root, cwd, id), "racine absente");
        let dir = root.join(encode_project_dir(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            !transcript_exists(&root, cwd, id),
            "dossier sans transcript"
        );
        std::fs::write(transcript_path(&dir, id), "{}\n").unwrap();
        assert!(transcript_exists(&root, cwd, id));
        assert!(transcript_exists(&root, &cwd.to_lowercase(), id), "casse");
        assert!(!transcript_exists(&root, cwd, "..\\x"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
