//! Recherche de `claude.exe` (plan 1.9) : `ORCH_CLAUDE_PATH`, puis
//! `%USERPROFILE%\.local\bin\claude.exe`, puis `PATH`.

use std::path::PathBuf;

/// Version de `claude.exe` validée (spec 4.8). Une autre version déclenche un
/// avertissement, sans bloquer.
pub const EXPECTED_CLAUDE_VERSION: &str = "2.1.325";

/// Renvoie le chemin trouvé, ou la liste des emplacements essayés.
pub fn locate_claude() -> Result<PathBuf, Vec<String>> {
    let mut tried = Vec::new();

    if let Some(p) = std::env::var_os("ORCH_CLAUDE_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        tried.push(format!("ORCH_CLAUDE_PATH={}", p.display()));
    }

    if let Some(home) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(home)
            .join(".local")
            .join("bin")
            .join("claude.exe");
        if p.is_file() {
            return Ok(p);
        }
        tried.push(p.display().to_string());
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("claude.exe");
            if p.is_file() {
                return Ok(p);
            }
        }
        tried.push("PATH (claude.exe)".into());
    }

    Err(tried)
}

/// Retire le préfixe `\\?\` que `canonicalize` ajoute sous Windows : certains
/// programmes (dont Node/Bun) le gèrent mal comme dossier courant.
pub fn display_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.to_string(),
        _ => s,
    }
}

/// Dossier de projet : premier argument de l'app s'il est un dossier, sinon le
/// dossier courant. Chemin absolu, sans préfixe `\\?\`.
pub fn launch_cwd_from_args<I: IntoIterator<Item = String>>(args: I) -> std::io::Result<PathBuf> {
    let current = std::env::current_dir()?;
    let arg = args.into_iter().skip(1).find(|a| !a.starts_with('-'));
    let candidate = match arg {
        Some(a) => {
            let p = PathBuf::from(a);
            if p.is_absolute() {
                p
            } else {
                current.join(p)
            }
        }
        None => current,
    };
    let canon = candidate.canonicalize()?;
    if !canon.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("pas un dossier : {}", canon.display()),
        ));
    }
    Ok(PathBuf::from(display_path(&canon)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_cwd_defaults_to_current_dir() {
        let cwd = launch_cwd_from_args(vec!["app".to_string()]).unwrap();
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(cwd, PathBuf::from(display_path(&expected)));
        assert!(!cwd.display().to_string().starts_with(r"\\?\"));
    }

    #[test]
    fn launch_cwd_resolves_dot_and_rejects_missing() {
        let dot = launch_cwd_from_args(vec!["app".into(), ".".into()]).unwrap();
        assert!(dot.is_absolute());
        assert!(launch_cwd_from_args(vec!["app".into(), "n-existe-pas-xyz".into()]).is_err());
    }
}
