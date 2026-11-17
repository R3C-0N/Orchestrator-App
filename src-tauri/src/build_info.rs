//! Constantes de compilation injectées par `build.rs` (J14).

/// Commit à partir duquel l'exe a été compilé (`unknown` hors dépôt git).
/// Le clone de l'app se place sur ce commit (spec, décision 16).
pub const BUILD_COMMIT: &str = env!("ORCH_BUILD_COMMIT");

/// Empreinte du contrat interface <-> cœur (spec, décision 18). Même calcul
/// que `tools/contract-fingerprint.mjs`.
pub const CONTRACT: &str = env!("ORCH_CONTRACT");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_set() {
        assert!(!BUILD_COMMIT.is_empty());
        assert_eq!(CONTRACT.len(), 16);
        assert!(CONTRACT.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Le calcul de Node donne la même empreinte (sauté si `node` manque).
    #[test]
    fn node_fingerprint_matches() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let Ok(out) = std::process::Command::new("node")
            .arg("tools/contract-fingerprint.mjs")
            .current_dir(&root)
            .output()
        else {
            eprintln!("node absent : test sauté");
            return;
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), CONTRACT);
    }
}
