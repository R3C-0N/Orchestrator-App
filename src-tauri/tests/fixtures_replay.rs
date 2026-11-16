//! Rejeu des fixtures `fixtures/stream-json/t*.jsonl` dans le réducteur
//! (plan 1.10, points 1 et 2). Sans processus, sans réseau.

use std::path::PathBuf;

use orchestrator_lib::api::events::{BlockDoneKind, EventBody, NoticeLevel};
use orchestrator_lib::state::reducer::{Action, Reducer};
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/stream-json")
}

/// Lignes du CLI (les lignes `>>> `, `STDERR`, `EXIT` du pilote sont écartées)
/// et envois du pilote.
fn load(name: &str) -> (Vec<Value>, Vec<Value>) {
    let text = std::fs::read_to_string(fixtures_dir().join(name)).expect("fixture");
    let mut cli = Vec::new();
    let mut sent = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(">>> ") {
            sent.push(serde_json::from_str(rest).expect("envoi JSON"));
        } else if line.starts_with('{') {
            cli.push(serde_json::from_str(line).unwrap_or_else(|e| panic!("{name}: {e}")));
        }
    }
    (cli, sent)
}

fn replay(name: &str, init_id: Option<&str>) -> (Reducer, Vec<Action>) {
    let (cli, _) = load(name);
    let mut r = Reducer::new();
    if let Some(id) = init_id {
        r.expect_initialize(id);
    }
    let mut all = Vec::new();
    for v in &cli {
        all.extend(r.handle(v));
    }
    (r, all)
}

fn emitted(actions: &[Action]) -> Vec<&EventBody> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Emit(b) => Some(b),
            _ => None,
        })
        .collect()
}

#[test]
fn every_fixture_replays_without_panic_or_unknown() {
    let mut names: Vec<_> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    names.sort();
    assert!(names.len() >= 11, "fixtures trouvées : {names:?}");
    for n in &names {
        let (r, actions) = replay(n, None);
        assert_eq!(r.unknown_count, 0, "{n} : types inconnus");
        // Tous les événements se sérialisent (contrat).
        for b in emitted(&actions) {
            serde_json::to_string(b).unwrap();
        }
    }
}

#[test]
fn t1_streams_text_and_final_block_matches_deltas() {
    let (_, actions) = replay("t1.jsonl", None);
    let ev = emitted(&actions);
    let deltas: String = ev
        .iter()
        .filter_map(|b| match b {
            EventBody::TextDelta { text, msg, .. } if msg.0 == first_text_msg(&ev) => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    let done = ev
        .iter()
        .find_map(|b| match b {
            EventBody::BlockDone {
                block: BlockDoneKind::Text { text },
                msg,
                ..
            } if msg.0 == first_text_msg(&ev) => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!deltas.is_empty());
    assert_eq!(deltas, done, "le texte des deltas = texte du bloc complet");
    let results = ev
        .iter()
        .filter(|b| matches!(b, EventBody::TurnResult { .. }))
        .count();
    assert_eq!(results, 2, "t1 contient 2 tours");
    let inits = ev
        .iter()
        .filter(|b| matches!(b, EventBody::Init { .. }))
        .count();
    assert_eq!(inits, 2);
}

fn first_text_msg(ev: &[&EventBody]) -> String {
    ev.iter()
        .find_map(|b| match b {
            EventBody::TextDelta { msg, .. } => Some(msg.0.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn t4_auto_allows_bash_like_the_driver_and_relays_question() {
    let (_, sent) = load("t4.jsonl");
    let (_, actions) = replay("t4.jsonl", None);
    let writes: Vec<Value> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Write(l) => Some(serde_json::from_str(l).unwrap()),
            _ => None,
        })
        .collect();
    // Une seule réponse automatique : le Bash. AskUserQuestion part à l'interface.
    assert_eq!(writes.len(), 1, "{writes:?}");
    let driver_allow = sent
        .iter()
        .find(|v| {
            v["type"] == "control_response"
                && v["response"]["response"]["updatedInput"]
                    .get("command")
                    .is_some()
        })
        .expect("allow Bash du pilote");
    assert_eq!(
        &writes[0], driver_allow,
        "allow identique à la ligne >>> du pilote"
    );
    let ev = emitted(&actions);
    let q = ev
        .iter()
        .find_map(|b| match b {
            EventBody::Question { questions, .. } => Some(questions.clone()),
            _ => None,
        })
        .expect("question relayée");
    assert_eq!(q[0].options.len(), 2);
    assert!(ev.iter().any(|b| matches!(b, EventBody::Notice { level: NoticeLevel::Info, code, .. } if code == "auto_allow")));
}

#[test]
fn t11_initialize_response_produces_ready() {
    let (_, actions) = replay("t11.jsonl", Some("init-1"));
    let ev = emitted(&actions);
    let ready = ev
        .iter()
        .find_map(|b| match b {
            EventBody::Ready {
                version,
                commands,
                agents,
                ..
            } => Some((version.clone(), commands.len(), agents.clone())),
            _ => None,
        })
        .expect("ready");
    assert_eq!(ready.0, "2.1.325");
    assert_eq!(ready.1, 108);
    assert_eq!(ready.2.len(), 22);
    // Noms d'agents débarrassés de U+00A0 (spec 4.8).
    assert!(ready.2.iter().all(|a| a.name == a.name.trim()));
    // Version attendue : aucun avertissement.
    assert!(!ev
        .iter()
        .any(|b| matches!(b, EventBody::Notice { code, .. } if code == "unexpected_version")));
    assert!(actions.iter().any(
        |a| matches!(a, Action::Resolve { request_id, result: Ok(_) } if request_id == "init-1")
    ));
}

#[test]
fn other_version_warns_once_without_blocking() {
    let mut r = Reducer::new();
    let init = serde_json::json!({"type":"system","subtype":"init","claude_code_version":"9.9.9","session_id":"x"});
    let a1 = r.handle(&init);
    let a2 = r.handle(&init);
    let warns = |a: &[Action]| {
        emitted(a)
            .iter()
            .filter(|b| matches!(b, EventBody::Notice { level: NoticeLevel::Warn, code, .. } if code == "unexpected_version"))
            .count()
    };
    assert_eq!(warns(&a1), 1);
    assert_eq!(warns(&a2), 0);
    assert!(emitted(&a2)
        .iter()
        .any(|b| matches!(b, EventBody::Init { .. })));
}

#[test]
fn unknown_and_garbage_messages_are_ignored() {
    let mut r = Reducer::new();
    assert!(r
        .handle(&serde_json::json!({"type":"brand_new_thing","x":1}))
        .is_empty());
    assert!(r.handle(&serde_json::json!({"no_type":true})).is_empty());
    assert!(r
        .handle(&serde_json::json!({"type":"stream_event"}))
        .is_empty());
    assert!(r
        .handle(&serde_json::json!({"type":"assistant","message":42}))
        .is_empty());
    assert_eq!(r.unknown_count, 2);
}

#[test]
fn other_control_requests_get_an_error_response() {
    let mut r = Reducer::new();
    let a = r.handle(&serde_json::json!({"type":"control_request","request_id":"x1","request":{"subtype":"hook_callback"}}));
    let w = a.iter().find_map(|a| match a {
        Action::Write(l) => Some(serde_json::from_str::<Value>(l).unwrap()),
        _ => None,
    });
    let w = w.expect("réponse écrite");
    assert_eq!(w["response"]["subtype"], "error");
    assert_eq!(w["response"]["request_id"], "x1");
}

#[test]
fn t6_interrupted_turn_is_reported() {
    let (_, actions) = replay("t6.jsonl", None);
    let ev = emitted(&actions);
    assert!(ev.iter().any(|b| matches!(b, EventBody::TurnResult { terminal_reason: Some(r), .. } if r == "aborted_streaming")));
    assert!(ev.iter().any(|b| matches!(
        b,
        EventBody::UserMessage {
            origin: orchestrator_lib::api::events::UserOrigin::Interrupted,
            ..
        }
    )));
}
