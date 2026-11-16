//! Événements `CoreEvent` poussés vers l'interface (section 1.5 du plan).
//!
//! Chaque événement porte `seq` (ordre, détection de trou), `t` (ms depuis
//! l'epoch à la lecture de la ligne) et `session`. Le corps est une union
//! étiquetée par `kind`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::types::SessionKey;

/// `"main"` ou l'id du `tool_use` Agent (`parent_tool_use_id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThreadId(pub String);

impl ThreadId {
    pub fn main() -> Self {
        ThreadId("main".into())
    }
}

/// `message.id` de l'API (`msg_…`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MsgKey(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoreEvent {
    #[ts(type = "number")]
    pub seq: u64,
    #[ts(type = "number")]
    pub t: u64,
    pub session: SessionKey,
    #[serde(flatten)]
    #[ts(flatten)]
    pub body: EventBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum EventBody {
    Starting {
        pid: u32,
        cwd: String,
        args: Vec<String>,
    },
    Ready {
        version: String,
        capabilities: Vec<String>,
        commands: Vec<CommandInfo>,
        agents: Vec<AgentInfo>,
        models: Vec<ModelInfo>,
        permission_mode: Option<String>,
    },
    Init {
        session_id: String,
        model: String,
        permission_mode: String,
        capabilities: Vec<String>,
        tools: Vec<String>,
        mcp_servers: Vec<McpServer>,
        skills: Vec<String>,
        slash_commands: Vec<String>,
        version: Option<String>,
    },
    Status {
        state: SessionState,
    },
    UserMessage {
        thread: ThreadId,
        uuid: Option<String>,
        text: String,
        origin: UserOrigin,
    },
    BlockStart {
        thread: ThreadId,
        msg: MsgKey,
        index: u32,
        block: BlockStartKind,
    },
    TextDelta {
        thread: ThreadId,
        msg: MsgKey,
        index: u32,
        text: String,
    },
    BlockDone {
        thread: ThreadId,
        msg: MsgKey,
        index: u32,
        block: BlockDoneKind,
    },
    ToolResult {
        thread: ThreadId,
        tool_use_id: String,
        content: Vec<ResultPart>,
        is_error: bool,
        meta: Option<Value>,
    },
    Agent(AgentEvent),
    Question {
        request_id: String,
        tool_use_id: Option<String>,
        questions: Vec<QuestionItem>,
        can_skip: bool,
    },
    QuestionClosed {
        request_id: String,
        reason: QuestionCloseReason,
    },
    TurnResult {
        subtype: String,
        is_error: bool,
        terminal_reason: Option<String>,
        #[ts(type = "number")]
        duration_ms: u64,
        cost_usd: f64,
        num_turns: u32,
        #[ts(type = "number | null")]
        context_used: Option<u64>,
        #[ts(type = "number | null")]
        context_window: Option<u64>,
        permission_denials: Vec<Value>,
        result: Option<String>,
    },
    RateLimit {
        status: String,
        #[ts(type = "number | null")]
        resets_at: Option<u64>,
        limit_kind: Option<String>,
        utilization: Option<f64>,
    },
    Notice {
        level: NoticeLevel,
        code: String,
        message: String,
        detail: Option<Value>,
    },
    Stderr {
        line: String,
    },
    Exited {
        code: Option<i32>,
        during_turn: bool,
        stderr_tail: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentInfo {
    /// Nom après `trim()` (le flux l'entoure parfois de U+00A0, spec 4.8).
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelInfo {
    pub value: String,
    pub resolved: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SessionState {
    Idle,
    Requesting,
    Streaming,
    Interrupting,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum UserOrigin {
    Typed,
    Replay,
    Interrupted,
    History,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum BlockStartKind {
    Text,
    Thinking,
    ToolUse { tool_use_id: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum BlockDoneKind {
    Text {
        text: String,
    },
    /// `empty` : rien à afficher. Vrai pour un bloc `thinking` sans texte
    /// (signature seule, spec 4.8) et toujours pour `redacted_thinking`
    /// (contenu chiffré).
    Thinking {
        empty: bool,
    },
    ToolUse {
        tool_use_id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ResultPart {
    Text { text: String },
    Image { media_type: Option<String> },
    Other { raw: Value },
}

/// État **complet** d'un sous-agent, renvoyé à chaque changement :
/// l'interface remplace sa copie, elle ne fusionne pas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentEvent {
    pub tool_use_id: String,
    pub task_id: Option<String>,
    /// `subagent_type` après `trim()` (U+00A0 retiré, spec 4.8).
    pub agent_type: String,
    /// `blue`, `green`, `orange`, `purple`, `pink`, `cyan`, `yellow`, `red`
    /// (`colors::agent_color`) ; `None` pour un agent inconnu (gris).
    pub color: Option<String>,
    pub description: String,
    pub status: AgentStatus,
    pub background: bool,
    /// Heure (ms, horloge des `CoreEvent`) de l'appel `Agent` ou du
    /// `task_started`.
    #[ts(type = "number | null")]
    pub started_at: Option<u64>,
    /// Heure de la ligne qui a terminé l'agent.
    #[ts(type = "number | null")]
    pub ended_at: Option<u64>,
    /// Outil en cours ou dernier outil (`task_progress.last_tool_name`,
    /// `tool_progress`, `tool_use` du fil de l'agent).
    pub last_tool: Option<String>,
    #[ts(type = "number | null")]
    pub tokens: Option<u64>,
    pub tool_uses: Option<u32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentStatus {
    Starting,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestionItem {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum QuestionCloseReason {
    /// Réponse envoyée par `answer_question`.
    Answered,
    /// Requête retirée par le CLI (`control_cancel_request`).
    Cancelled,
    /// Question ignorée par `skip_question` (`annotations[q].unanswered`,
    /// raisons `skipped`, `replied_in_chat` et `dismissed`, `t10`).
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}
