//! Réducteur : une ligne JSON du CLI -> actions (événements pour l'interface,
//! lignes à écrire sur stdin, réponses à nos requêtes de contrôle).
//!
//! Pur et synchrone : testable sur les fixtures sans processus. Tolérant : un
//! champ manquant ou un type inconnu ne fait jamais paniquer.
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

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn thread_of(v: &Value) -> ThreadId {
    match v.get("parent_tool_use_id").and_then(Value::as_str) {
        Some(id) => ThreadId(id.to_string()),
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

    /// Change d'état et émet `status` si l'état change.
    pub fn set_state(&mut self, st: SessionState, out: &mut Vec<Action>) {
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
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("system") => self.on_system(v, &mut out),
            Some("stream_event") => self.on_stream_event(v, &mut out),
            Some("assistant") => self.on_assistant(v, &mut out),
            Some("user") => self.on_user(v, &mut out),
            Some("result") => self.on_result(v, &mut out),
            Some("rate_limit_event") => self.on_rate_limit(v, &mut out),
            Some("control_request") => self.on_control_request(v, &mut out),
            Some("control_response") => self.on_control_response(v, &mut out),
            Some("control_cancel_request") => {
                if let Some(id) = s(v, "request_id") {
                    if self.questions.remove(&id).is_some() {
                        out.push(Action::Emit(EventBody::QuestionClosed {
                            request_id: id,
                            reason: QuestionCloseReason::Cancelled,
                        }));
                    }
                }
            }
            // Types définis par sdk.d.ts, connus et sans effet en J0
            // (`tool_progress` est capturé dans `t14`).
            Some(
                "tool_progress" | "tool_use_summary" | "keep_alive" | "auth_status"
                | "prompt_suggestion" | "conversation_reset" | "active_goal",
            ) => {}
            _ => self.unknown_count += 1,
        }
        out
    }

    fn on_system(&mut self, v: &Value, out: &mut Vec<Action>) {
        match v.get("subtype").and_then(Value::as_str) {
            Some("init") => {
                self.capabilities = strings(v.get("capabilities"));
                let version = s(v, "claude_code_version");
                if let Some(ver) = &version {
                    self.check_version(ver, out);
                }
                let mcp = v
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|m| McpServer {
                                name: s(m, "name").unwrap_or_default(),
                                status: s(m, "status").unwrap_or_default(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(Action::Emit(EventBody::Init {
                    session_id: s(v, "session_id").unwrap_or_default(),
                    model: s(v, "model").unwrap_or_default(),
                    permission_mode: s(v, "permissionMode").unwrap_or_default(),
                    capabilities: self.capabilities.clone(),
                    tools: strings(v.get("tools")),
                    mcp_servers: mcp,
                    skills: strings(v.get("skills")),
                    slash_commands: strings(v.get("slash_commands")),
                    version,
                }));
            }
            Some("status") => {
                if v.get("status").and_then(Value::as_str) == Some("requesting") {
                    self.set_state(SessionState::Requesting, out);
                }
            }
            Some("task_started") => {
                let agent_type = s(v, "subagent_type").unwrap_or_default().trim().to_string();
                out.push(Action::Emit(EventBody::Agent(AgentEvent {
                    tool_use_id: s(v, "tool_use_id").unwrap_or_default(),
                    task_id: s(v, "task_id"),
                    agent_type,
                    color: None,
                    description: s(v, "description").unwrap_or_default(),
                    status: AgentStatus::Running,
                    background: v
                        .get("is_backgrounded")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    started_at: None,
                    ended_at: None,
                    last_tool: None,
                    tokens: None,
                    tool_uses: None,
                    summary: None,
                })));
            }
            Some("task_notification") => {
                let status = match v.get("status").and_then(Value::as_str) {
                    Some("completed") => AgentStatus::Completed,
                    Some("failed") => AgentStatus::Failed,
                    Some("stopped") | Some("killed") => AgentStatus::Stopped,
                    _ => AgentStatus::Completed,
                };
                let usage = v.get("usage");
                out.push(Action::Emit(EventBody::Agent(AgentEvent {
                    tool_use_id: s(v, "tool_use_id").unwrap_or_default(),
                    task_id: s(v, "task_id"),
                    agent_type: String::new(),
                    color: None,
                    description: String::new(),
                    status,
                    background: false,
                    started_at: None,
                    ended_at: None,
                    last_tool: None,
                    tokens: usage.and_then(|u_| u(u_, "total_tokens")),
                    tool_uses: usage.and_then(|u_| u(u_, "tool_uses")).map(|n| n as u32),
                    summary: s(v, "summary"),
                })));
            }
            // Connus et sans effet en J0 : hook_*, thinking_tokens, task_progress,
            // task_updated, background_tasks_changed, etc.
            _ => {}
        }
    }

    fn on_stream_event(&mut self, v: &Value, out: &mut Vec<Action>) {
        let Some(ev) = v.get("event") else { return };
        let thread = thread_of(v);
        let msg_id = s(v, "api_message_id")
            .or_else(|| ev.get("message").and_then(|m| s(m, "id")))
            .unwrap_or_default();
        let index = u(ev, "index").unwrap_or(0) as u32;
        match ev.get("type").and_then(Value::as_str) {
            Some("message_start") => self.set_state(SessionState::Streaming, out),
            Some("content_block_start") => {
                let cb = ev.get("content_block").cloned().unwrap_or_default();
                let block = match cb.get("type").and_then(Value::as_str) {
                    Some("text") => BlockStartKind::Text,
                    Some("thinking") | Some("redacted_thinking") => BlockStartKind::Thinking,
                    Some("tool_use") | Some("server_tool_use") => BlockStartKind::ToolUse {
                        tool_use_id: s(&cb, "id").unwrap_or_default(),
                        name: s(&cb, "name").unwrap_or_default(),
                    },
                    _ => return,
                };
                out.push(Action::Emit(EventBody::BlockStart {
                    thread,
                    msg: MsgKey(msg_id),
                    index,
                    block,
                }));
            }
            Some("content_block_delta") => {
                let Some(d) = ev.get("delta") else { return };
                if d.get("type").and_then(Value::as_str) == Some("text_delta") {
                    if let Some(text) = s(d, "text") {
                        out.push(Action::Emit(EventBody::TextDelta {
                            thread,
                            msg: MsgKey(msg_id),
                            index,
                            text,
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_assistant(&mut self, v: &Value, out: &mut Vec<Action>) {
        let Some(m) = v.get("message") else { return };
        let msg_id = s(m, "id").unwrap_or_default();
        let thread = thread_of(v);
        let Some(content) = m.get("content").and_then(Value::as_array) else {
            return;
        };
        for b in content {
            let counter = self.assistant_blocks.entry(msg_id.clone()).or_insert(0);
            let index = *counter;
            *counter += 1;
            let block = match b.get("type").and_then(Value::as_str) {
                Some("text") => BlockDoneKind::Text {
                    text: s(b, "text").unwrap_or_default(),
                },
                Some("thinking") | Some("redacted_thinking") => BlockDoneKind::Thinking {
                    empty: s(b, "thinking").map(|t| t.is_empty()).unwrap_or(true),
                },
                Some("tool_use") | Some("server_tool_use") => BlockDoneKind::ToolUse {
                    tool_use_id: s(b, "id").unwrap_or_default(),
                    name: s(b, "name").unwrap_or_default(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                },
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

    fn on_user(&mut self, v: &Value, out: &mut Vec<Action>) {
        let thread = thread_of(v);
        let uuid = s(v, "uuid");
        let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
            return;
        };
        let mut texts = Vec::new();
        match content {
            Value::String(t) => texts.push(t.clone()),
            Value::Array(parts) => {
                for p in parts {
                    match p.get("type").and_then(Value::as_str) {
                        Some("text") => texts.push(s(p, "text").unwrap_or_default()),
                        Some("tool_result") => {
                            out.push(Action::Emit(EventBody::ToolResult {
                                thread: thread.clone(),
                                tool_use_id: s(p, "tool_use_id").unwrap_or_default(),
                                content: result_parts(p.get("content")),
                                is_error: p
                                    .get("is_error")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                meta: v.get("tool_use_result").cloned(),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
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

    fn on_result(&mut self, v: &Value, out: &mut Vec<Action>) {
        let usage = v.get("usage");
        let context_used = usage.map(|u_| {
            u(u_, "input_tokens").unwrap_or(0)
                + u(u_, "cache_creation_input_tokens").unwrap_or(0)
                + u(u_, "cache_read_input_tokens").unwrap_or(0)
        });
        let context_window = v
            .get("modelUsage")
            .and_then(Value::as_object)
            .and_then(|m| m.values().filter_map(|x| u(x, "contextWindow")).max());
        out.push(Action::Emit(EventBody::TurnResult {
            subtype: s(v, "subtype").unwrap_or_default(),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            terminal_reason: s(v, "terminal_reason"),
            duration_ms: u(v, "duration_ms").unwrap_or(0),
            cost_usd: v
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            num_turns: u(v, "num_turns").unwrap_or(0) as u32,
            context_used,
            context_window,
            permission_denials: v
                .get("permission_denials")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            result: s(v, "result"),
        }));
        self.set_state(SessionState::Idle, out);
    }

    fn on_rate_limit(&mut self, v: &Value, out: &mut Vec<Action>) {
        let Some(info) = v.get("rate_limit_info") else {
            return;
        };
        let kind = s(info, "rateLimitType");
        let utilization = kind.as_ref().and_then(|k| {
            info.get("unifiedWindows")
                .and_then(|w| w.get(k))
                .and_then(|w| w.get("utilization"))
                .and_then(Value::as_f64)
        });
        out.push(Action::Emit(EventBody::RateLimit {
            status: s(info, "status").unwrap_or_default(),
            resets_at: u(info, "resetsAt"),
            limit_kind: kind,
            utilization,
        }));
    }

    fn on_control_request(&mut self, v: &Value, out: &mut Vec<Action>) {
        let request_id = s(v, "request_id").unwrap_or_default();
        let req = v.get("request").cloned().unwrap_or_default();
        match req.get("subtype").and_then(Value::as_str) {
            Some("can_use_tool") => {
                let tool = s(&req, "tool_name").unwrap_or_default();
                let input = req.get("input").cloned().unwrap_or(json!({}));
                if tool == "AskUserQuestion" {
                    let questions = parse_questions(&input);
                    self.questions.insert(request_id.clone(), input);
                    out.push(Action::Emit(EventBody::Question {
                        request_id,
                        tool_use_id: s(&req, "tool_use_id"),
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
                            json!({"tool_name": tool, "tool_use_id": s(&req, "tool_use_id")}),
                        ),
                    }));
                }
            }
            other => {
                let sub = other.unwrap_or("?").to_string();
                out.push(Action::Write(outbound::control_error(
                    &request_id,
                    &format!("unsupported control request: {sub}"),
                )));
                out.push(Action::Emit(EventBody::Notice {
                    level: NoticeLevel::Warn,
                    code: "unsupported_control_request".into(),
                    message: format!("Requête de contrôle non gérée : {sub}"),
                    detail: Some(req),
                }));
            }
        }
    }

    fn on_control_response(&mut self, v: &Value, out: &mut Vec<Action>) {
        let Some(r) = v.get("response") else { return };
        let request_id = s(r, "request_id").unwrap_or_default();
        let result = match r.get("subtype").and_then(Value::as_str) {
            Some("success") => Ok(r.get("response").cloned().unwrap_or(Value::Null)),
            _ => Err(s(r, "error").unwrap_or_else(|| "erreur inconnue".into())),
        };
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

fn result_parts(content: Option<&Value>) -> Vec<ResultPart> {
    match content {
        None | Some(Value::Null) => vec![],
        Some(Value::String(t)) => vec![ResultPart::Text { text: t.clone() }],
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => ResultPart::Text {
                    text: s(p, "text").unwrap_or_default(),
                },
                Some("image") => ResultPart::Image {
                    media_type: p.get("source").and_then(|x| s(x, "media_type")),
                },
                _ => ResultPart::Other { raw: p.clone() },
            })
            .collect(),
        Some(other) => vec![ResultPart::Other { raw: other.clone() }],
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
    let arr = |k: &str| {
        resp.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    EventBody::Ready {
        version: s(resp, "claude_code_version").unwrap_or_default(),
        capabilities: strings(resp.get("capabilities")),
        commands: arr("commands")
            .iter()
            .map(|c| CommandInfo {
                name: s(c, "name").unwrap_or_default(),
                description: s(c, "description").unwrap_or_default(),
                argument_hint: s(c, "argumentHint").filter(|h| !h.is_empty()),
            })
            .collect(),
        agents: arr("agents")
            .iter()
            .map(|a| AgentInfo {
                name: s(a, "name").unwrap_or_default().trim().to_string(),
                description: s(a, "description"),
            })
            .collect(),
        models: arr("models")
            .iter()
            .map(|m| ModelInfo {
                value: s(m, "value").unwrap_or_default(),
                resolved: s(m, "resolvedModel"),
                display_name: s(m, "displayName"),
            })
            .collect(),
        permission_mode: s(resp, "current_permission_mode"),
    }
}
