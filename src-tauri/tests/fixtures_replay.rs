//! Rejeu des fixtures `fixtures/stream-json/t*.jsonl` (plan 1.10, points 1 et
//! 2) : lecture par le lecteur de lignes, décodage par `protocol::inbound`,
//! passage dans le réducteur, et comparaison des lignes sortantes aux lignes
//! `>>> ` du pilote. Sans processus, sans réseau.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use orchestrator_lib::api::events::{BlockDoneKind, EventBody, NoticeLevel};
use orchestrator_lib::api::types::{Answer, SkipReason};
use orchestrator_lib::process::reader::{LineSplitter, RawLine, MAX_LINE_BYTES};
use orchestrator_lib::protocol::control::{InControlRequestBody, OutControlRequest};
use orchestrator_lib::protocol::fixture::{classify, FixtureLine};
use orchestrator_lib::protocol::inbound::Inbound;
use orchestrator_lib::protocol::outbound;
use orchestrator_lib::state::reducer::{Action, Reducer};
use serde_json::{Map, Value};

/// Captures attendues : t1, t3 à t18 (il n'y a pas de t2).
const FIXTURES: &[&str] = &[
    "t1", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10", "t11", "t12", "t13", "t14", "t15",
    "t16", "t17", "t18",
];

/// Types inconnus tolérés au rejeu : aucun. Tout ce que les captures
/// contiennent est décodé (les types ignorés par choix sont listés dans
/// `inbound::IGNORED_TYPES` et `IGNORED_SYSTEM_SUBTYPES`, et ne sont pas des
/// inconnus).
const ALLOWED_UNKNOWN: &[&str] = &[];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/stream-json")
}

/// Lignes d'une capture, découpées par le lecteur de l'app (morceaux de 4 Kio,
/// pour traverser des frontières de lecture au milieu des lignes).
fn raw_lines(name: &str) -> Vec<String> {
    let bytes = std::fs::read(fixtures_dir().join(name)).expect("fixture");
    let mut s = LineSplitter::new(MAX_LINE_BYTES);
    let mut out = VecDeque::new();
    for chunk in bytes.chunks(4096) {
        s.feed(chunk, &mut out);
    }
    out.extend(s.finish());
    out.into_iter()
        .map(|l| match l {
            RawLine::Line(l) => l,
            RawLine::TooLong { len, .. } => panic!("{name} : ligne de {len} octets"),
        })
        .collect()
}

/// Lignes du CLI (les lignes `>>> `, `STDERR`, `EXIT` du pilote sont écartées)
/// et envois du pilote (texte brut, pour la comparaison à l'octet près).
fn load_text(name: &str) -> (Vec<String>, Vec<String>) {
    let mut cli = Vec::new();
    let mut sent = Vec::new();
    for line in raw_lines(name) {
        match classify(&line) {
            FixtureLine::Cli(l) => cli.push(l.to_string()),
            FixtureLine::Sent(l) => sent.push(l.to_string()),
            FixtureLine::Stderr(_) | FixtureLine::Exit(_) | FixtureLine::Blank => {}
        }
    }
    (cli, sent)
}

fn load(name: &str) -> (Vec<Value>, Vec<Value>) {
    let (cli, sent) = load_text(name);
    let parse =
        |l: &String| -> Value { serde_json::from_str(l).unwrap_or_else(|e| panic!("{name}: {e}")) };
    (
        cli.iter().map(parse).collect(),
        sent.iter().map(parse).collect(),
    )
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

// ---------------------------------------------------------------------------
// Partie lecture (jalon J2) : décodage par `protocol::inbound`.

#[test]
fn all_fixtures_are_present() {
    for f in FIXTURES {
        assert!(
            fixtures_dir().join(format!("{f}.jsonl")).is_file(),
            "{f}.jsonl manquant"
        );
    }
}

#[test]
fn every_cli_line_decodes_without_unknown() {
    let mut total = 0;
    for f in FIXTURES {
        let name = format!("{f}.jsonl");
        let (cli, sent) = load_text(&name);
        assert!(!cli.is_empty(), "{name} vide");
        for (i, line) in cli.iter().enumerate() {
            let m = Inbound::from_line(line)
                .unwrap_or_else(|e| panic!("{name} ligne CLI {} : JSON illisible ({e})", i + 1));
            let unknown: Vec<String> = m
                .unknown_kinds()
                .into_iter()
                .filter(|k| !ALLOWED_UNKNOWN.contains(&k.as_str()))
                .collect();
            assert!(
                unknown.is_empty(),
                "{name} ligne CLI {} : inconnu {unknown:?}",
                i + 1
            );
            assert!(
                !matches!(m, Inbound::Unknown(_) | Inbound::Ignored(_)),
                "{name} ligne CLI {} : non décodée",
                i + 1
            );
        }
        // Les envois du pilote sont du JSON valide, eux aussi.
        for l in &sent {
            serde_json::from_str::<Value>(l).expect("ligne >>> JSON");
        }
        println!(
            "{name} : {} lignes CLI décodées, {} lignes >>>",
            cli.len(),
            sent.len()
        );
        total += cli.len();
    }
    println!("total : {total} lignes CLI décodées");
}

/// Requête `can_use_tool` d'une capture : (request_id, input).
fn can_use_tool(name: &str, tool: &str) -> (String, Value) {
    let (cli, _) = load_text(name);
    cli.iter()
        .find_map(|l| match Inbound::from_line(l).unwrap() {
            Inbound::ControlRequest(r) => match r.request {
                InControlRequestBody::CanUseTool(c) if c.tool_name.as_deref() == Some(tool) => {
                    Some((r.request_id.unwrap(), c.input.unwrap()))
                }
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name} : pas de can_use_tool {tool}"))
}

fn sent_lines(name: &str) -> Vec<String> {
    load_text(name).1
}

fn assert_sent(name: &str, ours: &str) {
    let sent = sent_lines(name);
    assert!(
        sent.iter().any(|l| l == ours),
        "{name} : ligne produite absente des >>> (à l'octet près)\n  produite : {ours}\n  >>> : {sent:#?}"
    );
}

#[test]
fn user_messages_match_driver_bytes() {
    let mut n = 0;
    for f in FIXTURES {
        let name = format!("{f}.jsonl");
        for l in sent_lines(&name) {
            let v: Value = serde_json::from_str(&l).unwrap();
            if v["type"] == "user" {
                let text = v["message"]["content"].as_str().unwrap();
                assert_eq!(outbound::user_message(text), l, "{name}");
                n += 1;
            }
        }
    }
    println!("{n} messages utilisateur identiques à l'octet près");
    assert!(n >= 1000 + 11);
}

#[test]
fn control_requests_match_driver_bytes() {
    // t11 : initialize ; t5 : interrupt, set_model ; t6 : interrupt.
    assert_sent(
        "t11.jsonl",
        &outbound::control_request_typed("init-1", OutControlRequest::Initialize {}),
    );
    assert_sent(
        "t11.jsonl",
        &outbound::control_request("init-1", "initialize", Map::new()),
    );
    assert_sent(
        "t5.jsonl",
        &outbound::control_request_typed(
            "int-1",
            OutControlRequest::Interrupt {
                cancel_queued: None,
            },
        ),
    );
    let mut extra = Map::new();
    extra.insert("model".into(), Value::String("sonnet".into()));
    assert_sent(
        "t5.jsonl",
        &outbound::control_request("mdl-1", "set_model", extra),
    );
    assert_sent(
        "t5.jsonl",
        &outbound::control_request_typed(
            "mdl-1",
            OutControlRequest::SetModel {
                model: "sonnet".into(),
            },
        ),
    );
    assert_sent(
        "t6.jsonl",
        &outbound::control_request("int-2", "interrupt", Map::new()),
    );
}

#[test]
fn control_responses_match_driver_bytes() {
    // t4 : allow du Bash, puis réponse à la question.
    let (id, input) = can_use_tool("t4.jsonl", "Bash");
    assert_sent("t4.jsonl", &outbound::allow(&id, &input));
    let (id, input) = can_use_tool("t4.jsonl", "AskUserQuestion");
    let answers: BTreeMap<String, Answer> = [(
        "Quelle est ta couleur préférée ?".to_string(),
        Answer::One("Bleu".into()),
    )]
    .into();
    assert_sent(
        "t4.jsonl",
        &outbound::answer_question(&id, &input, &answers, None),
    );
    // t8 : réponse à la question.
    let (id, input) = can_use_tool("t8.jsonl", "AskUserQuestion");
    let answers: BTreeMap<String, Answer> = [(
        "Quelle est votre couleur préférée ?".to_string(),
        Answer::One("Bleu".into()),
    )]
    .into();
    assert_sent(
        "t8.jsonl",
        &outbound::answer_question(&id, &input, &answers, None),
    );
    // t9 : refus.
    let (id, _) = can_use_tool("t9.jsonl", "Bash");
    assert_sent(
        "t9.jsonl",
        &outbound::deny(&id, "Refusé par l'utilisateur (test)"),
    );
    // t10 : « ignorer », à l'octet près (plan 1.10).
    let (id, input) = can_use_tool("t10.jsonl", "AskUserQuestion");
    assert_sent(
        "t10.jsonl",
        &outbound::skip_question(&id, &input, SkipReason::Skipped),
    );
    // t16 : multi-sélection, tableau de deux labels.
    let (id, input) = can_use_tool("t16.jsonl", "AskUserQuestion");
    let answers: BTreeMap<String, Answer> = [(
        "Quelles couleurs ?".to_string(),
        Answer::Many(vec!["Rouge".into(), "Bleu".into()]),
    )]
    .into();
    assert_sent(
        "t16.jsonl",
        &outbound::answer_question(&id, &input, &answers, None),
    );
    // t17 : « Chat about this » (`replied_in_chat`) ; t18 : `dismissed`.
    let (id, input) = can_use_tool("t17.jsonl", "AskUserQuestion");
    assert_sent(
        "t17.jsonl",
        &outbound::skip_question(&id, &input, SkipReason::ReplyInChat),
    );
    let (id, input) = can_use_tool("t18.jsonl", "AskUserQuestion");
    assert_sent(
        "t18.jsonl",
        &outbound::skip_question(&id, &input, SkipReason::Dismissed),
    );
}

/// Les deux chemins de décodage (ligne de texte, valeur déjà lue) donnent le
/// même message sur toutes les lignes de toutes les captures.
#[test]
fn from_line_equals_from_value() {
    let mut n = 0;
    for f in FIXTURES {
        let (cli, _) = load_text(&format!("{f}.jsonl"));
        for l in &cli {
            let v: Value = serde_json::from_str(l).unwrap();
            assert_eq!(
                Inbound::from_line(l).unwrap(),
                Inbound::from_value(&v),
                "{f} : {l}"
            );
            n += 1;
        }
    }
    for l in [
        "[]",
        "5",
        "null",
        r#"{"type":5}"#,
        r#"{"type":"system"}"#,
        r#"{"type":"assistant","message":42}"#,
        r#"{"type":"control_request"}"#,
        r#"{"type":"user","message":{"content":[1,{"type":"text","text":7}]}}"#,
        r#"{"type":"result","usage":null,"modelUsage":{"a":3}}"#,
        r#"{"type":"stream_event","event":[1,2]}"#,
        // Clés en double : la dernière gagne, comme avec `Value`.
        r#"{"type":"result","subtype":"a","subtype":"b","is_error":true}"#,
        r#"{"type":"assistant","type":"result","subtype":"x"}"#,
        r#"{"type":"result","type":"assistant","message":{"id":"m","content":[]}}"#,
        r#"{"type":"system","subtype":"init","subtype":"status","status":"requesting"}"#,
        r#"{"type":"control_request","request_id":"a","request_id":"b","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#,
        r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","tool_name":"A","tool_name":"Bash","input":{}}}"#,
        r#"{"type":"stream_event","api_message_id":"m1","api_message_id":"m2","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"index":2,"delta":{"type":"text_delta","text":"x"}}}"#,
        r#"{"type":"assistant","message":{"id":"a","id":"b","content":[{"type":"text","text":"t","text":"u"}]}}"#,
        // Échappements `\u…` (é, 😀 en paire de substitution) et caractère nul.
        // `concat!` coupe chaque `\u` pour que l'échappement reste littéral.
        concat!(
            r#"{"type":"result","subtype":""#,
            "\\",
            "u00e9",
            "\\",
            "ud83d",
            "\\",
            r#"ude00"}"#
        ),
        concat!(r#"{"type":"result","result":"a"#, "\\", r#"u0000b"}"#),
        concat!(
            r#"{"type":"user","message":{"content":""#,
            "\\",
            "u0000",
            "\\",
            r#"u00e9"}}"#
        ),
    ] {
        let v: Value = serde_json::from_str(l).unwrap();
        assert_eq!(
            Inbound::from_line(l).unwrap(),
            Inbound::from_value(&v),
            "{l}"
        );
        n += 1;
    }
    // Nombre hors plage : erreur des deux côtés (`bad_json` en production).
    for l in [
        r#"{"type":"result","total_cost_usd":1e400}"#,
        r#"{"type":"assistant","message":{"usage":{"x":-1e400}}}"#,
        r#"{"type":"result","subtype":"a","subtype":"b","duration_ms":1e999}"#,
    ] {
        assert!(serde_json::from_str::<Value>(l).is_err(), "{l}");
        assert!(
            Inbound::from_line(l).is_err(),
            "from_line doit échouer : {l}"
        );
        n += 1;
    }
    println!("{n} lignes : from_line = from_value");
}

/// Chemin de production (`from_line` puis `handle_inbound`, celui du
/// superviseur) comparé au chemin `from_value` puis `handle`, sur toutes les
/// captures : mêmes actions, même état final du réducteur.
#[test]
fn production_path_replays_like_value_path() {
    let mut total = 0;
    for f in FIXTURES {
        let name = format!("{f}.jsonl");
        let (cli, _) = load_text(&name);
        let mut prod = Reducer::new();
        let mut reference = Reducer::new();
        prod.expect_initialize("init-1");
        reference.expect_initialize("init-1");
        for (i, l) in cli.iter().enumerate() {
            // Horodatage fixe des deux côtés : le test ne dépend pas de l'horloge.
            const T: u64 = 1_700_000_000_000;
            let a = prod.handle_inbound_at(&Inbound::from_line(l).unwrap(), T);
            let b = reference.handle_at(&serde_json::from_str::<Value>(l).unwrap(), T);
            assert_eq!(a, b, "{name} ligne CLI {}", i + 1);
        }
        assert_eq!(prod.unknown_count, reference.unknown_count, "{name}");
        assert_eq!(prod.capabilities, reference.capabilities, "{name}");
        assert_eq!(prod.version, reference.version, "{name}");
        assert_eq!(prod.questions, reference.questions, "{name}");
        assert_eq!(prod.state, reference.state, "{name}");
        println!(
            "{name} : {} lignes, chemin de production = chemin Value",
            cli.len()
        );
        total += cli.len();
    }
    println!("total : {total} lignes");
}

/// Les cas relevés par la relecture, sur le chemin de production.
#[test]
fn duplicate_keys_on_production_path() {
    let run = |l: &str| Reducer::new().handle_inbound(&Inbound::from_line(l).unwrap());
    let a = run(r#"{"type":"result","subtype":"a","subtype":"b","is_error":true}"#);
    assert!(
        matches!(&a[0], Action::Emit(EventBody::TurnResult { subtype, is_error: true, .. }) if subtype == "b"),
        "{a:?}"
    );
    let a = run(
        r#"{"type":"control_request","request_id":"a","request_id":"b","request":{"subtype":"hook_callback"}}"#,
    );
    let Action::Write(w) = &a[0] else {
        panic!("{a:?}")
    };
    assert_eq!(
        serde_json::from_str::<Value>(w).unwrap()["response"]["request_id"],
        "b"
    );
    // Échappements décodés (é, 😀 en paire de substitution).
    let l = concat!(
        r#"{"type":"result","subtype":""#,
        "\\",
        "u00e9",
        "\\",
        "ud83d",
        "\\",
        r#"ude00"}"#
    );
    let a = run(l);
    assert!(
        matches!(&a[0], Action::Emit(EventBody::TurnResult { subtype, .. }) if subtype == "\u{e9}\u{1f600}"),
        "{a:?}"
    );
}

/// Lecture + décodage + réducteur sur `t15` (10 505 lignes, 6,9 Mo). Le seuil
/// du plan (≥ 50 000 lignes/s) n'est vérifié qu'en build optimisé
/// (`cargo test --release`) ; en debug, le débit est seulement affiché.
#[test]
fn throughput_on_t15() {
    let bytes = std::fs::read(fixtures_dir().join("t15.jsonl")).unwrap();
    let rounds = if cfg!(debug_assertions) { 1 } else { 5 };
    let start = Instant::now();
    let mut lines = 0usize;
    for _ in 0..rounds {
        let mut s = LineSplitter::new(MAX_LINE_BYTES);
        let mut out = VecDeque::new();
        let mut r = Reducer::new();
        for chunk in bytes.chunks(64 * 1024) {
            s.feed(chunk, &mut out);
            while let Some(RawLine::Line(l)) = out.pop_front() {
                lines += 1;
                if let FixtureLine::Cli(l) = classify(&l) {
                    // Chemin du superviseur : ligne -> Inbound -> réducteur.
                    r.handle_inbound(&Inbound::from_line(l).unwrap());
                }
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let rate = lines as f64 / secs;
    println!("t15 : {lines} lignes en {secs:.3} s, {rate:.0} lignes/s");
    if !cfg!(debug_assertions) {
        assert!(rate >= 50_000.0, "débit {rate:.0} lignes/s < 50 000");
    }
}
