//! Mode sans interface : le même cœur que l'app, sans fenêtre.
//!
//! Lance `claude.exe` (commande de la spec 4.1 + options), envoie `initialize`,
//! puis un message, et imprime chaque `CoreEvent` en JSON (une ligne chacun)
//! jusqu'au `turn_result`. Sert de preuve du tuyau et d'outil de dev.
//!
//! Usage :
//!   probe [--cwd DIR] [--model M] [--message TEXTE] [--no-persist] [--hold]
//!
//! `--hold` : n'envoie aucun message, imprime `CLAUDE_PID <pid>` et attend
//! indéfiniment (sert au test « tuer l'app tue claude.exe »).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use orchestrator_lib::api::events::EventBody;
use orchestrator_lib::api::types::SessionKey;
use orchestrator_lib::process::locate::{launch_cwd_from_args, locate_claude};
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::Session;
use orchestrator_lib::state::batcher::Sink;

#[tokio::main]
async fn main() {
    let mut cwd: Option<PathBuf> = None;
    let mut model = None;
    let mut message = "Dis bonjour en un mot.".to_string();
    let mut extra = vec![];
    let mut hold = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cwd" => cwd = it.next().map(PathBuf::from),
            "--model" => model = it.next(),
            "--message" => message = it.next().unwrap_or_default(),
            "--no-persist" => extra.push("--no-session-persistence".to_string()),
            "--hold" => hold = true,
            other => {
                eprintln!("argument inconnu : {other}");
                std::process::exit(2);
            }
        }
    }
    let cwd = match cwd {
        Some(c) => launch_cwd_from_args(vec!["probe".into(), c.display().to_string()]),
        None => launch_cwd_from_args(vec!["probe".to_string()]),
    }
    .expect("dossier de travail invalide");
    let exe = locate_claude().unwrap_or_else(|tried| {
        eprintln!("claude.exe introuvable : {}", tried.join(" ; "));
        std::process::exit(3);
    });

    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let sink: Sink = Arc::new(move |batch| {
        for ev in batch {
            // Lecteur parti (test qui a lu le pid) : on ignore l'erreur.
            let _ = writeln!(
                std::io::stdout().lock(),
                "{}",
                serde_json::to_string(&ev).unwrap()
            );
            if matches!(
                ev.body,
                EventBody::TurnResult { .. } | EventBody::Exited { .. }
            ) {
                let _ = done_tx.send(());
            }
        }
        true
    });

    let session = match Session::start(
        SessionKey("probe".into()),
        &exe,
        &cwd,
        LaunchOptions {
            resume: None,
            model,
            extra_args: extra,
            ..LaunchOptions::default()
        },
        sink,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("échec du démarrage : {e}");
            std::process::exit(4);
        }
    };

    if hold {
        println!("CLAUDE_PID {}", session.pid);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }

    session.send_message(&message).expect("envoi du message");
    let _ = done_rx.recv().await;
    let info = session.stop().await;
    // Laisser le regroupeur vider ses derniers lots.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    eprintln!("arrêt : code={:?} tué={}", info.code, info.killed);
}
