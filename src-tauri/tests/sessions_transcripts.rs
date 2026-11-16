//! J11 : lecteur de transcript (décision 9, plan 1.8 et 1.10 point 4) sur
//! les extraits **synthétiques** de `fixtures/transcripts/projects/`
//! (régénérables par `node fixtures/transcripts/generate.mjs
//! fixtures/transcripts`).
//!
//! `cargo test transcripts` : à relancer à chaque nouvelle version du CLI.

use std::path::PathBuf;

use orchestrator_lib::api::events::{
    AgentEvent, AgentStatus, BlockDoneKind, EventBody, NoticeLevel, UserOrigin,
};
use orchestrator_lib::api::types::{CoreError, HistoryLoaded, SessionKey};
use orchestrator_lib::sessions::paths::find_project_dir;
use orchestrator_lib::sessions::transcript::{history_from_bytes, load_history, read_subagents};

const CWD: &str = r"C:\Users\<user>\Code\demo";
const FULL: &str = "0b8f3c2e-1111-4a6b-9c1d-000000000001";
const TRUNCATED: &str = "0b8f3c2e-1111-4a6b-9c1d-000000000002";
const UNREADABLE: &str = "0b8f3c2e-1111-4a6b-9c1d-000000000003";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/transcripts/projects")
}

fn key() -> SessionKey {
    SessionKey("s-test".into())
}

fn project() -> PathBuf {
    find_project_dir(&root(), CWD).expect("dossier du projet de démo")
}

fn load(id: &str) -> Result<HistoryLoaded, CoreError> {
    load_history(&project(), id, &key())
}

fn bodies(h: &HistoryLoaded) -> Vec<&EventBody> {
    h.events.iter().map(|e| &e.body).collect()
}

fn users(h: &HistoryLoaded) -> Vec<(String, UserOrigin)> {
    h.events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::UserMessage { text, origin, .. } => Some((text.clone(), *origin)),
            _ => None,
        })
        .collect()
}

fn texts(h: &HistoryLoaded) -> Vec<String> {
    h.events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::BlockDone {
                block: BlockDoneKind::Text { text },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Dernier état de chaque agent (l'interface remplace sa copie).
fn last_agents(h: &HistoryLoaded) -> Vec<AgentEvent> {
    let mut out: Vec<AgentEvent> = Vec::new();
    for e in &h.events {
        if let EventBody::Agent(a) = &e.body {
            match out.iter_mut().find(|x| x.tool_use_id == a.tool_use_id) {
                Some(x) => *x = a.clone(),
                None => out.push(a.clone()),
            }
        }
    }
    out
}

#[test]
fn transcripts_full_history_is_replayed_through_the_reducer() {
    let h = load(FULL).unwrap();
    assert_eq!(h.session_id, FULL);
    // Numérotation continue à partir de 0, clé de session de l'app.
    for (i, e) in h.events.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
        assert_eq!(e.session, key());
    }
    // Aucun état de tour dans l'historique.
    assert!(!bodies(&h)
        .iter()
        .any(|b| matches!(b, EventBody::Status { .. })));

    // Messages utilisateur : tapés (`History`), commande, interruption ; ni
    // `isMeta`, ni `<system-reminder>`, ni sortie de commande locale, ni
    // notification de tâche.
    assert_eq!(
        users(&h),
        vec![
            (
                "Lis le fichier README et résume-le.".into(),
                UserOrigin::History
            ),
            ("/model haiku".into(), UserOrigin::History),
            (
                "Modifie le titre puis lance deux agents en arrière-plan et un agent normal."
                    .into(),
                UserOrigin::History
            ),
            ("Encore une chose".into(), UserOrigin::History),
            (
                "[Request interrupted by user]".into(),
                UserOrigin::Interrupted
            ),
        ]
    );

    // Textes de l'assistant, sans la ligne de fil latéral.
    let t = texts(&h);
    assert_eq!(
        t,
        vec![
            "Je lis le fichier.",
            "Le README décrit **un projet de démonstration**.",
            "Les agents tournent en arrière-plan.",
            "L'agent a trouvé 3 fichiers.",
            "Je commence",
        ]
    );
    assert!(!t
        .iter()
        .any(|x| x.contains("latéral") || x.contains("Sous-agent")));

    // Index des blocs d'un même message réparti sur plusieurs lignes.
    let msg01: Vec<u32> = h
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::BlockDone { msg, index, .. } if msg.0 == "msg_01" => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(msg01, vec![0, 1, 2]);

    // Outils et résultats, avec le `toolUseResult` du transcript en `meta`.
    let edit_meta = h.events.iter().find_map(|e| match &e.body {
        EventBody::ToolResult {
            tool_use_id, meta, ..
        } if tool_use_id == "toolu_e1" => meta.clone(),
        _ => None,
    });
    let edit_meta = edit_meta.expect("tool_result de l'Edit");
    assert_eq!(edit_meta["newString"], "# Démo 2");
    assert!(edit_meta["structuredPatch"].is_array());
    assert!(bodies(&h).iter().any(|b| matches!(
        b,
        EventBody::BlockDone { block: BlockDoneKind::ToolUse { name, .. }, .. } if name == "Read"
    )));

    // Fins de tour (`system/turn_duration`) : 3 `turn_result`.
    let durations: Vec<u64> = h
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::TurnResult { duration_ms, .. } => Some(*duration_ms),
            _ => None,
        })
        .collect();
    assert_eq!(durations, vec![6000, 4000, 2000]);

    // Horodatage : `t` vient du `timestamp` des lignes, croissant.
    assert!(h.events[0].t >= 1_794_819_600_000);
    assert!(h.events.windows(2).all(|w| w[0].t <= w[1].t));
}

#[test]
fn transcripts_unknown_and_damaged_lines_are_counted_not_blocking() {
    let h = load(FULL).unwrap();
    assert_eq!(h.bad, 1, "une ligne abîmée au milieu");
    assert!(!h.truncated);
    assert_eq!(h.unknown, 1, "type `brand-new-type`");
    assert_eq!(h.unknown_elements, 1, "bloc `fancy_block`");
    assert_eq!(h.sidechain, 1);
    assert_eq!(h.lines, h.used + h.ignored + h.bad + h.sidechain);
    // Notice d'avertissement, à la fin.
    match &h.events.last().unwrap().body {
        EventBody::Notice { level, code, .. } => {
            assert_eq!(*level, NoticeLevel::Warn);
            assert_eq!(code, "history_damaged");
        }
        other => panic!("notice attendue, vu {other:?}"),
    }
}

#[test]
fn transcripts_subagents_are_rebuilt_from_meta_without_their_threads() {
    let dir = project().join(FULL).join("subagents");
    let (metas, threads) = read_subagents(&dir);
    assert_eq!(threads, 1, "agent-a1.jsonl");
    // La méta abîmée est ignorée.
    assert_eq!(metas.len(), 2);

    let h = load(FULL).unwrap();
    assert_eq!(h.subagent_threads, 1);
    // Aucun événement dans un autre fil que `main` : les fils des agents ne
    // sont pas relus.
    for e in &h.events {
        let thread = match &e.body {
            EventBody::UserMessage { thread, .. }
            | EventBody::BlockDone { thread, .. }
            | EventBody::ToolResult { thread, .. } => Some(thread),
            _ => None,
        };
        if let Some(t) = thread {
            assert_eq!(t.0, "main");
        }
    }

    let agents = last_agents(&h);
    let get = |id: &str| {
        agents
            .iter()
            .find(|a| a.tool_use_id == id)
            .unwrap_or_else(|| panic!("agent {id}"))
    };
    // Arrière-plan (méta), terminé par `<task-notification>`.
    let a1 = get("toolu_a1");
    assert_eq!(a1.agent_type, "agent-bleu", "U+00A0 retiré");
    assert_eq!(a1.color.as_deref(), Some("blue"));
    assert!(a1.background);
    assert_eq!(a1.task_id.as_deref(), Some("a1"));
    assert_eq!(a1.status, AgentStatus::Completed);
    assert_eq!(
        a1.summary.as_deref(),
        Some("Agent \"Compter les fichiers\" completed")
    );
    // Arrière-plan d'après `toolUseResult.isAsync`, jamais terminé : arrêté.
    let a3 = get("toolu_a3");
    assert!(a3.background);
    assert_eq!(a3.status, AgentStatus::Stopped);
    // Avant-plan : terminé par son `tool_result`.
    let a2 = get("toolu_a2");
    assert!(!a2.background);
    assert_eq!(a2.status, AgentStatus::Completed);
    // Connu par sa méta seulement.
    let a9 = get("toolu_a9");
    assert_eq!(a9.agent_type, "agent-rouge");
    assert_eq!(a9.description, "Agent connu par sa méta seulement");
    assert_eq!(a9.status, AgentStatus::Stopped);
    assert_eq!(agents.len(), 4);
}

#[test]
fn transcripts_truncated_file_loads_what_is_complete() {
    let h = load(TRUNCATED).unwrap();
    assert!(h.truncated);
    assert_eq!(h.bad, 0);
    // Pas d'avertissement pour une dernière ligne inachevée.
    assert!(!bodies(&h)
        .iter()
        .any(|b| matches!(b, EventBody::Notice { .. })));
    assert_eq!(
        texts(&h),
        vec![
            "Je lis le fichier.",
            "Le README décrit **un projet de démonstration**."
        ]
    );
    assert!(!texts(&h).iter().any(|t| t.contains("coupée")));
}

#[test]
fn transcripts_unreadable_or_missing_file_is_an_error() {
    assert!(matches!(
        load(UNREADABLE),
        Err(CoreError::TranscriptFormat(_))
    ));
    assert!(matches!(
        load("0b8f3c2e-1111-4a6b-9c1d-0000000000ff"),
        Err(CoreError::Io(_))
    ));
    assert!(matches!(
        load(r"..\..\secret"),
        Err(CoreError::InvalidArgument(_))
    ));
}

#[test]
fn transcripts_reader_never_panics_on_garbage() {
    let samples: &[&[u8]] = &[
        b"",
        b"\n\n\r\n",
        b"null\n42\n[]\n\"x\"\n",
        b"{\"type\":\"user\"}\n{\"type\":\"user\",\"message\":null}\n{\"type\":\"user\",\"message\":{\"content\":7}}\n",
        b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}\n",
        b"{\"type\":\"system\",\"subtype\":\"turn_duration\",\"durationMs\":\"x\"}\n",
        b"{\"type\":\"user\",\"message\":{\"content\":\"<task-notification></task-notification>\"}}\n",
        b"{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]},\"toolUseResult\":{\"isAsync\":true}}\n",
        b"\xff\xfe{\"type\":\"user\"}\n",
        b"{\"type\":\"user\",\"timestamp\":\"9999-99-99T99:99:99Z\",\"message\":{\"content\":\"ok\"}}",
    ];
    for s in samples {
        let h = history_from_bytes(s, &[], &key());
        assert!(h.lines >= h.bad);
    }
}

/// Balayage des vrais transcripts de la machine (lecture seule ; n'affiche
/// que des comptes) : `cargo test --test sessions_transcripts -- --ignored
/// --nocapture`. À relancer à chaque nouvelle version du CLI.
#[test]
#[ignore = "lit ~/.claude/projects"]
fn transcripts_local_sweep_never_fails() {
    use orchestrator_lib::sessions::paths::{is_session_id, projects_root};
    let Some(root) = projects_root() else { return };
    let (mut files, mut events, mut bad, mut unknown, mut errors, mut lines) = (0, 0, 0, 0, 0, 0);
    let t0 = std::time::Instant::now();
    for dir in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(dir.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = f.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".jsonl").filter(|i| is_session_id(i)) else {
                continue;
            };
            files += 1;
            match load_history(&dir.path(), id, &key()) {
                Ok(h) => {
                    events += h.events.len();
                    bad += h.bad;
                    unknown += h.unknown;
                    lines += h.lines;
                }
                Err(_) => errors += 1,
            }
        }
    }
    eprintln!(
        "{files} transcripts, {lines} lignes, {events} événements, {bad} illisibles, {unknown} de type inconnu, {errors} erreurs, {:?}",
        t0.elapsed()
    );
}
