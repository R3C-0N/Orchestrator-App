//! Réducteur : une ligne JSON du CLI -> actions (événements pour l'interface,
//! lignes à écrire sur stdin, réponses à nos requêtes de contrôle).
//!
//! Pur et synchrone : testable sur les fixtures sans processus. Tolérant : un
//! champ manquant ou un type inconnu ne fait jamais paniquer.
//!
//! Décodage : par les types de `protocol::inbound` et `protocol::control`
//! (jalon J2) ; le comportement est celui de J0.
//!
//! J0 : corps provisoires pour `agent` (task_started / task_notification) ; le
//! reste suit la section 1.5 du plan.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::api::events::{
    AgentEvent, AgentInfo, AgentStatus, BlockDoneKind, BlockStartKind, CommandInfo, EventBody,
    McpServer, ModelInfo, MsgKey, NoticeLevel, QuestionCloseReason, QuestionItem, QuestionOption,
    ResultPart, SessionState, ThreadId, UserOrigin,
};
use crate::process::locate::EXPECTED_CLAUDE_VERSION;
use crate::protocol::control::{
    InControlRequest, InControlRequestBody, InControlResponse, InitializeResponse,
};
use crate::protocol::inbound::{
    AssistantMessage, ContentBlock, Delta, Inbound, RateLimitEvent, ResultMessage,
    ResultPart as InPart, StreamEventKind, StreamEventMessage, SystemMessage, ToolResultContent,
    UserContent, UserMessage,
};
use crate::protocol::outbound;

pub const CAP_UNANSWERED: &str = "ask_user_question_unanswered_v1";
pub const INTERRUPTED_TEXT: &str = "[Request interrupted by user]";

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Emit(EventBody),
    /// Ligne à écrire sur stdin.
    Write(String),
    /// Réponse du CLI à l'une de nos requêtes de contrôle.
    Resolve {
        request_id: String,
        result: Result<Value, String>,
    },
}

#[derive(Debug, Default)]
pub struct Reducer {
    pub state: Option<SessionState>,
    /// Blocs `assistant` déjà reçus par `message.id` -> index du prochain bloc.
    assistant_blocks: HashMap<String, u32>,
    /// Questions en attente : request_id -> input brut de l'outil.
    pub questions: HashMap<String, Value>,
    /// request_id de nos requêtes `initialize`.
    initialize_ids: Vec<String>,
    pub capabilities: Vec<String>,
    pub version: Option<String>,
    version_warned: bool,
    /// Lignes de type inconnu, ignorées (plan 1.9 : comptées, pas de notice).
    pub unknown_count: u64,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn thread_of(parent_tool_use_id: &Option<String>) -> ThreadId {
    match parent_tool_use_id {
        Some(id) => ThreadId(id.clone()),
        None => ThreadId::main(),
    }
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclare un `request_id` d'`initialize` envoyé par nous : sa réponse
    /// produira l'événement `ready`.
    pub fn expect_initialize(&mut self, request_id: &str) {
        self.initialize_ids.push(request_id.to_string());
    }

    /// Change d'état et émet `status` si l'état change. `Exited` est terminal :
    /// plus aucun changement d'état n'est émis ensuite.
    pub fn set_state(&mut self, st: SessionState, out: &mut Vec<Action>) {
        if self.state == Some(SessionState::Exited) {
            return;
        }
        if self.state != Some(st) {
            self.state = Some(st);
            out.push(Action::Emit(EventBody::Status { state: st }));
        }
    }

    pub fn in_turn(&self) -> bool {
        matches!(
            self.state,
            Some(SessionState::Requesting | SessionState::Streaming | SessionState::Interrupting)
        )
    }

    /// Début d'une interruption : passe en `Interrupting` seulement pendant un
    /// tour (`Requesting` ou `Streaming`). Renvoie l'état quitté, à restaurer si
    /// l'interruption échoue ; `None` si l'état n'a pas changé.
    pub fn begin_interrupt(&mut self, out: &mut Vec<Action>) -> Option<SessionState> {
        match self.state {
            Some(prev @ (SessionState::Requesting | SessionState::Streaming)) => {
                self.set_state(SessionState::Interrupting, out);
                Some(prev)
            }
            _ => None,
        }
    }

    /// Échec ou délai de l'interruption : revient à `prev` si l'état est
    /// toujours `Interrupting` (un `result` arrivé entre-temps l'emporte).
    pub fn abort_interrupt(&mut self, prev: SessionState, out: &mut Vec<Action>) {
        if self.state == Some(SessionState::Interrupting) {
            self.set_state(prev, out);
        }
    }

    fn check_version(&mut self, version: &str, out: &mut Vec<Action>) {
        self.version = Some(version.to_string());
        if version != EXPECTED_CLAUDE_VERSION && !self.version_warned {
            self.version_warned = true;
            out.push(Action::Emit(EventBody::Notice {
                level: NoticeLevel::Warn,
                code: "unexpected_version".into(),
                message: format!(
                    "claude.exe {version} n'est pas la version testée ({EXPECTED_CLAUDE_VERSION}) ; l'app continue."
                ),
                detail: Some(json!({"found": version, "expected": EXPECTED_CLAUDE_VERSION})),
            }));
        }
    }

    /// Traite une ligne de stdout déjà désérialisée.
    pub fn handle(&mut self, v: &Value) -> Vec<Action> {
        self.handle_inbound(&Inbound::from_value(v))
    }

    /// Traite un message déjà décodé (`protocol::inbound`).
    pub fn handle_inbound(&mut self, m: &Inbound) -> Vec<Action> {
        let mut out = Vec::new();
        match m {
            Inbound::System(sys) => self.on_system(sys, &mut out),
            Inbound::StreamEvent(e) => self.on_stream_event(e, &mut out),
            Inbound::Assistant(a) => self.on_assistant(a, &mut out),
            Inbound::User(u_) => self.on_user(u_, &mut out),
            Inbound::Result(r) => self.on_result(r, &mut out),
            Inbound::RateLimitEvent(e) => self.on_rate_limit(e, &mut out),
            Inbound::ControlRequest(r) => self.on_control_request(r, &mut out),
            Inbound::ControlResponse(r) => self.on_control_response(r, &mut out),
            Inbound::ControlCancelRequest(c) => {
                if let Some(id) = &c.request_id {
                    if self.questions.remove(id).is_some() {
                        out.push(Action::Emit(EventBody::QuestionClosed {
                            request_id: id.clone(),
                            reason: QuestionCloseReason::Cancelled,
                        }));
                    }
                }
            }
            // Types définis par sdk.d.ts, connus et sans effet en J0
            // (`tool_progress` est capturé dans `t14`).
            Inbound::ToolProgress(_) | Inbound::Ignored(_) => {}
            Inbound::Unknown(_) => self.unknown_count += 1,
        }
        out
    }

    fn on_system(&mut self, sys: &SystemMessage, out: &mut Vec<Action>) {
        match sys {
            SystemMessage::Init(i) => {
                self.capabilities = i.capabilities.clone();
                let version = i.claude_code_version.clone();
                if let Some(ver) = &version {
                    self.check_version(ver, out);
                }
                let mcp = i
                    .mcp_servers
                    .iter()
                    .map(|m| McpServer {
                        name: m.name.clone().unwrap_or_default(),
                        status: m.status.clone().unwrap_or_default(),
                    })
                    .collect();
                out.push(Action::Emit(EventBody::Init {
                    session_id: i.session_id.clone().unwrap_or_default(),
                    model: i.model.clone().unwrap_or_default(),
                    permission_mode: i.permission_mode.clone().unwrap_or_default(),
                    capabilities: self.capabilities.clone(),
                    tools: i.tools.clone(),
                    mcp_servers: mcp,
                    skills: i.skills.clone(),
                    slash_commands: i.slash_commands.clone(),
                    version,
                }));
            }
            SystemMessage::Status(st) => {
                if st.status.as_deref() == Some("requesting") {
                    self.set_state(SessionState::Requesting, out);
                }
            }
            SystemMessage::TaskStarted(t) => {
                let agent_type = t
                    .subagent_type
                    .clone()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                out.push(Action::Emit(EventBody::Agent(AgentEvent {
                    tool_use_id: t.tool_use_id.clone().unwrap_or_default(),
                    task_id: t.task_id.clone(),
                    agent_type,
                    color: None,
                    description: t.description.clone().unwrap_or_default(),
                    status: AgentStatus::Running,
                    background: t.is_backgrounded.unwrap_or(false),
                    started_at: None,
                    ended_at: None,
                    last_tool: None,
                    tokens: None,
                    tool_uses: None,
                    summary: None,
                })));
            }
            SystemMessage::TaskNotification(t) => {
                let status = match t.status.as_deref() {
                    Some("completed") => AgentStatus::Completed,
                    Some("failed") => AgentStatus::Failed,
                    Some("stopped") | Some("killed") => AgentStatus::Stopped,
                    _ => AgentStatus::Completed,
                };
                let usage = t.usage.as_ref();
                out.push(Action::Emit(EventBody::Agent(AgentEvent {
                    tool_use_id: t.tool_use_id.clone().unwrap_or_default(),
                    task_id: t.task_id.clone(),
                    agent_type: String::new(),
                    color: None,
                    description: String::new(),
                    status,
                    background: false,
                    started_at: None,
                    ended_at: None,
                    last_tool: None,
                    tokens: usage.and_then(|u_| u_.total_tokens),
                    tool_uses: usage.and_then(|u_| u_.tool_uses).map(|n| n as u32),
                    summary: t.summary.clone(),
                })));
            }
            // Connus et sans effet en J0 : hook_*, thinking_tokens, task_progress,
            // task_updated, background_tasks_changed, etc.
            _ => {}
        }
    }

    fn on_stream_event(&mut self, m: &StreamEventMessage, out: &mut Vec<Action>) {
        let Some(ev) = &m.event else { return };
        let thread = thread_of(&m.parent_tool_use_id);
        let msg_id = m
            .api_message_id
            .clone()
            .or_else(|| ev.message.as_ref().and_then(|x| x.id.clone()))
            .unwrap_or_default();
        let index = ev.index.unwrap_or(0) as u32;
        match ev.kind {
            StreamEventKind::MessageStart => self.set_state(SessionState::Streaming, out),
            StreamEventKind::ContentBlockStart => {
                let block = match &ev.content_block {
                    Some(ContentBlock::Text { .. }) => BlockStartKind::Text,
                    Some(ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking) => {
                        BlockStartKind::Thinking
                    }
                    Some(ContentBlock::ToolUse(t) | ContentBlock::ServerToolUse(t)) => {
                        BlockStartKind::ToolUse {
                            tool_use_id: t.id.clone().unwrap_or_default(),
                            name: t.name.clone().unwrap_or_default(),
                        }
                    }
                    _ => return,
                };
                out.push(Action::Emit(EventBody::BlockStart {
                    thread,
                    msg: MsgKey(msg_id),
                    index,
                    block,
                }));
            }
            StreamEventKind::ContentBlockDelta => {
                if let Some(Delta::TextDelta { text: Some(text) }) = &ev.delta {
                    out.push(Action::Emit(EventBody::TextDelta {
                        thread,
                        msg: MsgKey(msg_id),
                        index,
                        text: text.clone(),
                    }));
                }
            }
            _ => {}
        }
    }

    fn on_assistant(&mut self, a: &AssistantMessage, out: &mut Vec<Action>) {
        let Some(m) = &a.message else { return };
        let msg_id = m.id.clone().unwrap_or_default();
        let thread = thread_of(&a.parent_tool_use_id);
        let Some(content) = &m.content else {
            return;
        };
        for b in content {
            let counter = self.assistant_blocks.entry(msg_id.clone()).or_insert(0);
            let index = *counter;
            *counter += 1;
            let block = match b {
                ContentBlock::Text { text } => BlockDoneKind::Text {
                    text: text.clone().unwrap_or_default(),
                },
                ContentBlock::Thinking { thinking } => BlockDoneKind::Thinking {
                    empty: thinking.as_ref().map(|t| t.is_empty()).unwrap_or(true),
                },
                ContentBlock::RedactedThinking => BlockDoneKind::Thinking { empty: true },
                ContentBlock::ToolUse(t) | ContentBlock::ServerToolUse(t) => {
                    BlockDoneKind::ToolUse {
                        tool_use_id: t.id.clone().unwrap_or_default(),
                        name: t.name.clone().unwrap_or_default(),
                        input: t.input.clone().unwrap_or(Value::Null),
                    }
                }
                _ => continue,
            };
            out.push(Action::Emit(EventBody::BlockDone {
                thread: thread.clone(),
                msg: MsgKey(msg_id.clone()),
                index,
                block,
            }));
        }
    }

    fn on_user(&mut self, m: &UserMessage, out: &mut Vec<Action>) {
        let thread = thread_of(&m.parent_tool_use_id);
        let uuid = m.uuid.clone();
        let Some(content) = m.message.as_ref().and_then(|b| b.content.as_ref()) else {
            return;
        };
        let mut texts = Vec::new();
        match content {
            UserContent::Text(t) => texts.push(t.clone()),
            UserContent::Blocks(parts) => {
                for p in parts {
                    match p {
                        ContentBlock::Text { text } => texts.push(text.clone().unwrap_or_default()),
                        ContentBlock::ToolResult(tr) => {
                            out.push(Action::Emit(EventBody::ToolResult {
                                thread: thread.clone(),
                                tool_use_id: tr.tool_use_id.clone().unwrap_or_default(),
                                content: result_parts(tr.content.as_ref()),
                                is_error: tr.is_error.unwrap_or(false),
                                meta: m.tool_use_result.clone(),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            UserContent::Other(_) => {}
        }
        if texts.is_empty() {
            return;
        }
        let text = texts.join("\n");
        let origin = if text.trim() == INTERRUPTED_TEXT {
            UserOrigin::Interrupted
        } else {
            UserOrigin::Replay
        };
        out.push(Action::Emit(EventBody::UserMessage {
            thread,
            uuid,
            text,
            origin,
        }));
    }

    fn on_result(&mut self, r: &ResultMessage, out: &mut Vec<Action>) {
        let context_used = r.usage.as_ref().map(|u_| {
            u_.input_tokens.unwrap_or(0)
                + u_.cache_creation_input_tokens.unwrap_or(0)
                + u_.cache_read_input_tokens.unwrap_or(0)
        });
        let context_window = r
            .model_usage
            .iter()
            .filter_map(|(_, m)| m.context_window)
            .max();
        out.push(Action::Emit(EventBody::TurnResult {
            subtype: r.subtype.clone().unwrap_or_default(),
            is_error: r.is_error.unwrap_or(false),
            terminal_reason: r.terminal_reason.clone(),
            duration_ms: r.duration_ms.unwrap_or(0),
            cost_usd: r.total_cost_usd.unwrap_or(0.0),
            num_turns: r.num_turns.unwrap_or(0) as u32,
            context_used,
            context_window,
            permission_denials: r.permission_denials.clone().unwrap_or_default(),
            result: r.result.clone(),
        }));
        self.set_state(SessionState::Idle, out);
    }

    fn on_rate_limit(&mut self, e: &RateLimitEvent, out: &mut Vec<Action>) {
        let Some(info) = &e.rate_limit_info else {
            return;
        };
        out.push(Action::Emit(EventBody::RateLimit {
            status: info.status.clone().unwrap_or_default(),
            resets_at: info.resets_at,
            limit_kind: info.rate_limit_type.clone(),
            utilization: info.utilization(),
        }));
    }

    fn on_control_request(&mut self, r: &InControlRequest, out: &mut Vec<Action>) {
        let request_id = r.request_id.clone().unwrap_or_default();
        match &r.request {
            InControlRequestBody::CanUseTool(c) => {
                let tool = c.tool_name.clone().unwrap_or_default();
                let input = c.input.clone().unwrap_or(json!({}));
                if tool == "AskUserQuestion" {
                    let questions = parse_questions(&input);
                    self.questions.insert(request_id.clone(), input);
                    out.push(Action::Emit(EventBody::Question {
                        request_id,
                        tool_use_id: c.tool_use_id.clone(),
                        questions,
                        can_skip: self.capabilities.iter().any(|c| c == CAP_UNANSWERED),
                    }));
                } else {
                    // Décision 7 : allow par défaut, sans passer par l'interface.
                    out.push(Action::Write(outbound::allow(&request_id, &input)));
                    out.push(Action::Emit(EventBody::Notice {
                        level: NoticeLevel::Info,
                        code: "auto_allow".into(),
                        message: format!("Outil {tool} autorisé automatiquement."),
                        detail: Some(
                            json!({"tool_name": tool, "tool_use_id": c.tool_use_id.clone()}),
                        ),
                    }));
                }
            }
            InControlRequestBody::Other { subtype, raw } => {
                let sub = subtype.clone().unwrap_or_else(|| "?".into());
                out.push(Action::Write(outbound::control_error(
                    &request_id,
                    &format!("unsupported control request: {sub}"),
                )));
                out.push(Action::Emit(EventBody::Notice {
                    level: NoticeLevel::Warn,
                    code: "unsupported_control_request".into(),
                    message: format!("Requête de contrôle non gérée : {sub}"),
                    detail: Some(raw.clone()),
                }));
            }
        }
    }

    fn on_control_response(&mut self, c: &InControlResponse, out: &mut Vec<Action>) {
        let Some(r) = &c.response else { return };
        let request_id = r.request_id.clone().unwrap_or_default();
        let result = r.result();
        if let Some(pos) = self.initialize_ids.iter().position(|x| *x == request_id) {
            self.initialize_ids.remove(pos);
            if let Ok(resp) = &result {
                let ready = ready_from_initialize(resp);
                if let EventBody::Ready {
                    version,
                    capabilities,
                    ..
                } = &ready
                {
                    if !version.is_empty() {
                        let ver = version.clone();
                        self.check_version(&ver, out);
                    }
                    if self.capabilities.is_empty() {
                        self.capabilities = capabilities.clone();
                    }
                }
                out.push(Action::Emit(ready));
                self.set_state(SessionState::Idle, out);
            }
        }
        out.push(Action::Resolve { request_id, result });
    }
}

fn result_parts(content: Option<&ToolResultContent>) -> Vec<ResultPart> {
    match content {
        None | Some(ToolResultContent::Null) => vec![],
        Some(ToolResultContent::Text(t)) => vec![ResultPart::Text { text: t.clone() }],
        Some(ToolResultContent::Parts(parts)) => parts
            .iter()
            .map(|p| match p {
                InPart::Text { text } => ResultPart::Text {
                    text: text.clone().unwrap_or_default(),
                },
                InPart::Image { media_type } => ResultPart::Image {
                    media_type: media_type.clone(),
                },
                InPart::Other(raw) => ResultPart::Other { raw: raw.clone() },
            })
            .collect(),
        Some(ToolResultContent::Other(other)) => vec![ResultPart::Other { raw: other.clone() }],
    }
}

pub fn parse_questions(input: &Value) -> Vec<QuestionItem> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .map(|qs| {
            qs.iter()
                .map(|q| QuestionItem {
                    question: s(q, "question").unwrap_or_default(),
                    header: s(q, "header").unwrap_or_default(),
                    options: q
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|os| {
                            os.iter()
                                .map(|o| QuestionOption {
                                    label: s(o, "label").unwrap_or_default(),
                                    description: s(o, "description").unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    multi_select: q
                        .get("multiSelect")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Construit `ready` depuis la réponse à `initialize` (`t11`).
pub fn ready_from_initialize(resp: &Value) -> EventBody {
    let r = InitializeResponse::from_value(resp);
    EventBody::Ready {
        version: r.claude_code_version.unwrap_or_default(),
        capabilities: r.capabilities,
        commands: r
            .commands
            .into_iter()
            .map(|c| CommandInfo {
                name: c.name.unwrap_or_default(),
                description: c.description.unwrap_or_default(),
                argument_hint: c.argument_hint.filter(|h| !h.is_empty()),
            })
            .collect(),
        agents: r
            .agents
            .into_iter()
            .map(|a| AgentInfo {
                name: a.name.unwrap_or_default().trim().to_string(),
                description: a.description,
            })
            .collect(),
        models: r
            .models
            .into_iter()
            .map(|m| ModelInfo {
                value: m.value.unwrap_or_default(),
                resolved: m.resolved_model,
                display_name: m.display_name,
            })
            .collect(),
        permission_mode: r.current_permission_mode,
    }
}
