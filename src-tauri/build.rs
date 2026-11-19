// Déclare les commandes de l'app : tauri-build génère pour chacune une
// permission `allow-<commande>` / `deny-<commande>` (kebab-case). Une commande
// absente de la capability de la fenêtre est alors refusée à l'appel.
//
// J14 (spec, décisions 16 et 18) : injecte aussi deux constantes de
// compilation, lues par `crate::build_info` :
// - `ORCH_BUILD_COMMIT` : commit à partir duquel l'exe est compilé (le clone
//   de l'app s'y place) ; `unknown` hors dépôt git ; remplaçable par la
//   variable d'environnement du même nom ;
// - `ORCH_CONTRACT` : empreinte du contrat interface <-> cœur (FNV-1a 64 bits
//   des fichiers de `CONTRACT_FILES`, fins de ligne `\r` retirées). Le même
//   calcul est fait côté interface par `tools/contract-fingerprint.mjs`.
use std::path::{Path, PathBuf};

const COMMANDS: &[&str] = &[
    "get_launch_info",
    "start_session",
    "send_message",
    "interrupt",
    "set_model",
    "answer_question",
    "skip_question",
    "list_projects",
    "list_sessions",
    "load_history",
    "stop_session",
    "get_diagnostics",
    // J14 : auto-amélioration.
    "attach_sessions",
    "ui_ready",
    "get_ui_info",
    "improve_start",
    "improve_cancel",
    "ui_rollback",
    // J16 : onglet d'amélioration (conversation avec la session dédiée).
    "improve_send",
    "improve_validate",
    "improve_interrupt",
    "improve_dismiss",
];

/// Fichiers du contrat, relatifs à la racine du dépôt. Un dossier compte pour
/// ses fichiers `.ts` directs.
const CONTRACT_FILES: &[&str] = &[
    "src-tauri/build.rs",
    "src-tauri/capabilities/default.json",
    "src/lib/core/bindings",
];

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn contract_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for rel in CONTRACT_FILES {
        let p = root.join(rel);
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".ts") {
                        out.push((format!("{rel}/{name}"), e.path()));
                    }
                }
            }
        } else {
            out.push((rel.to_string(), p));
        }
    }
    out.sort();
    out
}

fn contract_fingerprint(root: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (rel, path) in contract_files(root) {
        println!("cargo:rerun-if-changed={}", path.display());
        let content: Vec<u8> = std::fs::read(&path)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| *b != b'\r')
            .collect();
        fnv1a(&mut h, rel.as_bytes());
        fnv1a(&mut h, &[0]);
        fnv1a(&mut h, &content);
        fnv1a(&mut h, &[0]);
    }
    format!("{h:016x}")
}

fn build_commit(root: &Path) -> String {
    println!("cargo:rerun-if-env-changed=ORCH_BUILD_COMMIT");
    if let Ok(c) = std::env::var("ORCH_BUILD_COMMIT") {
        if !c.trim().is_empty() {
            return c.trim().to_string();
        }
    }
    let git = root.join(".git");
    if git.is_dir() {
        for p in ["HEAD", "refs/heads", "packed-refs"] {
            let p = git.join(p);
            if p.exists() {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().expect("racine du dépôt").to_path_buf();
    println!("cargo:rustc-env=ORCH_BUILD_COMMIT={}", build_commit(&root));
    println!(
        "cargo:rustc-env=ORCH_CONTRACT={}",
        contract_fingerprint(&root)
    );

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("échec de tauri-build");
}
