//! Lecteur de transcript (décision 9, plan 1.8) : l'historique d'une session
//! reprise, relu dans `~/.claude/projects/<cwd encodé>/<session_id>.jsonl`.
//!
//! **Isolé** : ce module ne dépend que du contrat (`api`) et du réducteur
//! (`state::reducer`), sans le modifier. Les lignes `user` / `assistant` du
//! fil principal passent par le **même réducteur** que le flux (avec
//! `history = true` : les messages utilisateur prennent l'origine `History`),
//! donc l'interface n'a qu'un code de rendu.
//!
//! **Tolérant** : le format est interne et non documenté. Une ligne illisible
//! ou d'un type inconnu est ignorée et comptée, jamais bloquante. Une
//! dernière ligne inachevée (transcript tronqué, ou en cours d'écriture) est
//! signalée par `truncated`, sans avertissement.
//!
//! Adaptations du transcript au flux, sans toucher au réducteur (lignes
//! synthétiques passées au réducteur) :
//! - `toolUseResult` (transcript) devient `tool_use_result` (flux) : diffs
//!   d'`Edit`, etc. ;
//! - un appel `Agent` lancé en arrière-plan (`toolUseResult.isAsync`, ou
//!   `requestShape: "background"` dans `subagents/agent-<id>.meta.json`)
//!   reçoit un `system/task_started` ; la fin vient du message
//!   `<task-notification>`, converti en `system/task_notification` ;
//! - `system/turn_duration` devient un `result` (ligne de fin de tour) ;
//! - une commande `/x` (`<command-name>`) devient le message `/x args` ;
//! - messages internes (`isMeta`, `<system-reminder>`, sorties de commandes
//!   locales…) : ignorés, comme dans le TUI ;
//! - à la fin, les sous-agents non terminés passent en `Stopped` : le
//!   processus qui les portait n'existe plus.
//!
//! Les fils des sous-agents (`subagents/agent-*.jsonl`) ne sont **pas**
//! relus (bonus du plan) ; leurs `.meta.json` complètent le panneau des
//! agents (type, description, arrière-plan). Les lignes `isSidechain: true`
//! du fichier principal sont ignorées.
//!
//! Lecture seule.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::paths::{subagents_dir, transcript_path};
use crate::api::events::{CoreEvent, EventBody, NoticeLevel};
use crate::api::types::{CoreError, HistoryLoaded, SessionKey};
use crate::state::agents::is_agent_tool;
use crate::state::reducer::{Action, Reducer, INTERRUPTED_TEXT};

/// Plafond d'une ligne de transcript (même valeur que le lecteur du flux).
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Types de lignes connus et sans rien à afficher (plan 1.8 et constaté sur
/// le CLI 2.1.325). Tout autre type est compté comme inconnu.
const KNOWN_SILENT: &[&str] = &[
    "attachment",
    "ai-title",
    "custom-title",
    "last-prompt",
    "summary",
    "queue-operation",
    "file-history-snapshot",
    "mode",
    "permission-mode",
    "session-owner",
    "atis-latch",
    "api-request",
    "api-request-shape",
    "api-request-blob",
    "tools-used",
    "cost-state",
    "progress",
    "agent-name",
    "tag",
    // Constatés dans ~/.claude/projects le 2026-11-16 (métadonnées internes).
    "frame-link",
    "pr-link",
    "bridge-session",
    "worktree-state",
    "relocated",
    "file-history-delta",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
    "agent-setting",
    "continued-in",
    "isolation-latch",
];

/// Début d'un texte injecté par le CLI dans un message `user`, à ne pas
/// afficher.
const INTERNAL_PREFIXES: &[&str] = &[
    "<system-reminder>",
    "<local-command-caveat>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<command-message>",
    "<bash-stdout>",
    "<bash-stderr>",
];

/// `subagents/agent-<id>.meta.json` (constaté : `agentType`, `description`,
/// `toolUseId`, `requestShape`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentMeta {
    /// `<id>` du nom de fichier (`agent-<id>.meta.json`), = `task_id`.
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub tool_use_id: Option<String>,
    pub background: bool,
}

/// Lit les `.meta.json` des sous-agents d'une session et compte leurs fils
/// `agent-*.jsonl`. Dossier absent : rien. Fichier illisible : ignoré.
pub fn read_subagents(dir: &Path) -> (Vec<SubagentMeta>, u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (vec![], 0);
    };
    let mut metas = Vec::new();
    let mut threads = 0;
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    for name in names {
        if name.starts_with("agent-") && name.ends_with(".jsonl") {
            threads += 1;
            continue;
        }
        let Some(id) = name
            .strip_prefix("agent-")
            .and_then(|n| n.strip_suffix(".meta.json"))
        else {
            continue;
        };
        let Some(v) = std::fs::read(dir.join(&name))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        metas.push(SubagentMeta {
            agent_id: id.to_string(),
            agent_type: s("agentType"),
            description: s("description"),
            tool_use_id: s("toolUseId").filter(|t| !t.is_empty()),
            background: s("requestShape").as_deref() == Some("background"),
        });
    }
    (metas, threads)
}

/// Relit l'historique de la session `session_id` du dossier de projet
/// `project_dir`. Erreurs : fichier absent ou illisible (`Io`), id qui n'est
/// pas un UUID (`InvalidArgument`), aucune ligne JSON valide dans un fichier
/// non vide (`TranscriptFormat`). Dans ces cas l'interface reprend la session
/// sans historique affiché, avec un avertissement.
pub fn load_history(
    project_dir: &Path,
    session_id: &str,
    session: &SessionKey,
) -> Result<HistoryLoaded, CoreError> {
    if !super::paths::is_session_id(session_id) {
        return Err(CoreError::InvalidArgument(format!(
            "session_id n'est pas un UUID : {session_id}"
        )));
    }
    let path = transcript_path(project_dir, session_id);
    let bytes =
        std::fs::read(&path).map_err(|e| CoreError::Io(format!("{} : {e}", path.display())))?;
    let (metas, threads) = read_subagents(&subagents_dir(project_dir, session_id));
    let mut h = history_from_bytes(&bytes, &metas, session);
    h.session_id = session_id.to_string();
    h.subagent_threads = threads;
    if !bytes.iter().all(u8::is_ascii_whitespace) && h.lines == h.bad + u32::from(h.truncated) {
        return Err(CoreError::TranscriptFormat(format!(
            "{} : aucune ligne JSON lisible sur {}",
            path.display(),
            h.lines
        )));
    }
    Ok(h)
}

/// Cœur pur du lecteur (testé sans fichier) : octets du transcript ->
/// `CoreEvent` numérotés à partir de 0, horodatés avec le `timestamp` des
/// lignes.
pub fn history_from_bytes(
    bytes: &[u8],
    metas: &[SubagentMeta],
    session: &SessionKey,
) -> HistoryLoaded {
    let mut r = HistoryReader::new(metas);
    let ends_with_newline = bytes.last().is_some_and(|b| *b == b'\n');
    let mut parts: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if ends_with_newline {
        parts.pop();
    }
    let n = parts.len();
    for (i, raw) in parts.into_iter().enumerate() {
        let last_unterminated = i + 1 == n && !ends_with_newline;
        r.line(raw, last_unterminated);
    }
    r.finish(session)
}

struct HistoryReader<'a> {
    reducer: Reducer,
    metas: &'a [SubagentMeta],
    /// Métas déjà appliquées (par `tool_use_id`).
    metas_used: HashSet<String>,
    /// Appels Agent rencontrés : `tool_use_id`.
    agent_calls: HashSet<String>,
    /// `tool_use_id` déjà marqués en arrière-plan.
    backgrounded: HashSet<String>,
    events: Vec<(EventBody, u64)>,
    t: u64,
    stats: HistoryLoaded,
}

impl<'a> HistoryReader<'a> {
    fn new(metas: &'a [SubagentMeta]) -> Self {
        let mut reducer = Reducer::new();
        reducer.history = true;
        HistoryReader {
            reducer,
            metas,
            metas_used: HashSet::new(),
            agent_calls: HashSet::new(),
            backgrounded: HashSet::new(),
            events: Vec::new(),
            t: 0,
            stats: HistoryLoaded::default(),
        }
    }

    fn feed(&mut self, v: &Value) {
        let t = self.t;
        for a in self.reducer.handle_at(v, t) {
            match a {
                // L'état du tour appartient à la session vivante, pas à
                // l'historique.
                Action::Emit(EventBody::Status { .. }) => {}
                Action::Emit(b) => self.events.push((b, t)),
                // Pas de requête de contrôle dans un transcript ; rien à
                // écrire ni à résoudre.
                Action::Write(_) | Action::Resolve { .. } => {}
            }
        }
    }

    fn line(&mut self, raw: &[u8], last_unterminated: bool) {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        self.stats.lines += 1;
        if raw.len() > MAX_LINE_BYTES {
            self.stats.bad += 1;
            return;
        }
        let text = String::from_utf8_lossy(raw);
        let v = match serde_json::from_str::<Value>(&text) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                self.stats.bad += 1;
                return;
            }
            Err(_) if last_unterminated => {
                self.stats.truncated = true;
                return;
            }
            Err(_) => {
                self.stats.bad += 1;
                return;
            }
        };
        if let Some(t) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_ms)
        {
            self.t = t;
        }
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let sidechain = v.get("isSidechain").and_then(Value::as_bool) == Some(true);
        match kind {
            "user" | "assistant" | "system" if sidechain => self.stats.sidechain += 1,
            "user" => self.on_user(v),
            "assistant" => self.on_assistant(&v),
            "system" => self.on_system(&v),
            k if KNOWN_SILENT.contains(&k) => self.stats.ignored += 1,
            _ => {
                self.stats.ignored += 1;
                self.stats.unknown += 1;
            }
        }
    }

    fn on_assistant(&mut self, v: &Value) {
        self.stats.used += 1;
        self.feed(v);
        let calls: Vec<String> = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .filter(|b| {
                b.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(is_agent_tool)
            })
            .filter_map(|b| b.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        for id in calls {
            self.agent_calls.insert(id.clone());
            if let Some(m) = self
                .metas
                .iter()
                .find(|m| m.tool_use_id.as_deref() == Some(&id))
            {
                let m = m.clone();
                self.apply_meta(&m);
            }
        }
    }

    /// `system/task_started` synthétique depuis un `.meta.json` : type,
    /// description, `task_id` et arrière-plan de l'agent.
    fn apply_meta(&mut self, m: &SubagentMeta) {
        let Some(id) = m.tool_use_id.clone() else {
            return;
        };
        if !self.metas_used.insert(id.clone()) {
            return;
        }
        if m.background {
            self.backgrounded.insert(id.clone());
        }
        self.feed(&json!({
            "type": "system",
            "subtype": "task_started",
            "task_id": m.agent_id,
            "tool_use_id": id,
            "subagent_type": m.agent_type,
            "description": m.description,
            "is_backgrounded": m.background,
            "task_type": "local_agent",
        }));
    }

    fn on_system(&mut self, v: &Value) {
        match v.get("subtype").and_then(Value::as_str) {
            // Fin de tour : la ligne « ✻ <Verbe> for <durée> » du fil.
            Some("turn_duration") => {
                self.stats.used += 1;
                let d = v.get("durationMs").and_then(Value::as_u64).unwrap_or(0);
                self.feed(&json!({
                    "type": "result",
                    "subtype": "success",
                    "is_error": false,
                    "duration_ms": d,
                    "num_turns": 0,
                }));
            }
            // Commande `/x` tapée par l'utilisateur.
            Some("local_command") => {
                match v
                    .get("content")
                    .and_then(Value::as_str)
                    .and_then(command_text)
                {
                    Some(text) => {
                        self.stats.used += 1;
                        self.user_text(v, text);
                    }
                    None => self.stats.ignored += 1,
                }
            }
            _ => self.stats.ignored += 1,
        }
    }

    /// Message utilisateur réduit à un texte.
    fn user_text(&mut self, v: &Value, text: String) {
        self.feed(&json!({
            "type": "user",
            "uuid": v.get("uuid").cloned().unwrap_or(Value::Null),
            "message": {"role": "user", "content": text},
        }));
    }

    fn on_user(&mut self, mut v: Value) {
        let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") {
            self.stats.ignored += 1;
            return;
        }
        let human = v.pointer("/origin/kind").and_then(Value::as_str) == Some("human");
        let Some(content) = v.pointer("/message/content").cloned() else {
            self.stats.ignored += 1;
            return;
        };
        match content {
            Value::String(s) => match classify_text(&s, human) {
                UserText::Show(text) => {
                    self.stats.used += 1;
                    self.user_text(&v, text);
                }
                UserText::TaskNotification(n) => {
                    self.stats.used += 1;
                    self.feed(&n);
                }
                UserText::Hide => self.stats.ignored += 1,
            },
            Value::Array(parts) => {
                let mut kept = Vec::with_capacity(parts.len());
                let mut notifications = Vec::new();
                for p in parts {
                    if p.get("type").and_then(Value::as_str) != Some("text") {
                        kept.push(p);
                        continue;
                    }
                    let s = p.get("text").and_then(Value::as_str).unwrap_or("");
                    match classify_text(s, human) {
                        UserText::Show(text) => kept.push(json!({"type": "text", "text": text})),
                        UserText::TaskNotification(n) => notifications.push(n),
                        UserText::Hide => {}
                    }
                }
                if kept.is_empty() && notifications.is_empty() {
                    self.stats.ignored += 1;
                    return;
                }
                self.stats.used += 1;
                self.mark_async_agents(&v, &kept);
                if !kept.is_empty() {
                    let obj = v.as_object_mut().expect("objet vérifié");
                    if let Some(tur) = obj.remove("toolUseResult") {
                        obj.insert("tool_use_result".into(), tur);
                    }
                    if let Some(m) = obj.get_mut("message").and_then(Value::as_object_mut) {
                        m.insert("content".into(), Value::Array(kept));
                    }
                    obj.remove("parent_tool_use_id");
                    self.feed(&v);
                }
                for n in notifications {
                    self.feed(&n);
                }
            }
            _ => self.stats.ignored += 1,
        }
    }

    /// Accusé de lancement d'un agent d'arrière-plan (`toolUseResult.isAsync`)
    /// : l'agent est marqué en arrière-plan **avant** son `tool_result`, sinon
    /// le réducteur le croirait terminé.
    fn mark_async_agents(&mut self, v: &Value, parts: &[Value]) {
        let tur = v.get("toolUseResult");
        let is_async = tur.and_then(|t| t.get("isAsync")).and_then(Value::as_bool) == Some(true)
            || tur.and_then(|t| t.get("status")).and_then(Value::as_str) == Some("async_launched");
        if !is_async {
            return;
        }
        let task_id = tur
            .and_then(|t| t.get("agentId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        for p in parts {
            if p.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(id) = p.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            if !self.agent_calls.contains(id) || !self.backgrounded.insert(id.to_string()) {
                continue;
            }
            let mut m = Map::new();
            m.insert("type".into(), json!("system"));
            m.insert("subtype".into(), json!("task_started"));
            m.insert("tool_use_id".into(), json!(id));
            m.insert("is_backgrounded".into(), json!(true));
            if let Some(t) = &task_id {
                m.insert("task_id".into(), json!(t));
            }
            self.feed(&Value::Object(m));
        }
    }

    fn finish(mut self, session: &SessionKey) -> HistoryLoaded {
        // Sous-agents connus seulement par leur `.meta.json` (appel absent du
        // transcript, par exemple tronqué).
        let rest: Vec<SubagentMeta> = self
            .metas
            .iter()
            .filter(|m| {
                m.tool_use_id
                    .as_ref()
                    .is_some_and(|id| !self.metas_used.contains(id))
            })
            .cloned()
            .collect();
        for m in rest {
            self.apply_meta(&m);
        }
        // Le processus qui portait les agents encore ouverts n'existe plus.
        let t = self.t;
        for a in self.reducer.exited_at(None, vec![], t) {
            if let Action::Emit(b @ EventBody::Agent(_)) = a {
                self.events.push((b, t));
            }
        }
        let mut stats = self.stats;
        stats.unknown_elements = self.reducer.unknown_count as u32;
        if stats.bad > 0 {
            self.events.push((
                EventBody::Notice {
                    level: NoticeLevel::Warn,
                    code: "history_damaged".into(),
                    message: format!(
                        "Historique : {} ligne(s) illisible(s) ignorée(s) sur {}.",
                        stats.bad, stats.lines
                    ),
                    detail: Some(json!({"bad": stats.bad, "lines": stats.lines})),
                },
                t,
            ));
        }
        stats.events = self
            .events
            .into_iter()
            .enumerate()
            .map(|(i, (body, t))| CoreEvent {
                seq: i as u64,
                t,
                session: session.clone(),
                body,
            })
            .collect();
        stats
    }
}

enum UserText {
    Show(String),
    TaskNotification(Value),
    Hide,
}

/// Que faire du texte d'un message `user` du transcript.
fn classify_text(s: &str, human: bool) -> UserText {
    let t = s.trim_start();
    if t.contains("<task-notification>") {
        return match task_notification(t) {
            Some(n) => UserText::TaskNotification(n),
            None => UserText::Hide,
        };
    }
    if let Some(cmd) = command_text(t) {
        return UserText::Show(cmd);
    }
    if t.starts_with("[Request interrupted by user") {
        return UserText::Show(INTERRUPTED_TEXT.to_string());
    }
    if !human && INTERNAL_PREFIXES.iter().any(|p| t.starts_with(p)) {
        return UserText::Hide;
    }
    if t.trim().is_empty() {
        return UserText::Hide;
    }
    UserText::Show(s.to_string())
}

/// Contenu d'une balise `<tag>…</tag>`.
fn tag<'s>(s: &'s str, name: &str) -> Option<&'s str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    Some(s[start..end].trim())
}

/// `<command-name>/x</command-name>…<command-args>a b</command-args>` -> `/x a b`.
fn command_text(s: &str) -> Option<String> {
    let name = tag(s, "command-name").filter(|n| !n.is_empty())?;
    let name = if name.starts_with('/') {
        name.to_string()
    } else {
        format!("/{name}")
    };
    Some(match tag(s, "command-args").filter(|a| !a.is_empty()) {
        Some(args) => format!("{name} {args}"),
        None => name,
    })
}

/// `<task-notification>` (fin d'un agent d'arrière-plan) ->
/// `system/task_notification` du flux.
fn task_notification(s: &str) -> Option<Value> {
    let task_id = tag(s, "task-id");
    let tool_use_id = tag(s, "tool-use-id");
    if task_id.is_none() && tool_use_id.is_none() {
        return None;
    }
    Some(json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": task_id,
        "tool_use_id": tool_use_id,
        "status": tag(s, "status"),
        "summary": tag(s, "summary"),
    }))
}

/// `2026-11-16T12:34:56.789Z` (ou avec décalage `+01:00`) -> ms depuis
/// l'epoch. `None` si la forme n'est pas reconnue.
pub fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    let mut ms = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        let f = &frac[..digits];
        ms = format!("{f:0<3}").get(..3)?.parse().ok()?;
        rest = &frac[digits..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (o.get(1..3)?.parse::<i64>().ok()? * 60 + o.get(4..6)?.parse::<i64>().ok()?)
        }
        _ => return None,
    };
    // Jours depuis 1970-01-01 (algorithme de Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset_min * 60;
    let total = secs * 1000 + ms;
    u64::try_from(total).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-11-16T10:20:30.456Z"),
            Some(1_794_824_430_456)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-16T11:20:30.456+01:00"),
            Some(1_794_824_430_456)
        );
        assert_eq!(parse_rfc3339_ms("pas une date"), None);
    }

    #[test]
    fn commands_and_notifications() {
        assert_eq!(
            command_text("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>haiku</command-args>").as_deref(),
            Some("/model haiku")
        );
        assert_eq!(
            command_text("<command-name>clear</command-name>").as_deref(),
            Some("/clear")
        );
        let n = task_notification("<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n<summary>fini</summary>\n</task-notification>").unwrap();
        assert_eq!(n["subtype"], "task_notification");
        assert_eq!(n["tool_use_id"], "toolu_1");
        assert_eq!(n["status"], "completed");
    }
}
