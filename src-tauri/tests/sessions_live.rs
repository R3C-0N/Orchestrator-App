//! Reprise avec le vrai `claude.exe` (J11, critère de fin, comme `t7`).
//! Ignoré par défaut : `cargo test --test sessions_live -- --ignored
//! --nocapture`. Deux tours en haiku, dans `.claude-work/j11-sandbox/` :
//! le premier donne un mot à retenir, puis la session est arrêtée, listée,
//! son historique relu, et reprise ; le second tour doit retrouver le mot.
//! Le transcript est écrit par le CLI lui-même ; ce test ne fait que le lire.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::events::{BlockDoneKind, CoreEvent, EventBody, UserOrigin};
use orchestrator_lib::api::types::SessionKey;
use orchestrator_lib::process::locate::{display_path, locate_claude};
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::Session;
use orchestrator_lib::sessions::list::list_sessions;
use orchestrator_lib::sessions::paths::{find_project_dir, projects_root};
use orchestrator_lib::sessions::transcript::load_history;
use orchestrator_lib::state::batcher::Sink;

fn sandbox() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.claude-work/j11-sandbox");
    std::fs::create_dir_all(&p).unwrap();
    PathBuf::from(display_path(&p.canonicalize().unwrap()))
}

type Events = Arc<Mutex<Vec<CoreEvent>>>;

async fn start(resume: Option<String>, events: &Events) -> Arc<Session> {
    let ev = events.clone();
    let sink: Sink = Arc::new(move |batch| {
        ev.lock().unwrap().extend(batch);
        true
    });
    Session::start(
        SessionKey("live-j11".into()),
        &locate_claude().expect("claude.exe"),
        &sandbox(),
        LaunchOptions {
            resume,
            model: Some("haiku".into()),
            extra_args: vec![],
        },
        sink,
    )
    .await
    .expect("start + initialize")
}

/// Attend un `turn_result` après `from` événements ; renvoie son texte.
async fn turn(events: &Events, from: usize) -> String {
    let t0 = Instant::now();
    loop {
        if let Some(r) = events.lock().unwrap()[from..]
            .iter()
            .find_map(|e| match &e.body {
                EventBody::TurnResult { result, .. } => Some(result.clone().unwrap_or_default()),
                _ => None,
            })
        {
            return r;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(120),
            "pas de fin de tour"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "lance le vrai claude.exe (2 tours haiku)"]
async fn live_resume_shows_history_and_keeps_context() {
    let events: Events = Arc::default();
    let s = start(None, &events).await;
    s.send_message("Retiens ce mot de code : MANDARINE-42. Réponds seulement OK.")
        .unwrap();
    turn(&events, 0).await;
    let session_id = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Init { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .expect("system/init");
    s.stop().await;
    println!("session {session_id}");

    // Liste des sessions du dossier.
    let root = projects_root().expect("~/.claude/projects");
    let cwd = sandbox().display().to_string();
    let t0 = Instant::now();
    let list = list_sessions(&root, &cwd);
    println!(
        "list_sessions : {} sessions en {:?}",
        list.len(),
        t0.elapsed()
    );
    assert!(list.iter().any(|x| x.id == session_id));

    // Historique relu.
    let dir = find_project_dir(&root, &cwd).expect("dossier du projet");
    let h = load_history(&dir, &session_id, &SessionKey("live-j11".into())).expect("historique");
    println!(
        "historique : {} événements, {} lignes, {} ignorées ({} inconnues), {} illisibles",
        h.events.len(),
        h.lines,
        h.ignored,
        h.unknown,
        h.bad
    );
    assert_eq!(h.bad, 0);
    assert!(h.events.iter().any(|e| matches!(&e.body,
        EventBody::UserMessage { text, origin: UserOrigin::History, .. } if text.contains("MANDARINE-42"))));
    assert!(h.events.iter().any(|e| matches!(
        &e.body,
        EventBody::BlockDone {
            block: BlockDoneKind::Text { .. },
            ..
        }
    )));

    // Reprise : le contexte est gardé.
    let events2: Events = Arc::default();
    let s2 = start(Some(session_id.clone()), &events2).await;
    // J14 : le premier historique chargé est gardé par la session et rendu
    // tel quel au rattachement, même si le transcript grossit ensuite.
    assert_eq!(s2.resumed_from.as_deref(), Some(session_id.as_str()));
    assert!(s2.cached_history(&session_id).is_none());
    assert_eq!(s2.keep_history(h.clone()), h);
    let from = events2.lock().unwrap().len();
    s2.send_message("Quel est le mot de code ? Réponds seulement le mot.")
        .unwrap();
    let answer = turn(&events2, from).await;
    let reread = load_history(&dir, &session_id, &SessionKey("live-j11".into())).expect("relu");
    assert!(
        reread.events.len() > h.events.len(),
        "le transcript a grossi"
    );
    assert_eq!(
        s2.cached_history(&session_id),
        Some(h.clone()),
        "cache inchangé"
    );
    let r = s2.resync(Arc::new(|_| true), 1);
    assert_eq!(r.history, Some(h));
    assert_eq!(r.pid, s2.pid);
    s2.stop().await;
    println!("réponse après reprise : {answer:?}");
    assert!(answer.contains("MANDARINE-42"));
}
