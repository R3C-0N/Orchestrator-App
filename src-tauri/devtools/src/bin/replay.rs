//! Outil `replay` (plan J4) : rejoue les captures `fixtures/stream-json/tN.jsonl`
//! dans le réducteur et écrit les `CoreEvent` attendus dans
//! `fixtures/ui-events/tN.json` (mode simulé `?mock=tN` de l'interface, tests
//! Vitest des magasins, instantanés du test `reducer_snapshots`).
//!
//! ```text
//! cargo run --bin replay                 # régénère les 13 fichiers
//! cargo run --bin replay -- t3 t15       # seulement ces captures
//! cargo run --bin replay -- --check      # compare sans écrire (code 1 si écart)
//! cargo run --bin replay -- --stdout t3  # imprime au lieu d'écrire
//! ```
//!
//! Sans processus ni réseau : seul le réducteur pur est utilisé.

use std::path::PathBuf;
use std::process::ExitCode;

use orchestrator_lib::state::replay::{replay_fixture, to_json_lines, UI_EVENT_FIXTURES};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn main() -> ExitCode {
    let mut check = false;
    let mut stdout = false;
    let mut names = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--check" => check = true,
            "--stdout" => stdout = true,
            "-h" | "--help" => {
                eprintln!("usage : replay [--check] [--stdout] [tN …]");
                return ExitCode::SUCCESS;
            }
            n => names.push(n.trim_end_matches(".jsonl").to_string()),
        }
    }
    if names.is_empty() {
        names = UI_EVENT_FIXTURES.iter().map(|s| s.to_string()).collect();
    }
    let src = root().join("fixtures/stream-json");
    let dst = root().join("fixtures/ui-events");
    if !check && !stdout {
        if let Err(e) = std::fs::create_dir_all(&dst) {
            eprintln!("{} : {e}", dst.display());
            return ExitCode::FAILURE;
        }
    }
    let mut failed = false;
    for name in &names {
        let path = src.join(format!("{name}.jsonl"));
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{} : {e}", path.display());
                failed = true;
                continue;
            }
        };
        let rep = replay_fixture(&text, name);
        let json = to_json_lines(&rep.events);
        let out = dst.join(format!("{name}.json"));
        let summary = format!(
            "{name} : {} événements, {} réponses automatiques, {} inconnus, {} lignes illisibles",
            rep.events.len(),
            rep.writes.len(),
            rep.reducer.unknown_count,
            rep.bad_lines
        );
        if stdout {
            print!("{json}");
            eprintln!("{summary}");
        } else if check {
            let same = std::fs::read_to_string(&out)
                .map(|old| old.replace("\r\n", "\n") == json)
                .unwrap_or(false);
            if same {
                eprintln!("{summary} : à jour");
            } else {
                eprintln!("{summary} : DIFFÉRENT de {}", out.display());
                failed = true;
            }
        } else if let Err(e) = std::fs::write(&out, json) {
            eprintln!("{} : {e}", out.display());
            failed = true;
        } else {
            eprintln!("{summary} -> {}", out.display());
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
