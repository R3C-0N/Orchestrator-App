//! J11 : lecteur de transcript (décision 9, plan 1.8 et 1.10 point 4) sur
//! les extraits **synthétiques** de `fixtures/transcripts/projects/`
//! (régénérables par `node fixtures/transcripts/generate.mjs
//! fixtures/transcripts`).
//!
//! `cargo test transcripts` : à relancer à chaque nouvelle version du CLI.

use std::path::PathBuf;

use orchestrator_lib::api::events::{
    AgentEvent, AgentStatus, BlockDoneKind, EventBody, NoticeLevel, ResultPart, UserOrigin,
};
use orchestrator_lib::api::types::{CoreError, HistoryLoaded, SessionKey};
use orchestrator_lib::sessions::paths::find_project_dir;
use orchestrator_lib::sessions::transcript::{
    history_from_bytes, history_from_bytes_with, load_history, load_history_with, read_subagents,
    HistoryLimits, Omitted, CLIPPED_MARK,
};

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
    // (lignes, utilisées, ignorées, illisibles, tronqué, événements)
    let expected: &[(u32, u32, u32, u32, bool, usize)] = &[
        (0, 0, 0, 0, false, 0),
        (0, 0, 0, 0, false, 0),
        // 4 lignes qui ne sont pas des objets : illisibles, une notice.
        (4, 0, 0, 4, false, 1),
        // `user` sans contenu exploitable : ignorés.
        (3, 0, 3, 0, false, 0),
        // `tool_use` sans id ni nom : un `block_done`, pas d'agent.
        (1, 1, 0, 0, false, 1),
        // Durée illisible : fin de tour à 0 ms.
        (1, 1, 0, 0, false, 1),
        // Notification sans identifiant : masquée.
        (1, 0, 1, 0, false, 0),
        // `tool_result` sans id, accusé asynchrone sans agent : un résultat.
        (1, 1, 0, 0, false, 1),
        // Octets invalides devant le JSON : illisible, une notice.
        (1, 0, 0, 1, false, 1),
        // Dernière ligne sans fin de ligne mais complète, date absurde : lue.
        (1, 1, 0, 0, false, 1),
    ];
    for (s, want) in samples.iter().zip(expected) {
        let h = history_from_bytes(s, &[], &key());
        let got = (
            h.lines,
            h.used,
            h.ignored,
            h.bad,
            h.truncated,
            h.events.len(),
        );
        assert_eq!(got, *want, "{:?}", String::from_utf8_lossy(s));
        assert_eq!(h.lines, h.used + h.ignored + h.bad + h.sidechain);
        for (i, e) in h.events.iter().enumerate() {
            assert_eq!(e.seq, i as u64);
        }
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

fn user_line(text: &str, origin: Option<&str>) -> String {
    let mut v = serde_json::json!({
        "type": "user", "isSidechain": false, "uuid": "u",
        "timestamp": "2026-11-16T09:00:00Z",
        "message": {"role": "user", "content": text},
    });
    if let Some(o) = origin {
        v["origin"] = serde_json::json!({ "kind": o });
    }
    v.to_string()
}

#[test]
fn transcripts_typed_text_citing_cli_tags_stays_typed() {
    let cited = "Que fait <task-notification><task-id>a1</task-id><tool-use-id>t1</tool-use-id></task-notification> ?";
    let mid = "Explique <command-name>/model</command-name> stp";
    let text = format!(
        "{}\n{}\n{}\n",
        user_line(cited, Some("human")),
        user_line(mid, Some("human")),
        user_line(
            "<command-name>/model</command-name>\n<command-args>haiku</command-args>",
            Some("human")
        ),
    );
    let h = history_from_bytes(text.as_bytes(), &[], &key());
    assert_eq!(
        users(&h),
        vec![
            (cited.to_string(), UserOrigin::History),
            (mid.to_string(), UserOrigin::History),
            ("/model haiku".to_string(), UserOrigin::History),
        ]
    );
    assert!(!bodies(&h).iter().any(|b| matches!(b, EventBody::Agent(_))));
}

#[test]
fn transcripts_peer_messages_are_never_shown_as_typed() {
    // Rapport d'un sous-agent (`origin.kind = "peer"`), même sans `isMeta`.
    let text = format!(
        "{}\n{}\n",
        user_line("Rapport du sous-agent : fini", Some("peer")),
        user_line("Message tapé", None),
    );
    let h = history_from_bytes(text.as_bytes(), &[], &key());
    assert_eq!(
        users(&h),
        vec![("Message tapé".to_string(), UserOrigin::History)]
    );
    assert_eq!(h.ignored, 1);
}

#[test]
fn transcripts_window_keeps_the_end_and_reports_what_is_omitted() {
    let path = project().join(format!("{FULL}.jsonl"));
    let bytes = std::fs::read(&path).unwrap();
    let total_lines = bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .count() as u32;
    let limits = HistoryLimits {
        max_bytes: bytes.len() as u64 / 2,
        ..HistoryLimits::default()
    };
    let h = load_history_with(&project(), FULL, &key(), &limits).unwrap();
    assert!(h.omitted_lines > 0);
    assert_eq!(h.omitted_lines + h.lines, total_lines);
    let half = bytes.len() as u64 / 2;
    assert!(h.omitted_bytes >= half && h.omitted_bytes < half + 4096);
    match &h.events[0].body {
        EventBody::Notice {
            level,
            code,
            detail,
            ..
        } => {
            assert_eq!(*level, NoticeLevel::Info);
            assert_eq!(code, "history_truncated");
            let d = detail.as_ref().unwrap();
            assert_eq!(d["omitted_lines"], h.omitted_lines);
            assert_eq!(d["omitted_bytes"], h.omitted_bytes);
        }
        other => panic!("notice attendue en tête, vu {other:?}"),
    }
    // La fin est intacte.
    assert_eq!(texts(&h).last().map(String::as_str), Some("Je commence"));
    // Plafond non atteint : rien d'omis, pas de notice.
    let full = load(FULL).unwrap();
    assert_eq!((full.omitted_lines, full.omitted_bytes), (0, 0));
    assert!(!bodies(&full)
        .iter()
        .any(|b| matches!(b, EventBody::Notice { code, .. } if code == "history_truncated")));
}

#[test]
fn transcripts_large_tool_results_and_meta_are_clipped() {
    let big = "é".repeat(150_000); // 300 000 octets
    let lines = [
        serde_json::json!({"type": "assistant", "timestamp": "2026-11-16T09:00:00Z",
            "message": {"id": "m1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Write", "input": {"file_path": "a", "content": big}}]}}),
        serde_json::json!({"type": "user", "timestamp": "2026-11-16T09:00:01Z",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": big}]},
            "toolUseResult": {"type": "create", "filePath": "a", "originalFile": big, "content": big}}),
    ]
    .iter()
    .map(|v| v.to_string() + "\n")
    .collect::<String>();
    let h = history_from_bytes(lines.as_bytes(), &[], &key());
    assert_eq!(h.clipped, 4);
    let max = HistoryLimits::default().max_item_bytes;
    let (content, meta) = h
        .events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ToolResult { content, meta, .. } => Some((content.clone(), meta.clone())),
            _ => None,
        })
        .unwrap();
    let ResultPart::Text { text } = &content[0] else {
        panic!("texte attendu")
    };
    assert!(text.ends_with(CLIPPED_MARK) && text.len() <= max + CLIPPED_MARK.len());
    let meta = meta.unwrap();
    let orig = meta["originalFile"].as_str().unwrap();
    assert!(orig.ends_with(CLIPPED_MARK) && orig.len() <= max + CLIPPED_MARK.len());
}

#[test]
fn transcripts_output_budget_drops_oldest_events_at_a_user_message() {
    let mut text = String::new();
    for i in 0..50 {
        text.push_str(&user_line(
            &format!("question {i} {}", "q".repeat(1000)),
            None,
        ));
        text.push('\n');
        text.push_str(
            &serde_json::json!({"type": "assistant", "timestamp": "2026-11-16T09:00:00Z",
                "message": {"id": format!("m{i}"), "role": "assistant",
                "content": [{"type": "text", "text": "r".repeat(1000)}]}})
            .to_string(),
        );
        text.push('\n');
    }
    let limits = HistoryLimits {
        max_output_bytes: 20_000,
        ..HistoryLimits::default()
    };
    let h = history_from_bytes_with(text.as_bytes(), &[], &key(), &limits, Omitted::default());
    assert!(h.dropped_events > 0);
    assert!(matches!(&h.events[0].body,
        EventBody::Notice { code, .. } if code == "history_truncated"));
    assert!(matches!(&h.events[1].body, EventBody::UserMessage { .. }));
    let size: usize = h
        .events
        .iter()
        .map(|e| serde_json::to_vec(e).unwrap().len())
        .sum();
    assert!(size < 25_000, "{size}");
    assert_eq!(texts(&h).last().unwrap(), &"r".repeat(1000));
}

/// Transcript synthétique de 200 Mo, écrit dans un dossier temporaire (jamais
/// commité) : la réponse sérialisée reste sous 30 Mo.
#[test]
fn transcripts_200mb_history_serializes_under_30mb() {
    use std::io::Write;
    /// Supprime le dossier (200 Mo) à la fin du test, même en cas d'échec.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = std::env::temp_dir().join(format!("orch-j11-200mb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = Cleanup(dir.clone());
    let id = "0b8f3c2e-1111-4a6b-9c1d-0000000000aa";
    let path = dir.join(format!("{id}.jsonl"));
    // ~290 ko de sortie (texte JSON déjà échappé), et une ligne géante
    // comme la plus longue constatée (962 000 caractères).
    let out = "ligne de sortie de commande\\n".repeat(10_000);
    let long_line = "x".repeat(962_000);
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut i = 0u64;
        let mut written = 0u64;
        while written < 200 * 1024 * 1024 {
            let lines = [
                format!(
                    r#"{{"type":"user","timestamp":"2026-11-16T09:00:00Z","origin":{{"kind":"human"}},"message":{{"role":"user","content":"tour {i}"}}}}"#
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"2026-11-16T09:00:01Z","message":{{"id":"m{i}","role":"assistant","content":[{{"type":"tool_use","id":"t{i}","name":"Bash","input":{{"command":"echo {i}"}}}}]}}}}"#
                ),
                format!(
                    r#"{{"type":"user","timestamp":"2026-11-16T09:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{i}","content":"{out}"}}]}},"toolUseResult":{{"stdout":"{out}","stderr":"","originalFile":"{long_line}"}}}}"#
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"2026-11-16T09:00:03Z","message":{{"id":"n{i}","role":"assistant","content":[{{"type":"text","text":"Fini {i}."}}]}}}}"#
                ),
            ];
            for l in lines {
                f.write_all(l.as_bytes()).unwrap();
                f.write_all(b"\n").unwrap();
                written += l.len() as u64 + 1;
            }
            i += 1;
        }
    }
    let size = std::fs::metadata(&path).unwrap().len();
    let t0 = std::time::Instant::now();
    let h = load_history(&dir, id, &key()).unwrap();
    let elapsed = t0.elapsed();
    let json = serde_json::to_vec(&h).unwrap();
    eprintln!(
        "200 Mo : fichier {} o, {} événements, {} lignes omises ({} o), {} chaînes raccourcies, JSON {} o, {:?}",
        size,
        h.events.len(),
        h.omitted_lines,
        h.omitted_bytes,
        h.clipped,
        json.len(),
        elapsed
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(size >= 200 * 1024 * 1024);
    assert!(json.len() < 30 * 1024 * 1024, "{} octets", json.len());
    assert!(h.omitted_bytes >= size - HistoryLimits::default().max_bytes);
    assert_eq!(h.bad, 0);
    assert!(matches!(&h.events[0].body,
        EventBody::Notice { code, .. } if code == "history_truncated"));
}

/// Plus gros vrai transcript de la machine (lecture seule ; n'affiche que des
/// tailles et des durées).
#[test]
#[ignore = "lit ~/.claude/projects"]
fn transcripts_largest_local_transcript_timing() {
    use orchestrator_lib::sessions::paths::{is_session_id, projects_root};
    let Some(root) = projects_root() else { return };
    let mut best: Option<(u64, PathBuf, String)> = None;
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
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|b| len > b.0) {
                best = Some((len, dir.path(), id.to_string()));
            }
        }
    }
    let Some((len, dir, id)) = best else { return };
    let t0 = std::time::Instant::now();
    let h = load_history(&dir, &id, &key()).unwrap();
    let elapsed = t0.elapsed();
    let json = serde_json::to_vec(&h).unwrap();
    eprintln!(
        "plus gros transcript : {len} o ; load_history {elapsed:?} ; {} événements ; {} lignes relues, {} omises ({} o) ; {} chaînes raccourcies ; {} événements retirés ; JSON {} o",
        h.events.len(),
        h.lines,
        h.omitted_lines,
        h.omitted_bytes,
        h.clipped,
        h.dropped_events,
        json.len()
    );
    assert!(json.len() < 30 * 1024 * 1024);
}
