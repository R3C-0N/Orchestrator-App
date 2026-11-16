//! J11 : `CLAUDE_CONFIG_DIR` déplace `~/.claude` (et donc les transcripts).
//! Seul test de ce binaire : il modifie l'environnement du processus.

use std::path::PathBuf;

use orchestrator_lib::sessions::list::list_projects;
use orchestrator_lib::sessions::paths::{claude_config_dir, projects_root, CONFIG_DIR_ENV};

#[test]
fn claude_config_dir_moves_the_projects_root() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/transcripts");
    // SAFETY : seul test de ce binaire, aucun autre fil ne lit l'environnement.
    unsafe { std::env::set_var(CONFIG_DIR_ENV, &fixtures) };
    assert_eq!(claude_config_dir(), Some(fixtures.clone()));
    assert_eq!(projects_root(), Some(fixtures.join("projects")));
    let p = list_projects(&projects_root().unwrap());
    assert!(p.iter().any(|x| x.encoded == "C--Users--user--Code-demo"));

    // Vide : ignoré, retour à `%USERPROFILE%\.claude`.
    unsafe { std::env::set_var(CONFIG_DIR_ENV, "") };
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap();
    assert_eq!(claude_config_dir(), Some(home.join(".claude")));
    unsafe { std::env::remove_var(CONFIG_DIR_ENV) };
    assert_eq!(projects_root(), Some(home.join(".claude").join("projects")));
}
