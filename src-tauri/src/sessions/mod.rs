//! Projets, sessions et reprise (jalon J11, plan 1.8, décisions 9 et 11).
//!
//! - `paths` : emplacement et encodage des dossiers de `~/.claude/projects` ;
//! - `list` : projets récents et sessions d'un projet ;
//! - `transcript` : historique d'une session reprise, lecteur isolé et
//!   tolérant, passé au réducteur du flux.
//!
//! Lecture seule : rien n'est jamais écrit sous `~/.claude`.

pub mod list;
pub mod paths;
pub mod transcript;

/// Dossier de projet au lancement (décision 11) : `<app> .` ou
/// `<app> <chemin>` l'impose ; sans argument (ou avec un argument qui n'est
/// pas un dossier), l'interface ouvre le sélecteur des projets récents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchDir {
    /// Dossier passé en argument (absolu, sans `\?\`), sinon dossier courant.
    pub cwd: std::path::PathBuf,
    /// Vrai si un dossier valide a été passé en argument.
    pub from_args: bool,
    /// Argument donné mais inutilisable (absent, pas un dossier).
    pub error: Option<String>,
}

/// Lit le dossier de projet dans les arguments de l'app (le premier qui ne
/// commence pas par `-`, comme `process::locate::launch_cwd_from_args`).
pub fn launch_dir_from_args<I: IntoIterator<Item = String>>(args: I) -> LaunchDir {
    let args: Vec<String> = args.into_iter().collect();
    let given = args.iter().skip(1).find(|a| !a.starts_with('-')).cloned();
    let current = || {
        crate::process::locate::launch_cwd_from_args(vec![String::new()])
            .or_else(|_| std::env::current_dir())
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
    };
    match given {
        None => LaunchDir {
            cwd: current(),
            from_args: false,
            error: None,
        },
        Some(arg) => match crate::process::locate::launch_cwd_from_args(args) {
            Ok(cwd) => LaunchDir {
                cwd,
                from_args: true,
                error: None,
            },
            Err(e) => LaunchDir {
                cwd: current(),
                from_args: false,
                error: Some(format!("{arg} : {e}")),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_dir_distinguishes_dot_from_nothing() {
        let none = launch_dir_from_args(vec!["app".to_string()]);
        assert!(!none.from_args);
        assert!(none.error.is_none());
        let dot = launch_dir_from_args(vec!["app".into(), ".".into()]);
        assert!(dot.from_args);
        assert_eq!(dot.cwd, none.cwd);
        let flag_only = launch_dir_from_args(vec!["app".into(), "--verbose".into()]);
        assert!(!flag_only.from_args);
        let missing = launch_dir_from_args(vec!["app".into(), "n-existe-pas-xyz".into()]);
        assert!(!missing.from_args);
        assert!(missing.error.unwrap().contains("n-existe-pas-xyz"));
    }
}
