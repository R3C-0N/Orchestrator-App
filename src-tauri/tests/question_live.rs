//! Réponses à AskUserQuestion avec le vrai `claude.exe` (finitions du
//! prototype 1). Ignoré par défaut :
//! `cargo test -j 2 --test question_live -- --ignored --nocapture`.
//!
//! Trois sessions haiku, une par réponse, dans `.claude-work/finitions-sandbox/`
//! (`--no-session-persistence` : aucun transcript écrit). Claude doit poser une
//! question **multi-sélection** ; on répond :
//! 1. par un **tableau** de deux labels (`Answer::Many`) : le `tool_result`
//!    reçu par Claude doit contenir les deux ;
//! 2. par `skip_question(replied_in_chat)` ;
//! 3. par `skip_question(dismissed)`.
//!
//! Le texte de chaque `tool_result` est imprimé. Le flux brut de chaque
//! session est copié (format des fixtures) dans
//! `.claude-work/finitions-sandbox/logs/raw/`, pour fabriquer `t16` et
//! suivantes après anonymisation.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::events::{CoreEvent, EventBody, QuestionCloseReason, ResultPart};
use orchestrator_lib::api::types::{Answer, Answers, SessionKey, SkipReason};
use orchestrator_lib::logging;
use orchestrator_lib::process::locate::{display_path, locate_claude};
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::Session;
use orchestrator_lib::state::batcher::Sink;

const PROMPT: &str = "Appelle l'outil AskUserQuestion une seule fois, avec une seule question \
à choix multiples (multiSelect: true) : question « Quelles couleurs ? », header « Couleurs », \
options Rouge, Vert et Bleu. Après la réponse, réponds seulement : fait.";

fn sandbox() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.claude-work/finitions-sandbox");
    std::fs::create_dir_all(&p).unwrap();
    PathBuf::from(display_path(&p.canonicalize().unwrap()))
}

type Events = Arc<Mutex<Vec<CoreEvent>>>;

async fn start(name: &str, events: &Events) -> Arc<Session> {
    let ev = events.clone();
    let sink: Sink = Arc::new(move |batch| {
        ev.lock().unwrap().extend(batch);
        true
    });
    Session::start(
        SessionKey(name.into()),
        &locate_claude().expect("claude.exe"),
        &sandbox(),
        LaunchOptions {
            resume: None,
            model: Some("haiku".into()),
            extra_args: vec!["--no-session-persistence".into()],
        },
        sink,
    )
    .await
    .expect("start + initialize")
}

/// Attend le premier événement qui satisfait `f`.
async fn wait_for<T>(events: &Events, what: &str, f: impl Fn(&EventBody) -> Option<T>) -> T {
    let t0 = Instant::now();
    loop {
        if let Some(x) = events.lock().unwrap().iter().find_map(|e| f(&e.body)) {
            return x;
        }
        assert!(t0.elapsed() < Duration::from_secs(120), "attente : {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn text_of(parts: &[ResultPart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            ResultPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

enum Reply {
    Many,
    Skip(SkipReason),
}

/// Une session : question multi-sélection, réponse, `tool_result`.
/// Renvoie le texte du `tool_result`, son `is_error`, et le texte final.
async fn run(name: &str, reply: Reply) -> (String, bool, String) {
    let events: Events = Arc::default();
    let s = start(name, &events).await;
    s.send_message(PROMPT).unwrap();
    let (request_id, tool_use_id, q) = wait_for(&events, "question", |b| match b {
        EventBody::Question {
            request_id,
            tool_use_id,
            questions,
            ..
        } => Some((request_id.clone(), tool_use_id.clone(), questions.clone())),
        _ => None,
    })
    .await;
    println!("[{name}] question : {q:?}");
    assert_eq!(q.len(), 1);
    assert!(q[0].multi_select, "la question doit être multi-sélection");
    let labels: Vec<String> = q[0].options.iter().map(|o| o.label.clone()).collect();
    assert!(labels.len() >= 2);
    match reply {
        Reply::Many => {
            let mut answers = Answers::new();
            answers.insert(
                q[0].question.clone(),
                Answer::Many(vec![
                    labels[0].clone(),
                    labels[2.min(labels.len() - 1)].clone(),
                ]),
            );
            s.answer_question(&request_id, &answers, None).unwrap();
        }
        Reply::Skip(reason) => s.skip_question(&request_id, reason).unwrap(),
    }
    let tool_use_id = tool_use_id.expect("tool_use_id de la question");
    let (text, is_error) = wait_for(&events, "tool_result", |b| match b {
        EventBody::ToolResult {
            tool_use_id: id,
            content,
            is_error,
            ..
        } if *id == tool_use_id => Some((text_of(content), *is_error)),
        _ => None,
    })
    .await;
    let closed = wait_for(&events, "question_closed", |b| match b {
        EventBody::QuestionClosed { reason, .. } => Some(*reason),
        _ => None,
    })
    .await;
    let result = wait_for(&events, "turn_result", |b| match b {
        EventBody::TurnResult { result, .. } => Some(result.clone().unwrap_or_default()),
        _ => None,
    })
    .await;
    if let Some(p) = s.raw_log_path() {
        println!("[{name}] flux brut : {}", p.display());
    }
    s.stop().await;
    println!("[{name}] fermeture : {closed:?}");
    println!("[{name}] tool_result (is_error={is_error}) : {text:?}");
    println!("[{name}] texte final : {result:?}");
    match reply {
        Reply::Many => {
            assert_eq!(closed, QuestionCloseReason::Answered);
            assert!(!is_error, "tableau refusé : {text}");
            assert!(text.contains(&labels[0]), "{text}");
            assert!(text.contains(&labels[2.min(labels.len() - 1)]), "{text}");
        }
        Reply::Skip(_) => {
            assert_eq!(closed, QuestionCloseReason::Skipped);
            // Constaté en 2.1.325 (`t17`, `t18`) : même texte pour toutes
            // les raisons, sans erreur.
            assert!(!is_error, "skip refusé : {text}");
            assert_eq!(text, "The user did not answer the questions.");
        }
    }
    (text, is_error, result)
}

#[tokio::test]
#[ignore = "lance le vrai claude.exe (3 tours haiku)"]
async fn live_multi_select_and_skip_reasons() {
    // Journal et flux brut dans le bac à sable, jamais dans %LOCALAPPDATA%.
    std::env::set_var("ORCH_LOG_DIR", sandbox().join("logs"));
    std::env::remove_var("ORCH_RAW_LOG");
    assert!(logging::init().is_some(), "journal du bac à sable");

    run("live-q-many", Reply::Many).await;
    run("live-q-chat", Reply::Skip(SkipReason::ReplyInChat)).await;
    run("live-q-dismissed", Reply::Skip(SkipReason::Dismissed)).await;
}
