//! Réducteur : un message du CLI -> actions (événements pour l'interface,
//! lignes à écrire sur stdin, réponses à nos requêtes de contrôle).
//!
//! Pur et synchrone (plan 1.6) : aucune E/S, l'heure est passée en argument
//! (`handle_inbound_at`), ce qui permet de rejouer les fixtures sans
//! processus et d'en tirer des instantanés stables (`state::replay`).
//! Tolérant : un champ manquant ou un type inconnu ne fait jamais paniquer.
//!
//! Décodage par `protocol::inbound` et `protocol::control` (J2). Corrélation
//! (J4) : fils et index de blocs (`threads`), sous-agents (`agents`),
//! questions et `initialize` en attente (`pending`).
//!
//! Choix documentés :
//! - **Thinking** : `block_done` porte `Thinking { empty }`. `empty` vaut
//!   `true` quand il n'y a rien à afficher : bloc `thinking` sans texte
//!   (signature seule, spec 4.8) et **toujours** pour `redacted_thinking`,
//!   dont le contenu est chiffré (`data`) : un éventuel champ `thinking` sur
//!   ce type n'est pas lu (il n'existe pas dans l'API ; le décodeur ne le
//!   garde pas). J0 lisait ce champ et pouvait donner `false` : c'était un
//!   texte non affichable présenté comme affichable.
//! - **Inconnus** (plan 1.9) : chaque élément non reconnu d'un message
//!   (`Inbound::unknown_kinds` : type, sous-type `system/*`, événement de
//!   flux, delta, bloc, requête de contrôle) est compté dans `unknown_count`
//!   et par nom dans `unknown_kinds` ; aucune notice.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::api::events::{
    AgentEvent, AgentInfo, BlockDoneKind, BlockStartKind, CommandInfo, EventBody, McpServer,
    ModelInfo, MsgKey, NoticeLevel, QuestionCloseReason, ResultPart, SessionState, ThreadId,
    UserOrigin,
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

use super::agents::AgentRegistry;
use super::threads::{thread_of, BlockCounters};

pub use super::pending::{parse_questions, CAP_UNANSWERED};

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
    /// Index des blocs des messages `assistant` complets.
    blocks: BlockCounters,
    /// Questions en attente : request_id -> input brut de l'outil.
    pub questions: HashMap<String, Value>,
    /// request_id de nos requêtes `initialize`.
    initialize_ids: Vec<String>,
    pub capabilities: Vec<String>,
    pub version: Option<String>,
    version_warned: bool,
    /// Éléments non reconnus (plan 1.9 : comptés, pas de notice).
    pub unknown_count: u64,
    /// Même compte, par nom (`system/xyz`, `assistant/bloc/xyz`…).
    pub unknown_kinds: BTreeMap<String, u64>,
    /// Sous-agents.
    agents: AgentRegistry,
    /// Rejeu d'un transcript (`load_history`, J11) : les messages
    /// utilisateur prennent l'origine `History`.
    pub history: bool,
    /// Heure de la ligne en cours (ms depuis l'epoch).
    now: u64,
}

/// Heure système en ms depuis l'epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sous-agents connus, dans l'ordre d'apparition.
    pub fn agents(&self) -> &[AgentEvent] {
        self.agents.all()
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

    /// Message tapé par l'utilisateur et écrit sur stdin : `user_message`
    /// `Typed` puis `status` `Requesting` (ce que fait `send_message`).
    pub fn user_sent(&mut self, text: &str) -> Vec<Action> {
        let mut out = vec![Action::Emit(EventBody::UserMessage {
            thread: ThreadId::main(),
            uuid: None,
            text: text.to_string(),
            origin: UserOrigin::Typed,
        })];
        self.set_state(SessionState::Requesting, &mut out);
        out
    }

    /// Ferme une question en attente (réponse envoyée, `Answered` ou
    /// `Skipped`) : la retire de la table et émet `question_closed`. Rien si
    /// la question n'est pas (ou plus) en attente.
    pub fn close_question(&mut self, request_id: &str, reason: QuestionCloseReason) -> Vec<Action> {
        if self.questions.remove(request_id).is_none() {
            return vec![];
        }
        vec![Action::Emit(EventBody::QuestionClosed {
            request_id: request_id.to_string(),
            reason,
        })]
    }

    /// Fin du processus : `exited` (avec `during_turn`), puis `status`
    /// `Exited`, terminal.
    pub fn exited(&mut self, code: Option<i32>, stderr_tail: Vec<String>) -> Vec<Action> {
        let mut out = vec![Action::Emit(EventBody::Exited {
            code,
            during_turn: self.in_turn(),
            stderr_tail,
        })];
        self.set_state(SessionState::Exited, &mut out);
        out
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

    /// Traite une ligne de stdout déjà désérialisée, à l'heure système.
    pub fn handle(&mut self, v: &Value) -> Vec<Action> {
        self.handle_at(v, now_ms())
    }

    /// Traite une ligne de stdout déjà désérialisée, lue à l'heure `t`.
    pub fn handle_at(&mut self, v: &Value, t: u64) -> Vec<Action> {
        self.handle_inbound_at(&Inbound::from_value(v), t)
    }

    /// Traite un message déjà décodé, à l'heure système.
    pub fn handle_inbound(&mut self, m: &Inbound) -> Vec<Action> {
        self.handle_inbound_at(m, now_ms())
    }

    /// Traite un message déjà décodé (`protocol::inbound`), lu à l'heure `t`
    /// (ms depuis l'epoch ; sert à `started_at` / `ended_at` des agents).
    pub fn handle_inbound_at(&mut self, m: &Inbound, t: u64) -> Vec<Action> {
        self.now = t;
        for k in m.unknown_kinds() {
            self.unknown_count += 1;
            *self.unknown_kinds.entry(k).or_insert(0) += 1;
        }
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
                    out.extend(self.close_question(id, QuestionCloseReason::Cancelled));
                }
            }
            Inbound::ToolProgress(p) => {
                emit_agent(&mut out, self.agents.on_tool_progress(p));
            }
            // Types de `sdk.d.ts` connus et sans effet ; inconnus déjà comptés.
            Inbound::Ignored(_) | Inbound::Unknown(_) => {}
        }
        out
    }

    fn on_system(&mut self, sys: &SystemMessage, out: &mut Vec<Action>) {
        let now = self.now;
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
            SystemMessage::TaskStarted(t) => emit_agent(out, self.agents.on_task_started(t, now)),
            SystemMessage::TaskProgress(p) => emit_agent(out, self.agents.on_task_progress(p)),
            SystemMessage::TaskUpdated(u) => {
                let a = self.agents.on_task_updated(u, now);
                self.forget_finished(a.as_ref());
                emit_agent(out, a);
            }
            SystemMessage::TaskNotification(n) => {
                let a = self.agents.on_task_notification(n, now);
                self.forget_finished(a.as_ref());
                emit_agent(out, a);
            }
            SystemMessage::BackgroundTasksChanged(b) => {
                for a in self.agents.on_background_tasks(b.tasks.as_ref()) {
                    out.push(Action::Emit(EventBody::Agent(a)));
                }
            }
            // Connus et sans effet : hook_*, thinking_tokens, sous-types ignorés.
            SystemMessage::Hook(_)
            | SystemMessage::ThinkingTokens(_)
            | SystemMessage::Ignored(_)
            | SystemMessage::Unknown(_) => {}
        }
    }

    /// Un agent fini n'aura plus de message : ses compteurs de blocs partent.
    fn forget_finished(&mut self, a: Option<&AgentEvent>) {
        if let Some(a) = a.filter(|a| a.ended_at.is_some()) {
            self.blocks.clear_thread(&a.tool_use_id);
        }
    }

    fn on_stream_event(&mut self, m: &StreamEventMessage, out: &mut Vec<Action>) {
        let Some(ev) = &m.event else { return };
        let thread = thread_of(m.parent_tool_use_id.as_deref());
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
                // Les `input_json_delta` restent dans le cœur (plan 1.5) :
                // l'`input` complet arrive avec le message `assistant`.
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
        let thread = thread_of(a.parent_tool_use_id.as_deref());
        let Some(content) = &m.content else {
            return;
        };
        for b in content {
            // Tout bloc compte, même inconnu : l'index suit celui de l'API.
            let index = self.blocks.next_index(&thread, &msg_id);
            let block = match b {
                ContentBlock::Text { text } => BlockDoneKind::Text {
                    text: text.clone().unwrap_or_default(),
                },
                ContentBlock::Thinking { thinking } => BlockDoneKind::Thinking {
                    empty: thinking.as_deref().is_none_or(str::is_empty),
                },
                // Contenu chiffré : jamais affichable (voir l'en-tête).
                ContentBlock::RedactedThinking => BlockDoneKind::Thinking { empty: true },
                ContentBlock::ToolUse(t) | ContentBlock::ServerToolUse(t) => {
                    BlockDoneKind::ToolUse {
                        tool_use_id: t.id.clone().unwrap_or_default(),
                        name: t.name.clone().unwrap_or_default(),
                        input: t.input.clone().unwrap_or(Value::Null),
                    }
                }
                // Bloc inconnu (compté) ou inattendu ici : rien à émettre.
                _ => continue,
            };
            let agent_events = match &block {
                BlockDoneKind::ToolUse {
                    tool_use_id,
                    name,
                    input,
                } => {
                    let parent = (thread != ThreadId::main()).then_some(thread.0.as_str());
                    self.agents
                        .on_tool_use(tool_use_id, name, input, parent, self.now)
                }
                _ => vec![],
            };
            out.push(Action::Emit(EventBody::BlockDone {
                thread: thread.clone(),
                msg: MsgKey(msg_id.clone()),
                index,
                block,
            }));
            // Après `block_done` : l'interface crée la ligne de l'outil, puis
            // la transforme en ligne d'agent.
            for ev in agent_events {
                out.push(Action::Emit(EventBody::Agent(ev)));
            }
        }
    }

    fn on_user(&mut self, m: &UserMessage, out: &mut Vec<Action>) {
        let thread = thread_of(m.parent_tool_use_id.as_deref());
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
                            let tool_use_id = tr.tool_use_id.clone().unwrap_or_default();
                            let is_error = tr.is_error.unwrap_or(false);
                            out.push(Action::Emit(EventBody::ToolResult {
                                thread: thread.clone(),
                                tool_use_id: tool_use_id.clone(),
                                content: result_parts(tr.content.as_ref()),
                                is_error,
                                meta: m.tool_use_result.clone(),
                            }));
                            let a = self.agents.on_tool_result(&tool_use_id, is_error, self.now);
                            self.forget_finished(a.as_ref());
                            emit_agent(out, a);
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
        } else if self.history {
            UserOrigin::History
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
        // Un message du fil principal ne chevauche pas deux tours.
        self.blocks.clear_thread("main");
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

fn emit_agent(out: &mut Vec<Action>, a: Option<AgentEvent>) {
    if let Some(a) = a {
        out.push(Action::Emit(EventBody::Agent(a)));
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
