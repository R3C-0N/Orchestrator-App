//! Rejeu d'une capture `fixtures/stream-json/tN.jsonl` en `CoreEvent`
//! (outil `replay`, plan J4 ; instantanés `fixtures/ui-events/tN.json`).
//!
//! Les lignes du CLI passent par le décodeur (`Inbound::from_line`) puis par
//! le réducteur. Les lignes du pilote de capture sont traduites en ce que
//! ferait le superviseur de l'app à leur place :
//! - `>>> {"type":"user",…}` : message tapé (`user_message` `Typed`, puis
//!   `status` `Requesting`), comme `send_message` ;
//! - `>>> control_request` `initialize` : réponse attendue (produit `ready`) ;
//!   `interrupt` : passage en `Interrupting` pendant un tour ;
//! - `>>> control_response` à une question en attente : `question_closed`
//!   (`Answered`, ou `Skipped` pour un « ignorer », `t10`) ;
//! - `STDERR …` : événement `stderr` ; `EXIT n` : `exited` puis `Exited`.
//!
//! Les autres envois du pilote (allow ou deny d'un outil, `set_model`) ne
//! produisent rien : l'app répond elle-même aux `can_use_tool` (décision 7),
//! et le réducteur l'a déjà fait.
//!
//! Horloge : les captures ne sont pas horodatées ligne à ligne. `t` vaut
//! `REPLAY_T0 + REPLAY_STEP_MS × numéro de ligne` (ligne de la capture,
//! à partir de 0) : déterministe, croissant, et réaliste pour les durées des
//! agents et le rejeu temporisé du mode simulé.

use std::collections::VecDeque;

use serde_json::Value;

use crate::api::events::{CoreEvent, EventBody, NoticeLevel};
use crate::api::types::SessionKey;
use crate::protocol::fixture::{classify, FixtureLine};
use crate::protocol::inbound::Inbound;

use super::pending::close_reason_of_response;
use super::reducer::{Action, Reducer};

/// Captures rejouées en `fixtures/ui-events/` : les 13 captures réelles
/// (`t2` n'existe pas). `t15`, générée (10 505 lignes), n'en fait pas partie :
/// le mode simulé a sa propre session longue (`?mock=long`).
pub const UI_EVENT_FIXTURES: &[&str] = &[
    "t1", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10", "t11", "t12", "t13", "t14",
];

/// 2026-11-16 10:00:00 UTC, jour des captures.
pub const REPLAY_T0: u64 = 1_794_823_200_000;
/// Écart entre deux lignes de capture.
pub const REPLAY_STEP_MS: u64 = 20;
/// Lignes de stderr gardées pour `exited.stderr_tail` (comme le superviseur).
const STDERR_TAIL: usize = 20;

/// Résultat d'un rejeu.
#[derive(Debug, Default)]
pub struct Replay {
    pub events: Vec<CoreEvent>,
    /// Lignes que l'app aurait écrites sur stdin (réponses automatiques).
    pub writes: Vec<String>,
    /// Lignes du CLI illisibles (JSON invalide).
    pub bad_lines: usize,
    /// Le réducteur en fin de rejeu (compteurs, agents, questions).
    pub reducer: Reducer,
}

/// Rejoue le texte d'une capture. `session` devient le `session` de chaque
/// événement (le nom de la fixture, par exemple).
pub fn replay_fixture(text: &str, session: &str) -> Replay {
    let session = SessionKey(session.to_string());
    let mut rep = Replay::default();
    let mut stderr: VecDeque<String> = VecDeque::new();
    let mut seq = 0u64;
    for (n, raw) in text.lines().enumerate() {
        let t = REPLAY_T0 + REPLAY_STEP_MS * n as u64;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let r = &mut rep.reducer;
        let actions: Vec<Action> = match classify(line) {
            FixtureLine::Blank => continue,
            FixtureLine::Cli(l) => match Inbound::from_line(l.trim()) {
                Ok(m) => r.handle_inbound_at(&m, t),
                Err(e) => {
                    rep.bad_lines += 1;
                    vec![Action::Emit(EventBody::Notice {
                        level: NoticeLevel::Warn,
                        code: "bad_json".into(),
                        message: format!("Ligne illisible ignorée : {e}"),
                        detail: Some(Value::String(l.chars().take(500).collect())),
                    })]
                }
            },
            FixtureLine::Sent(l) => match serde_json::from_str::<Value>(l) {
                Ok(v) => sent(r, &v),
                Err(_) => vec![],
            },
            FixtureLine::Stderr(l) => {
                if stderr.len() == STDERR_TAIL {
                    stderr.pop_front();
                }
                stderr.push_back(l.to_string());
                vec![Action::Emit(EventBody::Stderr {
                    line: l.to_string(),
                })]
            }
            FixtureLine::Exit(code) => r.exited(code, stderr.iter().cloned().collect()),
        };
        for a in actions {
            match a {
                Action::Emit(body) => {
                    rep.events.push(CoreEvent {
                        seq,
                        t,
                        session: session.clone(),
                        body,
                    });
                    seq += 1;
                }
                Action::Write(w) => rep.writes.push(w),
                Action::Resolve { .. } => {}
            }
        }
    }
    rep
}

/// Ligne `>>> ` du pilote : ce que le superviseur aurait fait en l'envoyant.
fn sent(r: &mut Reducer, v: &Value) -> Vec<Action> {
    let str_at = |p: &str| v.pointer(p).and_then(Value::as_str);
    match str_at("/type") {
        Some("user") => {
            let text = match v.pointer("/message/content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            r.user_sent(&text)
        }
        Some("control_request") => {
            let mut out = Vec::new();
            match (str_at("/request/subtype"), str_at("/request_id")) {
                (Some("initialize"), Some(id)) => r.expect_initialize(id),
                (Some("interrupt"), _) => {
                    r.begin_interrupt(&mut out);
                }
                _ => {}
            }
            out
        }
        Some("control_response") => {
            let Some(id) = str_at("/response/request_id") else {
                return vec![];
            };
            if !r.questions.contains_key(id) {
                return vec![];
            }
            let reason =
                close_reason_of_response(v.pointer("/response/response").unwrap_or(&Value::Null));
            r.close_question(id, reason)
        }
        _ => vec![],
    }
}

/// Sérialise des événements en tableau JSON, un événement par ligne (diffs
/// lisibles dans git), avec une fin de ligne finale.
pub fn to_json_lines(events: &[CoreEvent]) -> String {
    let mut s = String::from("[\n");
    for (i, e) in events.iter().enumerate() {
        s.push_str(&serde_json::to_string(e).expect("sérialisation d'un CoreEvent"));
        if i + 1 < events.len() {
            s.push(',');
        }
        s.push('\n');
    }
    s.push_str("]\n");
    s
}
