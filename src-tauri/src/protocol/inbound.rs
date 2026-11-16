//! Messages lus sur le stdout de `claude.exe` (spec 4.3 à 4.7bis).
//!
//! Types : `SDKMessage` et `StdoutMessage` de `.claude-work/reference/sdk.d.ts`
//! (SDK 0.3.325), restreints aux champs utiles et vérifiés sur les fixtures
//! `fixtures/stream-json/t1`, `t3` à `t15`.
//!
//! Tolérance (plan 1.4 et 1.9) : [`Inbound::from_value`] ne peut pas échouer.
//! - un `type` inconnu donne [`Inbound::Unknown`] ; un sous-type `system/*`,
//!   un événement `stream_event`, un delta, un bloc de contenu ou une requête
//!   de contrôle inconnus ont chacun leur variante `Unknown` ;
//! - un champ absent, nul ou d'un type inattendu donne `None` (module
//!   [`super::lenient`]) ; les champs non déclarés sont ignorés.
//!
//! [`Inbound::unknown_kinds`] liste tout ce qui n'a pas été reconnu dans un
//! message : c'est ce que le rejeu des fixtures vérifie (aucun inconnu).
//!
//! Les types ci-dessous sont internes au cœur : ils ne passent pas le contrat
//! (l'interface ne voit que des `CoreEvent`, plan 1.5).

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use super::control::{InControlRequest, InControlRequestBody, InControlResponse};
use super::lenient::{self, from_value, obj, opt, raw, str_field, strings, vec_or_default};

/// Types de premier niveau décodés en variantes typées.
pub const DECODED_TYPES: &[&str] = &[
    "system",
    "stream_event",
    "assistant",
    "user",
    "result",
    "rate_limit_event",
    "control_request",
    "control_response",
    "control_cancel_request",
    "tool_progress",
];

/// Types de premier niveau définis par `sdk.d.ts` (`StdoutMessage`), reconnus
/// mais sans contenu décodé : [`Inbound::Ignored`]. Aucun n'est capturé dans
/// les fixtures.
pub const IGNORED_TYPES: &[&str] = &[
    "tool_use_summary",
    "keep_alive",
    "auth_status",
    "prompt_suggestion",
    "conversation_reset",
    "active_goal",
];

/// Sous-types `system/*` décodés.
pub const DECODED_SYSTEM_SUBTYPES: &[&str] = &[
    "init",
    "status",
    "task_started",
    "task_progress",
    "task_updated",
    "task_notification",
    "background_tasks_changed",
    "hook_started",
    "hook_progress",
    "hook_response",
    "thinking_tokens",
];

/// Sous-types `system/*` définis par `sdk.d.ts` (`SDKMessage`), reconnus mais
/// sans contenu décodé : [`SystemMessage::Ignored`]. Aucun n'est capturé.
pub const IGNORED_SYSTEM_SUBTYPES: &[&str] = &[
    "compact_boundary",
    "api_retry",
    "control_request_progress",
    "model_refusal_fallback",
    "model_refusal_no_fallback",
    "local_command_output",
    "plugin_install",
    "session_state_changed",
    "worker_shutting_down",
    "commands_changed",
    "notification",
    "files_persisted",
    "memory_recall",
    "elicitation_complete",
    "permission_denied",
    "mirror_error",
    "informational",
    "session_writes_lost",
];

/// Une ligne de stdout décodée.
///
/// Valeur de passage (une par ligne, jamais stockée en masse) : la taille de
/// la plus grande variante ne justifie pas une allocation de plus par ligne.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    System(SystemMessage),
    StreamEvent(StreamEventMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    Result(ResultMessage),
    RateLimitEvent(RateLimitEvent),
    /// Requête du CLI vers l'app (`can_use_tool`, …), spec 4.5.
    ControlRequest(InControlRequest),
    /// Réponse du CLI à l'une de nos requêtes.
    ControlResponse(InControlResponse),
    ControlCancelRequest(ControlCancelRequest),
    ToolProgress(ToolProgress),
    /// Type de [`IGNORED_TYPES`] : connu, sans effet.
    Ignored(String),
    /// Type inconnu, ou ligne sans `type` textuel (`None`).
    Unknown(Option<String>),
}

impl Inbound {
    /// Décode une valeur JSON déjà lue. Ne peut pas échouer.
    pub fn from_value(v: &Value) -> Inbound {
        let Some(kind) = str_field(v, "type") else {
            return Inbound::Unknown(None);
        };
        match kind {
            "system" => Inbound::System(SystemMessage::from_value(v)),
            "stream_event" => Inbound::StreamEvent(from_value(v)),
            "assistant" => Inbound::Assistant(from_value(v)),
            "user" => Inbound::User(from_value(v)),
            "result" => Inbound::Result(from_value(v)),
            "rate_limit_event" => Inbound::RateLimitEvent(from_value(v)),
            "control_request" => Inbound::ControlRequest(InControlRequest::from_value(v)),
            "control_response" => Inbound::ControlResponse(from_value(v)),
            "control_cancel_request" => Inbound::ControlCancelRequest(from_value(v)),
            "tool_progress" => Inbound::ToolProgress(from_value(v)),
            k if IGNORED_TYPES.contains(&k) => Inbound::Ignored(k.to_string()),
            k => Inbound::Unknown(Some(k.to_string())),
        }
    }

    /// Décode une ligne de texte. Seule une erreur de syntaxe JSON échoue.
    ///
    /// Chemin rapide du flux : pas de `Value` intermédiaire pour toute la
    /// ligne. Une première passe lit `type` et `subtype` (le reste est sauté
    /// sans allocation), la seconde lit la structure du type. Le résultat est
    /// identique à `from_value` sur la même ligne (vérifié sur les fixtures).
    pub fn from_line(line: &str) -> Result<Inbound, serde_json::Error> {
        if !line.trim_start().starts_with('{') {
            // JSON valide mais pas un objet (`[]`, `5`) : pas de type.
            serde_json::from_str::<serde::de::IgnoredAny>(line)?;
            return Ok(Inbound::Unknown(None));
        }
        let tag: Tag = match serde_json::from_str(line) {
            Ok(t) => t,
            // JSON valide mais pas un objet (`[]`, `5`) : pas de type.
            Err(e) if e.classify() == serde_json::error::Category::Data => {
                return Ok(Inbound::Unknown(None))
            }
            Err(e) => return Err(e),
        };
        let src = Line(line);
        let Some(kind) = tag.kind else {
            return Ok(Inbound::Unknown(None));
        };
        Ok(match kind.as_str() {
            "system" => Inbound::System(match tag.subtype.as_deref() {
                None => SystemMessage::Unknown(None),
                Some(sub) => SystemMessage::from_subtype(sub, src),
            }),
            "stream_event" => Inbound::StreamEvent(src.read()),
            "assistant" => Inbound::Assistant(src.read()),
            "user" => Inbound::User(src.read()),
            "result" => Inbound::Result(src.read()),
            "rate_limit_event" => Inbound::RateLimitEvent(src.read()),
            "control_request" => {
                let r: RawControlRequest = src.read();
                Inbound::ControlRequest(InControlRequest {
                    request_id: r.request_id,
                    request: InControlRequestBody::from_value(r.request.as_ref()),
                })
            }
            "control_response" => Inbound::ControlResponse(src.read()),
            "control_cancel_request" => Inbound::ControlCancelRequest(src.read()),
            "tool_progress" => Inbound::ToolProgress(src.read()),
            k if IGNORED_TYPES.contains(&k) => Inbound::Ignored(k.to_string()),
            _ => Inbound::Unknown(Some(kind)),
        })
    }

    /// Tout ce qui n'a pas été reconnu dans ce message, sous la forme
    /// `type`, `system/sous-type`, `stream_event/…`, `bloc/…`, etc.
    pub fn unknown_kinds(&self) -> Vec<String> {
        let mut out = Vec::new();
        let name = |k: &Option<String>| k.clone().unwrap_or_else(|| "?".into());
        match self {
            Inbound::Unknown(k) => out.push(name(k)),
            Inbound::System(SystemMessage::Unknown(k)) => out.push(format!("system/{}", name(k))),
            Inbound::StreamEvent(m) => {
                if let Some(ev) = &m.event {
                    if let StreamEventKind::Unknown(k) = &ev.kind {
                        out.push(format!("stream_event/{}", name(k)));
                    }
                    // Seul le delta de `content_block_delta` a un `type`.
                    if let (StreamEventKind::ContentBlockDelta, Some(Delta::Unknown(k))) =
                        (&ev.kind, &ev.delta)
                    {
                        out.push(format!("stream_event/delta/{}", name(k)));
                    }
                    if let Some(ContentBlock::Unknown(b)) = &ev.content_block {
                        out.push(format!("stream_event/bloc/{}", block_name(b)));
                    }
                }
            }
            Inbound::Assistant(m) => {
                for b in m.message.iter().flat_map(|m| m.content.iter().flatten()) {
                    if let ContentBlock::Unknown(b) = b {
                        out.push(format!("assistant/bloc/{}", block_name(b)));
                    }
                }
            }
            Inbound::User(m) => {
                if let Some(UserContent::Blocks(bs)) =
                    m.message.as_ref().and_then(|m| m.content.as_ref())
                {
                    for b in bs {
                        if let ContentBlock::Unknown(b) = b {
                            out.push(format!("user/bloc/{}", block_name(b)));
                        }
                    }
                }
            }
            Inbound::ControlRequest(r) => {
                if let Some(k) = r.request.unknown_subtype() {
                    out.push(format!("control_request/{k}"));
                }
            }
            _ => {}
        }
        out
    }
}

/// Source d'un message : valeur déjà lue, ou ligne de texte (lue une seconde
/// fois, directement dans la structure du type).
trait Source: Copy {
    fn read<T: DeserializeOwned + Default>(self) -> T;
}

impl Source for &Value {
    fn read<T: DeserializeOwned + Default>(self) -> T {
        from_value(self)
    }
}

#[derive(Clone, Copy)]
struct Line<'a>(&'a str);

impl Source for Line<'_> {
    fn read<T: DeserializeOwned + Default>(self) -> T {
        // La syntaxe a été vérifiée par la première passe ; une structure
        // tolérante lue depuis un objet ne peut pas échouer autrement.
        serde_json::from_str(self.0).unwrap_or_default()
    }
}

/// Première passe de [`Inbound::from_line`].
#[derive(Deserialize)]
struct Tag {
    #[serde(rename = "type", default, deserialize_with = "opt")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "opt")]
    subtype: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawControlRequest {
    #[serde(deserialize_with = "opt")]
    request_id: Option<String>,
    #[serde(deserialize_with = "raw")]
    request: Option<Value>,
}

fn block_name(b: &Value) -> String {
    str_field(b, "type").unwrap_or("?").to_string()
}

// ---------------------------------------------------------------------------
// system/*

#[derive(Debug, Clone, PartialEq)]
pub enum SystemMessage {
    Init(SystemInit),
    Status(SystemStatus),
    TaskStarted(TaskStarted),
    TaskProgress(TaskProgress),
    TaskUpdated(TaskUpdated),
    TaskNotification(TaskNotification),
    BackgroundTasksChanged(BackgroundTasksChanged),
    /// `hook_started`, `hook_progress`, `hook_response`.
    Hook(HookEvent),
    ThinkingTokens(ThinkingTokens),
    /// Sous-type de [`IGNORED_SYSTEM_SUBTYPES`].
    Ignored(String),
    /// Sous-type inconnu, ou absent (`None`).
    Unknown(Option<String>),
}

impl SystemMessage {
    pub fn from_value(v: &Value) -> SystemMessage {
        match str_field(v, "subtype") {
            None => SystemMessage::Unknown(None),
            Some(sub) => SystemMessage::from_subtype(sub, v),
        }
    }

    fn from_subtype<S: Source>(sub: &str, src: S) -> SystemMessage {
        match sub {
            "init" => SystemMessage::Init(src.read()),
            "status" => SystemMessage::Status(src.read()),
            "task_started" => SystemMessage::TaskStarted(src.read()),
            "task_progress" => SystemMessage::TaskProgress(src.read()),
            "task_updated" => SystemMessage::TaskUpdated(src.read()),
            "task_notification" => SystemMessage::TaskNotification(src.read()),
            "background_tasks_changed" => SystemMessage::BackgroundTasksChanged(src.read()),
            "hook_started" | "hook_progress" | "hook_response" => SystemMessage::Hook(src.read()),
            "thinking_tokens" => SystemMessage::ThinkingTokens(src.read()),
            s if IGNORED_SYSTEM_SUBTYPES.contains(&s) => SystemMessage::Ignored(s.to_string()),
            s => SystemMessage::Unknown(Some(s.to_string())),
        }
    }
}

/// `system/init` : émis après le premier message, puis à chaque tour (spec 4.3).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SystemInit {
    #[serde(deserialize_with = "opt")]
    pub session_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub model: Option<String>,
    #[serde(rename = "permissionMode", deserialize_with = "opt")]
    pub permission_mode: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub cwd: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub claude_code_version: Option<String>,
    #[serde(deserialize_with = "strings")]
    pub capabilities: Vec<String>,
    #[serde(deserialize_with = "strings")]
    pub tools: Vec<String>,
    #[serde(deserialize_with = "vec_or_default")]
    pub mcp_servers: Vec<McpServerStatus>,
    #[serde(deserialize_with = "strings")]
    pub skills: Vec<String>,
    #[serde(deserialize_with = "strings")]
    pub slash_commands: Vec<String>,
    /// Noms bruts, parfois entourés de U+00A0 (spec 4.8) : `trim()` à l'usage.
    #[serde(deserialize_with = "strings")]
    pub agents: Vec<String>,
    #[serde(deserialize_with = "opt")]
    pub uuid: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct McpServerStatus {
    #[serde(deserialize_with = "opt")]
    pub name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub status: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub source: Option<String>,
}

/// `system/status` : `{"status":"requesting"}` au début de chaque requête API.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SystemStatus {
    #[serde(deserialize_with = "opt")]
    pub status: Option<String>,
    #[serde(rename = "permissionMode", deserialize_with = "opt")]
    pub permission_mode: Option<String>,
}

/// `system/task_started` (spec 4.4, `t3`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TaskStarted {
    #[serde(deserialize_with = "opt")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub run_id: Option<String>,
    /// Id du `tool_use` Agent : clé de liaison avec le fil du sous-agent.
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
    /// Brut, parfois entouré de U+00A0 (spec 4.8).
    #[serde(deserialize_with = "opt")]
    pub subagent_type: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub is_backgrounded: Option<bool>,
    #[serde(deserialize_with = "opt")]
    pub task_type: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub spawn_depth: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub prompt: Option<String>,
}

/// `usage` de `task_progress` et `task_notification`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TaskUsage {
    #[serde(deserialize_with = "opt")]
    pub total_tokens: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub tool_uses: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub duration_ms: Option<u64>,
}

/// `system/task_progress`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TaskProgress {
    #[serde(deserialize_with = "opt")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub subagent_type: Option<String>,
    #[serde(deserialize_with = "obj")]
    pub usage: Option<TaskUsage>,
    #[serde(deserialize_with = "opt")]
    pub last_tool_name: Option<String>,
}

/// `system/task_updated` : `patch` partiel (`status`, `end_time`…).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TaskUpdated {
    #[serde(deserialize_with = "opt")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "raw")]
    pub patch: Option<Value>,
}

/// `system/task_notification` : fin d'un sous-agent ou d'une tâche.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TaskNotification {
    #[serde(deserialize_with = "opt")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub status: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub summary: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub output_file: Option<String>,
    #[serde(deserialize_with = "obj")]
    pub usage: Option<TaskUsage>,
}

/// `system/background_tasks_changed` : liste des tâches actives.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct BackgroundTasksChanged {
    #[serde(deserialize_with = "raw")]
    pub tasks: Option<Value>,
}

/// `system/hook_started`, `hook_progress`, `hook_response`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookEvent {
    #[serde(deserialize_with = "opt")]
    pub subtype: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub hook_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub hook_name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub hook_event: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub outcome: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub exit_code: Option<i64>,
}

/// `system/thinking_tokens`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThinkingTokens {
    #[serde(deserialize_with = "opt")]
    pub estimated_tokens: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub estimated_tokens_delta: Option<i64>,
}

// ---------------------------------------------------------------------------
// stream_event

/// `stream_event` : enveloppe d'un événement de l'API Messages (spec 4.3).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StreamEventMessage {
    #[serde(deserialize_with = "obj")]
    pub event: Option<StreamEvent>,
    /// `null` dans le fil principal, id du `tool_use` Agent sinon (spec 4.4).
    #[serde(deserialize_with = "opt")]
    pub parent_tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub api_message_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub uuid: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub session_id: Option<String>,
}

/// Événement de l'API : champs à plat, la variante est dans [`Self::kind`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StreamEvent {
    #[serde(rename = "type", deserialize_with = "stream_event_kind")]
    pub kind: StreamEventKind,
    #[serde(deserialize_with = "opt")]
    pub index: Option<u64>,
    /// `message_start`.
    #[serde(deserialize_with = "obj")]
    pub message: Option<ApiMessage>,
    /// `content_block_start`.
    #[serde(deserialize_with = "content_block_opt")]
    pub content_block: Option<ContentBlock>,
    /// `content_block_delta` (et `delta` brut de `message_delta`, voir `kind`).
    #[serde(deserialize_with = "delta_opt")]
    pub delta: Option<Delta>,
    #[serde(deserialize_with = "raw")]
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum StreamEventKind {
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
    Unknown(Option<String>),
    /// Valeur par défaut d'un événement sans `type`.
    #[default]
    Missing,
}

fn stream_event_kind<'de, D: serde::Deserializer<'de>>(d: D) -> Result<StreamEventKind, D::Error> {
    let k: Option<String> = opt(d)?;
    Ok(match k.as_deref() {
        Some("message_start") => StreamEventKind::MessageStart,
        Some("content_block_start") => StreamEventKind::ContentBlockStart,
        Some("content_block_delta") => StreamEventKind::ContentBlockDelta,
        Some("content_block_stop") => StreamEventKind::ContentBlockStop,
        Some("message_delta") => StreamEventKind::MessageDelta,
        Some("message_stop") => StreamEventKind::MessageStop,
        _ => StreamEventKind::Unknown(k),
    })
}

/// Delta d'un `content_block_delta`. Le `delta` de `message_delta`
/// (`stop_reason`…) n'a pas de `type` : il donne `Unknown(None)` et n'est pas
/// compté comme inconnu par [`Inbound::unknown_kinds`].
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    TextDelta { text: Option<String> },
    InputJsonDelta { partial_json: Option<String> },
    ThinkingDelta { thinking: Option<String> },
    SignatureDelta,
    CitationsDelta,
    Unknown(Option<String>),
}

impl Delta {
    pub fn from_value(v: &Value) -> Delta {
        let text = |k| str_field(v, k).map(str::to_string);
        match str_field(v, "type") {
            Some("text_delta") => Delta::TextDelta { text: text("text") },
            Some("input_json_delta") => Delta::InputJsonDelta {
                partial_json: text("partial_json"),
            },
            Some("thinking_delta") => Delta::ThinkingDelta {
                thinking: text("thinking"),
            },
            Some("signature_delta") => Delta::SignatureDelta,
            Some("citations_delta") => Delta::CitationsDelta,
            other => Delta::Unknown(other.map(str::to_string)),
        }
    }
}

fn delta_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Delta>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(Some(Delta::from_value(&v)))
}

// ---------------------------------------------------------------------------
// Messages de l'API et blocs de contenu

/// `message` d'un `assistant` ou d'un `message_start`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiMessage {
    #[serde(deserialize_with = "opt")]
    pub id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub model: Option<String>,
    /// `None` si absent ou si ce n'est pas un tableau.
    #[serde(deserialize_with = "blocks_opt")]
    pub content: Option<Vec<ContentBlock>>,
    #[serde(deserialize_with = "opt")]
    pub stop_reason: Option<String>,
    #[serde(deserialize_with = "raw")]
    pub usage: Option<Value>,
}

/// Bloc de contenu d'un message (`assistant`, `user`, `content_block_start`).
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text {
        text: Option<String>,
    },
    /// `thinking` est vide quand seule la signature est transmise (spec 4.8).
    Thinking {
        thinking: Option<String>,
    },
    RedactedThinking,
    ToolUse(ToolUseBlock),
    ServerToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Image {
        media_type: Option<String>,
    },
    /// Type inconnu, ou élément qui n'est pas un objet : valeur brute.
    Unknown(Value),
}

impl ContentBlock {
    pub fn from_value(v: &Value) -> ContentBlock {
        ContentBlock::from_owned(v.clone())
    }

    /// Décode un bloc en déplaçant ses chaînes (pas de copie du texte).
    pub fn from_owned(mut v: Value) -> ContentBlock {
        let kind = str_field(&v, "type").map(str::to_string);
        match kind.as_deref() {
            Some("text") => ContentBlock::Text {
                text: take_str(&mut v, "text"),
            },
            Some("thinking") => ContentBlock::Thinking {
                thinking: take_str(&mut v, "thinking"),
            },
            Some("redacted_thinking") => ContentBlock::RedactedThinking,
            Some("tool_use") => ContentBlock::ToolUse(owned(v)),
            Some("server_tool_use") => ContentBlock::ServerToolUse(owned(v)),
            Some("tool_result") => ContentBlock::ToolResult(owned(v)),
            Some("image") => ContentBlock::Image {
                media_type: v
                    .get("source")
                    .and_then(|s| str_field(s, "media_type"))
                    .map(str::to_string),
            },
            _ => ContentBlock::Unknown(v),
        }
    }
}

/// Retire `v[k]` s'il s'agit d'une chaîne.
fn take_str(v: &mut Value, k: &str) -> Option<String> {
    match v.as_object_mut()?.remove(k)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// Structure tolérante lue depuis une valeur possédée.
fn owned<T: serde::de::DeserializeOwned + Default>(v: Value) -> T {
    T::deserialize(v).unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToolUseBlock {
    #[serde(deserialize_with = "opt")]
    pub id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub name: Option<String>,
    /// Entrée de l'outil, brute (`null` conservé).
    #[serde(deserialize_with = "raw")]
    pub input: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToolResultBlock {
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "tool_result_content")]
    pub content: Option<ToolResultContent>,
    #[serde(deserialize_with = "opt")]
    pub is_error: Option<bool>,
}

/// `content` d'un `tool_result` : texte seul ou liste de parties.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultContent {
    Text(String),
    Parts(Vec<ResultPart>),
    /// `null`.
    Null,
    /// Ni chaîne ni tableau : valeur brute.
    Other(Value),
}

/// Partie d'un `tool_result`.
#[derive(Debug, Clone, PartialEq)]
pub enum ResultPart {
    Text {
        text: Option<String>,
    },
    Image {
        media_type: Option<String>,
    },
    /// Autre type de partie : valeur brute, relayée telle quelle.
    Other(Value),
}

fn tool_result_content<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ToolResultContent>, D::Error> {
    Ok(Some(match Value::deserialize(d)? {
        Value::Null => ToolResultContent::Null,
        Value::String(s) => ToolResultContent::Text(s),
        Value::Array(parts) => ToolResultContent::Parts(
            parts
                .into_iter()
                .map(|p| match str_field(&p, "type") {
                    Some("text") => ResultPart::Text {
                        text: str_field(&p, "text").map(str::to_string),
                    },
                    Some("image") => ResultPart::Image {
                        media_type: p
                            .get("source")
                            .and_then(|s| str_field(s, "media_type"))
                            .map(str::to_string),
                    },
                    _ => ResultPart::Other(p),
                })
                .collect(),
        ),
        other => ToolResultContent::Other(other),
    }))
}

fn blocks_opt<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<ContentBlock>>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => Some(a.into_iter().map(ContentBlock::from_owned).collect()),
        _ => None,
    })
}

fn content_block_opt<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ContentBlock>, D::Error> {
    Ok(Some(ContentBlock::from_owned(Value::deserialize(d)?)))
}

// ---------------------------------------------------------------------------
// assistant, user

/// `assistant` : message complet ; fait foi sur les deltas (plan 1.5).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AssistantMessage {
    #[serde(deserialize_with = "obj")]
    pub message: Option<ApiMessage>,
    #[serde(deserialize_with = "opt")]
    pub parent_tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub uuid: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub session_id: Option<String>,
}

/// `user` : rejeu d'un message envoyé, texte injecté (interruption), ou
/// résultats d'outils.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UserMessage {
    #[serde(deserialize_with = "obj")]
    pub message: Option<UserMessageBody>,
    #[serde(deserialize_with = "opt")]
    pub parent_tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub uuid: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub session_id: Option<String>,
    #[serde(rename = "isReplay", deserialize_with = "opt")]
    pub is_replay: Option<bool>,
    /// Résultat structuré de l'outil (forme propre à chaque outil, `t13`).
    #[serde(deserialize_with = "raw")]
    pub tool_use_result: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UserMessageBody {
    #[serde(deserialize_with = "opt")]
    pub role: Option<String>,
    #[serde(deserialize_with = "user_content")]
    pub content: Option<UserContent>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
    /// Ni chaîne ni tableau.
    Other(Value),
}

fn user_content<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<UserContent>, D::Error> {
    Ok(Some(match Value::deserialize(d)? {
        Value::String(s) => UserContent::Text(s),
        Value::Array(a) => {
            UserContent::Blocks(a.into_iter().map(ContentBlock::from_owned).collect())
        }
        other => UserContent::Other(other),
    }))
}

// ---------------------------------------------------------------------------
// result, rate_limit_event, tool_progress, control_cancel_request

/// `result` : fin d'un tour (spec 4.3).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ResultMessage {
    #[serde(deserialize_with = "opt")]
    pub subtype: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub is_error: Option<bool>,
    #[serde(deserialize_with = "opt")]
    pub terminal_reason: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub result: Option<String>,
    #[serde(deserialize_with = "strings")]
    pub errors: Vec<String>,
    #[serde(deserialize_with = "opt")]
    pub duration_ms: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub duration_api_ms: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub total_cost_usd: Option<f64>,
    #[serde(deserialize_with = "opt")]
    pub num_turns: Option<u64>,
    #[serde(deserialize_with = "obj")]
    pub usage: Option<ResultUsage>,
    /// `modelUsage` : par modèle, dans l'ordre du flux ; les entrées qui ne
    /// sont pas des objets sont écartées.
    #[serde(rename = "modelUsage", deserialize_with = "model_usage")]
    pub model_usage: Vec<(String, ModelUsage)>,
    #[serde(deserialize_with = "opt")]
    pub permission_denials: Option<Vec<Value>>,
    #[serde(deserialize_with = "opt")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ResultUsage {
    #[serde(deserialize_with = "opt")]
    pub input_tokens: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub output_tokens: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(deserialize_with = "opt")]
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelUsage {
    #[serde(rename = "contextWindow", deserialize_with = "opt")]
    pub context_window: Option<u64>,
    #[serde(rename = "costUSD", deserialize_with = "opt")]
    pub cost_usd: Option<f64>,
}

fn model_usage<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<(String, ModelUsage)>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Object(m) => m
            .into_iter()
            .filter(|(_, v)| v.is_object())
            .map(|(k, v)| (k, lenient::from_value(&v)))
            .collect(),
        _ => Vec::new(),
    })
}

/// `rate_limit_event`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RateLimitEvent {
    #[serde(deserialize_with = "obj")]
    pub rate_limit_info: Option<RateLimitInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RateLimitInfo {
    #[serde(deserialize_with = "opt")]
    pub status: Option<String>,
    #[serde(rename = "resetsAt", deserialize_with = "opt")]
    pub resets_at: Option<u64>,
    #[serde(rename = "rateLimitType", deserialize_with = "opt")]
    pub rate_limit_type: Option<String>,
    #[serde(rename = "unifiedWindows", deserialize_with = "raw")]
    pub unified_windows: Option<Value>,
}

impl RateLimitInfo {
    /// `unifiedWindows[rateLimitType].utilization`.
    pub fn utilization(&self) -> Option<f64> {
        let k = self.rate_limit_type.as_deref()?;
        self.unified_windows
            .as_ref()?
            .get(k)?
            .get("utilization")?
            .as_f64()
    }
}

/// `tool_progress` (`t14`) : battement d'un outil en cours.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToolProgress {
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub tool_name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub parent_tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub elapsed_time_seconds: Option<f64>,
}

/// `control_cancel_request` : le CLI retire l'une de ses requêtes.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ControlCancelRequest {
    #[serde(deserialize_with = "opt")]
    pub request_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_type_and_missing_type() {
        assert_eq!(
            Inbound::from_value(&json!({"type": "brand_new", "x": 1})),
            Inbound::Unknown(Some("brand_new".into()))
        );
        assert_eq!(
            Inbound::from_value(&json!({"x": 1})),
            Inbound::Unknown(None)
        );
        assert_eq!(Inbound::from_value(&json!(42)), Inbound::Unknown(None));
        assert_eq!(
            Inbound::from_value(&json!({"type": "keep_alive"})),
            Inbound::Ignored("keep_alive".into())
        );
    }

    #[test]
    fn unknown_nested_kinds_are_listed() {
        let m = Inbound::from_value(&json!({"type":"system","subtype":"nouveau"}));
        assert_eq!(m.unknown_kinds(), vec!["system/nouveau"]);
        let m = Inbound::from_value(
            &json!({"type":"assistant","message":{"content":[{"type":"text","text":"a"},{"type":"hologram"}]}}),
        );
        assert_eq!(m.unknown_kinds(), vec!["assistant/bloc/hologram"]);
        let m = Inbound::from_value(
            &json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"x_delta"}}}),
        );
        assert_eq!(m.unknown_kinds(), vec!["stream_event/delta/x_delta"]);
        // Le delta de message_delta n'a pas de type : pas un inconnu.
        let m = Inbound::from_value(
            &json!({"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}}}),
        );
        assert!(m.unknown_kinds().is_empty());
        let m = Inbound::from_value(
            &json!({"type":"control_request","request_id":"r","request":{"subtype":"teleport"}}),
        );
        assert_eq!(m.unknown_kinds(), vec!["control_request/teleport"]);
    }

    #[test]
    fn wrong_field_types_do_not_fail() {
        let m = Inbound::from_value(
            &json!({"type":"result","subtype":5,"is_error":"oui","usage":7,"modelUsage":{"a":{"contextWindow":10},"b":3}}),
        );
        let Inbound::Result(r) = m else { panic!() };
        assert_eq!(r.subtype, None);
        assert_eq!(r.is_error, None);
        assert_eq!(
            r.usage,
            Some(ResultUsage::default()),
            "présent mais illisible"
        );
        assert_eq!(r.model_usage.len(), 1);
        assert_eq!(r.model_usage[0].1.context_window, Some(10));
    }

    #[test]
    fn user_content_forms() {
        let Inbound::User(u) =
            Inbound::from_value(&json!({"type":"user","message":{"content":"salut"}}))
        else {
            panic!()
        };
        assert_eq!(
            u.message.unwrap().content,
            Some(UserContent::Text("salut".into()))
        );
        let Inbound::User(u) = Inbound::from_value(
            &json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":"x"},{"type":"weird"}]}]}}),
        ) else {
            panic!()
        };
        let Some(UserContent::Blocks(b)) = u.message.unwrap().content else {
            panic!()
        };
        let ContentBlock::ToolResult(tr) = &b[0] else {
            panic!()
        };
        assert_eq!(tr.tool_use_id.as_deref(), Some("t"));
        let Some(ToolResultContent::Parts(p)) = &tr.content else {
            panic!()
        };
        assert_eq!(
            p[0],
            ResultPart::Text {
                text: Some("x".into())
            }
        );
        assert!(matches!(p[1], ResultPart::Other(_)));
    }

    #[test]
    fn rate_limit_utilization() {
        let Inbound::RateLimitEvent(e) = Inbound::from_value(
            &json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.25}}}}),
        ) else {
            panic!()
        };
        assert_eq!(e.rate_limit_info.unwrap().utilization(), Some(0.25));
    }

    #[test]
    fn bad_json_is_the_only_error() {
        assert!(Inbound::from_line("{pas du json").is_err());
        assert!(Inbound::from_line("[]").is_ok());
    }
}
