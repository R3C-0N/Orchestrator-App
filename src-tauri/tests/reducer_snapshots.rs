//! Réducteur J4 sur les captures (plan 1.10, point 1) : instantanés
//! `fixtures/ui-events/tN.json` et assertions ciblées.
//!
//! Instantanés : les fichiers `fixtures/ui-events/tN.json` sont produits par
//! l'outil `replay` (`cargo run --bin replay`) et relus par l'interface (mode
//! simulé, Vitest). Ce test vérifie qu'ils sont à jour : un écart veut dire
//! que le comportement du réducteur a changé. Après vérification du
//! changement, régénérer avec `cargo run --bin replay`.

use std::path::PathBuf;

use orchestrator_lib::api::events::{
    AgentEvent, AgentStatus, BlockDoneKind, CoreEvent, EventBody, NoticeLevel, QuestionCloseReason,
    ResultPart, SessionState, UserOrigin,
};
use orchestrator_lib::api::types::SkipReason;
use orchestrator_lib::protocol::fixture::{classify, FixtureLine};
use orchestrator_lib::protocol::inbound::Inbound;
use orchestrator_lib::protocol::outbound;
use orchestrator_lib::state::reducer::{Action, Reducer};
use orchestrator_lib::state::replay::{
    replay_fixture, to_json_lines, Replay, REPLAY_STEP_MS, REPLAY_T0, UI_EVENT_FIXTURES,
};
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(root().join(format!("fixtures/stream-json/{name}.jsonl")))
        .unwrap_or_else(|e| panic!("{name} : {e}"))
}

fn replay(name: &str) -> Replay {
    replay_fixture(&fixture_text(name), name)
}

fn bodies(rep: &Replay) -> Vec<&EventBody> {
    rep.events.iter().map(|e| &e.body).collect()
}

fn agents(rep: &Replay) -> Vec<&AgentEvent> {
    rep.events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Agent(a) => Some(a),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Instantanés

#[test]
fn ui_events_snapshots_are_up_to_date() {
    let mut stale = Vec::new();
    for name in UI_EVENT_FIXTURES {
        let rep = replay(name);
        assert_eq!(rep.bad_lines, 0, "{name} : lignes illisibles");
        assert_eq!(rep.reducer.unknown_count, 0, "{name} : inconnus");
        let path = root().join(format!("fixtures/ui-events/{name}.json"));
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| {
                panic!("{} : {e} (lancer `cargo run --bin replay`)", path.display())
            })
            .replace("\r\n", "\n");
        if to_json_lines(&rep.events) != expected {
            stale.push(*name);
        }
        // Le fichier se relit en `CoreEvent` (contrat aller-retour).
        let back: Vec<CoreEvent> = serde_json::from_str(&expected).unwrap();
        assert_eq!(back.len(), rep.events.len(), "{name}");
    }
    assert!(
        stale.is_empty(),
        "instantanés périmés : {stale:?} ; vérifier le changement puis `cargo run --bin replay`"
    );
}

#[test]
fn replay_is_deterministic_and_ordered() {
    for name in UI_EVENT_FIXTURES {
        let a = replay(name);
        let b = replay(name);
        assert_eq!(a.events, b.events, "{name}");
        for (i, e) in a.events.iter().enumerate() {
            assert_eq!(e.seq, i as u64, "{name} : seq continu");
            assert_eq!(e.session.0, *name);
            assert!(e.t >= REPLAY_T0 && (e.t - REPLAY_T0).is_multiple_of(REPLAY_STEP_MS));
        }
        assert!(a.events.windows(2).all(|w| w[0].t <= w[1].t), "{name}");
    }
}

#[test]
fn t15_long_session_replays_without_unknown() {
    let rep = replay("t15");
    assert_eq!(rep.bad_lines, 0);
    assert_eq!(rep.reducer.unknown_count, 0);
    let results = bodies(&rep)
        .iter()
        .filter(|b| matches!(b, EventBody::TurnResult { .. }))
        .count();
    assert_eq!(results, 1000);
    // Mémoire bornée : aucune question ni compteur de bloc du fil principal
    // ne survit aux tours.
    assert!(rep.reducer.questions.is_empty());
}

// ---------------------------------------------------------------------------
// Assertions ciblées (plan 1.10)

#[test]
fn t3_two_agents_blue_and_red_completed() {
    let rep = replay("t3");
    let all = rep.reducer.agents();
    assert_eq!(all.len(), 2, "{all:?}");
    let bleu = &all[0];
    let rouge = &all[1];
    assert_eq!(bleu.agent_type, "agent-bleu", "U+00A0 retiré");
    assert_eq!(bleu.color.as_deref(), Some("blue"));
    assert_eq!(rouge.agent_type, "agent-rouge");
    assert_eq!(rouge.color.as_deref(), Some("red"));
    for a in [bleu, rouge] {
        assert_eq!(a.status, AgentStatus::Completed);
        assert!(a.task_id.is_some());
        assert!(a.started_at.is_some() && a.ended_at >= a.started_at);
        assert!(a.summary.is_some());
        assert!(!a.description.is_empty());
    }
    assert!(!bleu.background);
    assert!(rouge.background);
    // Usage et outil en cours de task_progress / task_notification.
    assert_eq!(bleu.last_tool.as_deref(), Some("Read"));
    assert_eq!(bleu.tokens, Some(35640));
    assert_eq!(bleu.tool_uses, Some(1));

    // Chaque événement `agent` est un état complet : type et couleur
    // toujours présents, statut qui ne recule jamais.
    let rank = |s: AgentStatus| match s {
        AgentStatus::Starting => 0,
        AgentStatus::Running => 1,
        _ => 2,
    };
    for id in [&bleu.tool_use_id, &rouge.tool_use_id] {
        let evs: Vec<_> = agents(&rep)
            .into_iter()
            .filter(|a| &a.tool_use_id == id)
            .collect();
        assert!(evs.len() >= 3, "{id} : {evs:?}");
        assert_eq!(evs[0].status, AgentStatus::Starting);
        assert!(evs
            .iter()
            .all(|a| a.color.is_some() && !a.agent_type.is_empty()));
        assert!(evs
            .windows(2)
            .all(|w| rank(w[0].status) <= rank(w[1].status)));
    }
    // L'agent rouge est relié à sa tâche dès `background_tasks_changed`
    // (avant `task_started`).
    let first_linked = agents(&rep)
        .into_iter()
        .find(|a| a.tool_use_id == rouge.tool_use_id && a.task_id.is_some())
        .unwrap();
    assert!(first_linked.background);
    assert_eq!(first_linked.status, AgentStatus::Running);

    // `agent` suit le `block_done` de l'appel Agent (la ligne d'outil existe
    // avant de devenir une ligne d'agent).
    let b = bodies(&rep);
    let done = b
        .iter()
        .position(|e| matches!(e, EventBody::BlockDone { block: BlockDoneKind::ToolUse { tool_use_id, .. }, .. } if *tool_use_id == bleu.tool_use_id))
        .unwrap();
    let first_agent = b
        .iter()
        .position(|e| matches!(e, EventBody::Agent(a) if a.tool_use_id == bleu.tool_use_id))
        .unwrap();
    assert_eq!(first_agent, done + 1);
}

#[test]
fn bash_tasks_never_become_agents() {
    for name in ["t5", "t14"] {
        let rep = replay(name);
        assert!(
            agents(&rep).is_empty(),
            "{name} : une tâche Bash n'est pas un agent"
        );
    }
}

fn questions(rep: &Replay) -> Vec<String> {
    bodies(rep)
        .iter()
        .filter_map(|b| match b {
            EventBody::Question { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .collect()
}

fn closed(rep: &Replay) -> Vec<(String, QuestionCloseReason)> {
    bodies(rep)
        .iter()
        .filter_map(|b| match b {
            EventBody::QuestionClosed { request_id, reason } => Some((request_id.clone(), *reason)),
            _ => None,
        })
        .collect()
}

fn auto_allows(rep: &Replay) -> usize {
    bodies(rep)
        .iter()
        .filter(|b| matches!(b, EventBody::Notice { level: NoticeLevel::Info, code, .. } if code == "auto_allow"))
        .count()
}

#[test]
fn t4_question_and_auto_allowed_bash() {
    let rep = replay("t4");
    let q = questions(&rep);
    assert_eq!(q.len(), 1);
    assert_eq!(auto_allows(&rep), 1, "allow automatique pour Bash");
    assert_eq!(rep.writes.len(), 1);
    assert_eq!(
        closed(&rep),
        vec![(q[0].clone(), QuestionCloseReason::Answered)]
    );
    // Le message tapé par le pilote apparaît comme message `Typed`.
    assert!(bodies(&rep).iter().any(|b| matches!(
        b,
        EventBody::UserMessage {
            origin: UserOrigin::Typed,
            ..
        }
    )));
}

#[test]
fn t8_question_without_can_use_tool_for_bash() {
    let rep = replay("t8");
    let q = questions(&rep);
    assert_eq!(q.len(), 1);
    // bypassPermissions : le Bash part sans can_use_tool (décision 7, t8).
    assert_eq!(auto_allows(&rep), 0);
    assert_eq!(
        closed(&rep),
        vec![(q[0].clone(), QuestionCloseReason::Answered)]
    );
}

#[test]
fn t6_aborted_turn_then_normal_turn() {
    let rep = replay("t6");
    let results: Vec<(Option<String>, bool)> = bodies(&rep)
        .iter()
        .filter_map(|b| match b {
            EventBody::TurnResult {
                terminal_reason,
                is_error,
                ..
            } => Some((terminal_reason.clone(), *is_error)),
            _ => None,
        })
        .collect();
    let aborted = results
        .iter()
        .position(|(r, _)| r.as_deref() == Some("aborted_streaming"))
        .expect("tour interrompu");
    let normal = results
        .iter()
        .rposition(|(r, e)| r.as_deref() == Some("completed") && !e)
        .expect("tour normal");
    assert!(aborted < normal, "{results:?}");
    // L'interruption passe par Interrupting puis revient à Idle.
    let states: Vec<SessionState> = bodies(&rep)
        .iter()
        .filter_map(|b| match b {
            EventBody::Status { state } => Some(*state),
            _ => None,
        })
        .collect();
    let i = states
        .iter()
        .position(|s| *s == SessionState::Interrupting)
        .expect("Interrupting");
    assert!(states[i..].contains(&SessionState::Idle));
}

#[test]
fn t10_skip_closes_with_skipped_and_matches_driver_bytes() {
    let rep = replay("t10");
    let q = questions(&rep);
    assert_eq!(q.len(), 1);
    assert_eq!(
        closed(&rep),
        vec![(q[0].clone(), QuestionCloseReason::Skipped)]
    );

    // Réponse « ignorer » construite depuis l'input gardé par le réducteur,
    // identique à l'octet près à la ligne `>>> ` du pilote.
    let text = fixture_text("t10");
    let mut r = Reducer::new();
    let mut checked = false;
    for line in text.lines() {
        match classify(line.trim_end_matches('\r')) {
            FixtureLine::Cli(l) => {
                r.handle_inbound_at(&Inbound::from_line(l).unwrap(), 0);
            }
            FixtureLine::Sent(l) if l.contains("\"unanswered\"") => {
                let input = r.questions.get(&q[0]).expect("question en attente");
                assert_eq!(
                    outbound::skip_question(&q[0], input, SkipReason::Skipped),
                    l
                );
                checked = true;
            }
            _ => {}
        }
    }
    assert!(checked);
}

/// `t16` à `t18` (finitions) : question multi-sélection réelle, répondue
/// par un tableau (`Answered`), puis ignorée avec `replied_in_chat` et
/// `dismissed` (`Skipped`) ; texte exact du `tool_result` reçu par Claude.
#[test]
fn t16_t18_multi_select_answer_and_skip_reasons() {
    const SKIPPED: &str = "The user did not answer the questions.";
    for (name, reason, expected) in [
        (
            "t16",
            QuestionCloseReason::Answered,
            "Your questions have been answered: \"Quelles couleurs ?\"=\"Rouge,Bleu\". You can now continue with these answers in mind.",
        ),
        ("t17", QuestionCloseReason::Skipped, SKIPPED),
        ("t18", QuestionCloseReason::Skipped, SKIPPED),
    ] {
        let rep = replay(name);
        let q = questions(&rep);
        assert_eq!(q.len(), 1, "{name}");
        assert_eq!(closed(&rep), vec![(q[0].clone(), reason)], "{name}");
        let b = bodies(&rep);
        assert!(
            b.iter().any(|b| matches!(b,
                EventBody::Question { questions, .. } if questions[0].multi_select)),
            "{name} : question multi-sélection"
        );
        let texts: Vec<&str> = b
            .iter()
            .filter_map(|b| match b {
                EventBody::ToolResult { content, .. } => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|p| match p {
                ResultPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&expected), "{name} : {texts:?}");
    }
}

#[test]
fn t11_ready_with_108_commands_and_22_agents() {
    let rep = replay("t11");
    let ready = bodies(&rep)
        .into_iter()
        .find_map(|b| match b {
            EventBody::Ready {
                commands, agents, ..
            } => Some((commands.len(), agents.len())),
            _ => None,
        })
        .expect("ready");
    assert_eq!(ready, (108, 22));
}

#[test]
fn t14_exit_code_is_reported_after_the_error_turn() {
    let rep = replay("t14");
    let b = bodies(&rep);
    assert!(matches!(
        b.iter()
            .rev()
            .find(|e| matches!(e, EventBody::Exited { .. })),
        Some(EventBody::Exited { code: Some(1), .. })
    ));
    assert!(matches!(
        b.last(),
        Some(EventBody::Status {
            state: SessionState::Exited
        })
    ));
    assert!(b.iter().any(|e| matches!(e, EventBody::TurnResult { subtype, is_error: true, .. } if subtype == "error_max_turns")));
}

// ---------------------------------------------------------------------------
// Cas hors captures

fn emitted(a: &[Action]) -> Vec<EventBody> {
    a.iter()
        .filter_map(|a| match a {
            Action::Emit(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn redacted_thinking_is_always_empty() {
    let mut r = Reducer::new();
    let a = r.handle_at(
        &json!({"type":"assistant","message":{"id":"m1","content":[
            {"type":"redacted_thinking","data":"chiffré","thinking":"texte non vide"},
            {"type":"thinking","thinking":"pensée visible","signature":"s"},
            {"type":"thinking","thinking":"","signature":"s"}
        ]}}),
        0,
    );
    let empties: Vec<(u32, bool)> = emitted(&a)
        .into_iter()
        .filter_map(|b| match b {
            EventBody::BlockDone {
                index,
                block: BlockDoneKind::Thinking { empty },
                ..
            } => Some((index, empty)),
            _ => None,
        })
        .collect();
    assert_eq!(empties, vec![(0, true), (1, false), (2, true)]);
}

#[test]
fn unknown_kinds_are_counted_including_system_subtypes_and_blocks() {
    let mut r = Reducer::new();
    let lines = [
        json!({"type":"brand_new_thing"}),
        json!({"type":"system","subtype":"brand_new_subtype"}),
        json!({"type":"assistant","message":{"id":"m","content":[{"type":"mystery_block"},{"type":"text","text":"ok"}]}}),
        json!({"type":"stream_event","event":{"type":"weird_event"}}),
        // Connus, ignorés par choix : pas des inconnus.
        json!({"type":"keep_alive"}),
        json!({"type":"system","subtype":"api_retry"}),
    ];
    let mut out = Vec::new();
    for l in &lines {
        out.extend(r.handle_at(l, 0));
    }
    assert_eq!(r.unknown_count, 4, "{:?}", r.unknown_kinds);
    assert_eq!(r.unknown_kinds.get("system/brand_new_subtype"), Some(&1));
    assert_eq!(
        r.unknown_kinds.get("assistant/bloc/mystery_block"),
        Some(&1)
    );
    // Aucune notice ; le bloc texte connu garde son index API (1).
    let ev = emitted(&out);
    assert!(!ev.iter().any(|b| matches!(b, EventBody::Notice { .. })));
    assert!(ev.iter().any(|b| matches!(
        b,
        EventBody::BlockDone {
            index: 1,
            block: BlockDoneKind::Text { .. },
            ..
        }
    )));
}

#[test]
fn tool_progress_and_subagent_tools_set_last_tool() {
    let mut r = Reducer::new();
    r.handle_at(
        &json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"tA","name":"Task","input":{"subagent_type":"\u{a0}agent-vert\u{a0}","description":"d"}}]}}),
        1,
    );
    let a = emitted(&r.handle_at(
        &json!({"type":"tool_progress","tool_use_id":"x","tool_name":"Grep","parent_tool_use_id":"tA","elapsed_time_seconds":1}),
        2,
    ));
    match &a[..] {
        [EventBody::Agent(a)] => {
            assert_eq!(a.last_tool.as_deref(), Some("Grep"));
            assert_eq!(a.color.as_deref(), Some("green"));
            assert_eq!(a.agent_type, "agent-vert");
        }
        other => panic!("{other:?}"),
    }
    let a = emitted(&r.handle_at(
        &json!({"type":"assistant","parent_tool_use_id":"tA","message":{"id":"m2","content":[{"type":"tool_use","id":"tB","name":"Bash","input":{}}]}}),
        3,
    ));
    assert!(a
        .iter()
        .any(|b| matches!(b, EventBody::Agent(a) if a.last_tool.as_deref() == Some("Bash"))));
    // Agent inconnu : pas de couleur.
    let a = emitted(&r.handle_at(
        &json!({"type":"assistant","message":{"id":"m3","content":[{"type":"tool_use","id":"tC","name":"Agent","input":{"subagent_type":"general-purpose"}}]}}),
        4,
    ));
    assert!(a
        .iter()
        .any(|b| matches!(b, EventBody::Agent(a) if a.tool_use_id == "tC" && a.color.is_none())));
}

/// Fil d'une question (colonne des agents, groupe « en attente ») : `main`
/// pour le fil principal, l'agent par `agent_id` (= `task_id`) ou par le fil
/// où l'appel `AskUserQuestion` a été vu.
#[test]
fn question_thread_names_the_waiting_agent() {
    let thread_of = |a: &[Action]| {
        emitted(a)
            .into_iter()
            .find_map(|b| match b {
                EventBody::Question { thread, .. } => Some(thread.0),
                _ => None,
            })
            .unwrap()
    };
    let ask = |rid: &str, tool: &str, agent: Option<&str>| {
        let mut req = json!({"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":tool,"input":{"questions":[]}});
        if let Some(a) = agent {
            req["agent_id"] = json!(a);
        }
        json!({"type":"control_request","request_id":rid,"request":req})
    };
    let mut r = Reducer::new();
    r.handle_at(
        &json!({"type":"assistant","message":{"id":"m","content":[
            {"type":"tool_use","id":"tA","name":"Agent","input":{"subagent_type":"agent-bleu","description":"a"}},
            {"type":"tool_use","id":"tB","name":"Agent","input":{"subagent_type":"agent-rouge","description":"b"}}
        ]}}),
        1,
    );
    r.handle_at(
        &json!({"type":"system","subtype":"task_started","task_id":"a0b1","tool_use_id":"tB","subagent_type":"agent-rouge","description":"b","task_type":"local_agent"}),
        2,
    );
    // Fil principal.
    assert_eq!(thread_of(&r.handle_at(&ask("q0", "tq0", None), 3)), "main");
    // Par le fil de l'appel AskUserQuestion (agent bleu).
    r.handle_at(
        &json!({"type":"assistant","parent_tool_use_id":"tA","message":{"id":"m2","content":[
            {"type":"tool_use","id":"tq1","name":"AskUserQuestion","input":{"questions":[]}}
        ]}}),
        4,
    );
    assert_eq!(thread_of(&r.handle_at(&ask("q1", "tq1", None), 5)), "tA");
    assert_eq!(r.ask_entries(), 1);
    // La question fermée, l'appel est oublié.
    assert_eq!(
        r.close_question("q1", QuestionCloseReason::Answered).len(),
        1
    );
    assert_eq!(r.ask_entries(), 0);
    // Par `agent_id` = `task_id` (agent rouge), même sans appel vu.
    assert_eq!(
        thread_of(&r.handle_at(&ask("q2", "tq2", Some("a0b1")), 6)),
        "tB"
    );
    // Par `agent_id` = `tool_use_id` de l'agent (bleu).
    assert_eq!(
        thread_of(&r.handle_at(&ask("q5", "tq5", Some("tA")), 6)),
        "tA"
    );
    // `agent_id` inconnu, appel jamais vu : fil principal, pas d'agent inventé.
    assert_eq!(
        thread_of(&r.handle_at(&ask("q3", "tq3", Some("inconnu")), 7)),
        "main"
    );
    // `agent_id` inconnu, appel vu dans le fil du rouge : le rouge.
    r.handle_at(
        &json!({"type":"assistant","parent_tool_use_id":"tB","message":{"id":"m3","content":[
            {"type":"tool_use","id":"tq4","name":"AskUserQuestion","input":{"questions":[]}}
        ]}}),
        8,
    );
    assert_eq!(
        thread_of(&r.handle_at(&ask("q4", "tq4", Some("inconnu")), 9)),
        "tB"
    );
    assert_eq!(r.ask_entries(), 1);
    // L'agent rouge se termine (question toujours ouverte) : l'appel est oublié.
    r.handle_at(
        &json!({"type":"system","subtype":"task_notification","task_id":"a0b1","tool_use_id":"tB","status":"completed"}),
        10,
    );
    assert_eq!(r.ask_entries(), 0);
}

/// Mort du processus avec deux questions ouvertes (fil principal et agent) :
/// chacune est fermée (`Cancelled`) avant `exited`, l'agent passe en
/// `Stopped`, la table des appels `AskUserQuestion` est vidée ; rien la
/// seconde fois.
#[test]
fn exit_cancels_open_questions_and_forgets_agent_asks() {
    let mut r = Reducer::new();
    r.handle_at(
        &json!({"type":"assistant","message":{"id":"m","content":[
            {"type":"tool_use","id":"tA","name":"Agent","input":{"subagent_type":"agent-bleu","description":"a"}}
        ]}}),
        1,
    );
    r.handle_at(
        &json!({"type":"assistant","parent_tool_use_id":"tA","message":{"id":"m2","content":[
            {"type":"tool_use","id":"tqA","name":"AskUserQuestion","input":{"questions":[]}}
        ]}}),
        2,
    );
    for (rid, tool) in [("q_main", "tqM"), ("q_agent", "tqA")] {
        r.handle_at(
            &json!({"type":"control_request","request_id":rid,"request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":tool,"input":{"questions":[]}}}),
            3,
        );
    }
    assert_eq!(r.questions.len(), 2);
    assert_eq!(r.ask_entries(), 1);
    let ev = emitted(&r.exited_at(Some(1), vec![], 10));
    let closed: Vec<(String, QuestionCloseReason)> = ev
        .iter()
        .filter_map(|b| match b {
            EventBody::QuestionClosed { request_id, reason } => Some((request_id.clone(), *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        vec![
            ("q_agent".to_string(), QuestionCloseReason::Cancelled),
            ("q_main".to_string(), QuestionCloseReason::Cancelled),
        ]
    );
    // Questions fermées avant `exited`.
    let pos = |f: &dyn Fn(&EventBody) -> bool| ev.iter().position(f).unwrap();
    assert!(
        pos(&|b| matches!(b, EventBody::QuestionClosed { .. }))
            < pos(&|b| matches!(b, EventBody::Exited { .. }))
    );
    assert!(ev
        .iter()
        .any(|b| matches!(b, EventBody::Agent(a) if a.tool_use_id == "tA" && a.status == AgentStatus::Stopped)));
    assert!(r.questions.is_empty());
    assert_eq!(r.ask_entries(), 0);
    // Une réponse tardive ne ferme plus rien ; une seconde fin n'émet rien.
    assert!(r
        .close_question("q_main", QuestionCloseReason::Answered)
        .is_empty());
    assert!(r.exited_at(Some(1), vec![], 11).is_empty());
}

/// `stop_unfinished` (fin du processus) vide la table des appels
/// `AskUserQuestion` même sans question ouverte.
#[test]
fn exit_forgets_agent_asks_without_open_question() {
    let mut r = Reducer::new();
    r.handle_at(
        &json!({"type":"assistant","message":{"id":"m","content":[
            {"type":"tool_use","id":"tA","name":"Agent","input":{"subagent_type":"agent-bleu","description":"a"}}
        ]}}),
        1,
    );
    r.handle_at(
        &json!({"type":"assistant","parent_tool_use_id":"tA","message":{"id":"m2","content":[
            {"type":"tool_use","id":"tqA","name":"AskUserQuestion","input":{"questions":[]}}
        ]}}),
        2,
    );
    assert_eq!(r.ask_entries(), 1);
    r.exited_at(None, vec![], 3);
    assert_eq!(r.ask_entries(), 0);
}

#[test]
fn cancel_closes_and_history_mode_marks_user_messages() {
    let mut r = Reducer::new();
    r.handle_at(
        &json!({"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[]}}}),
        0,
    );
    let a = emitted(&r.handle_at(
        &json!({"type":"control_cancel_request","request_id":"q1"}),
        0,
    ));
    assert_eq!(
        a,
        vec![EventBody::QuestionClosed {
            request_id: "q1".into(),
            reason: QuestionCloseReason::Cancelled
        }]
    );
    assert!(r
        .close_question("q1", QuestionCloseReason::Skipped)
        .is_empty());

    r.history = true;
    let a = emitted(&r.handle_at(
        &json!({"type":"user","message":{"role":"user","content":"bonjour"}}),
        0,
    ));
    assert!(matches!(
        &a[..],
        [EventBody::UserMessage {
            origin: UserOrigin::History,
            ..
        }]
    ));
}

#[test]
fn ui_event_files_parse_as_the_ts_contract_expects() {
    // Forme attendue par `session.svelte.ts` : `kind` à plat, `agent` à plat.
    let rep = replay("t3");
    let v: Value = serde_json::from_str(&to_json_lines(&rep.events)).unwrap();
    let agent = v
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "agent")
        .unwrap();
    for k in [
        "seq",
        "t",
        "session",
        "tool_use_id",
        "agent_type",
        "color",
        "status",
    ] {
        assert!(agent.get(k).is_some(), "champ {k} absent : {agent}");
    }
}
