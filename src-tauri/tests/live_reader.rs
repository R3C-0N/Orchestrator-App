//! Lecteur et écrivain branchés sur le vrai `claude.exe` (jalon J2).
//!
//! Ignoré par défaut (réseau, coût de quelques centimes) :
//! `cargo test --test live_reader -- --ignored`. Tourne dans
//! `.claude-work/j2-sandbox/` avec `--model haiku` et
//! `--no-session-persistence` (aucun transcript écrit).

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orchestrator_lib::api::events::{CoreEvent, EventBody};
use orchestrator_lib::api::types::SessionKey;
use orchestrator_lib::process::locate::locate_claude;
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::Session;
use orchestrator_lib::state::batcher::Sink;

fn sandbox() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.claude-work/j2-sandbox");
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

#[tokio::test]
#[ignore = "lance le vrai claude.exe (réseau, coût)"]
async fn live_turn_through_reader_and_writer() {
    let exe = locate_claude().expect("claude.exe");
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let sink: Sink = {
        let events = events.clone();
        Arc::new(move |batch| {
            for ev in batch {
                if matches!(
                    ev.body,
                    EventBody::TurnResult { .. } | EventBody::Exited { .. }
                ) {
                    let _ = tx.send(());
                }
                events.lock().unwrap().push(ev);
            }
            true
        })
    };
    let session = Session::start(
        SessionKey("live-j2".into()),
        &exe,
        &sandbox(),
        LaunchOptions {
            resume: None,
            model: Some("haiku".into()),
            extra_args: vec!["--no-session-persistence".into()],
            ..LaunchOptions::default()
        },
        sink,
    )
    .await
    .expect("start + initialize");
    // Accents et caractères multioctets dans les deux sens.
    session
        .send_message("Réponds exactement : « été 😀 » et rien d'autre.")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(120), rx.recv())
        .await
        .expect("turn_result dans les 120 s");
    let info = session.stop().await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let evs = events.lock().unwrap();
    let bad: Vec<_> = evs
        .iter()
        .filter(|e| matches!(&e.body, EventBody::Notice { code, .. } if code == "bad_json"))
        .collect();
    assert!(bad.is_empty(), "lignes illisibles : {bad:?}");
    assert!(evs
        .iter()
        .any(|e| matches!(e.body, EventBody::Ready { .. })));
    let text: String = evs
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::BlockDone {
                block: orchestrator_lib::api::events::BlockDoneKind::Text { text },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect();
    println!(
        "réponse : {text:?} ; arrêt : {info:?} ; {} événements",
        evs.len()
    );
    assert!(text.contains("été"), "UTF-8 reçu intact : {text:?}");
    assert!(
        info.code == Some(0) && !info.killed,
        "arrêt propre par fermeture de stdin"
    );
}
