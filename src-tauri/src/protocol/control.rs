//! Protocole de contrôle (spec 4.5 et 4.7bis) : requêtes et réponses dans les
//! deux sens.
//!
//! - CLI vers app : [`InControlRequest`] (`can_use_tool`, …), à quoi l'app
//!   répond par [`ControlResponseBody`] ;
//! - app vers CLI : [`OutControlRequest`] (`initialize`, `interrupt`,
//!   `set_model`, …), à quoi le CLI répond par [`InControlResponse`].
//!
//! Types : `SDKControlRequest`, `SDKControlResponse`, `ControlResponse`,
//! `ControlErrorResponse`, `PermissionResult` de `sdk.d.ts`. La corrélation des
//! `request_id` (table des requêtes en attente, délais) reste dans le
//! superviseur (`process::supervisor`, jalon J6).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::lenient::{from_value, opt, raw, str_field, strings, vec_or_default};

/// Requêtes que le CLI peut nous adresser d'après `sdk.d.ts` (hors
/// `can_use_tool`, seule décodée) ; reconnues, elles reçoivent une réponse
/// d'erreur (« non gérée »).
pub const KNOWN_CLI_REQUEST_SUBTYPES: &[&str] = &[
    "can_use_tool",
    "hook_callback",
    "mcp_message",
    "elicitation",
    "request_user_dialog",
];

// ---------------------------------------------------------------------------
// CLI -> app

/// `{"type":"control_request","request_id":…,"request":{"subtype":…}}` lu sur
/// stdout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InControlRequest {
    pub request_id: Option<String>,
    pub request: InControlRequestBody,
}

impl InControlRequest {
    pub fn from_value(v: &Value) -> InControlRequest {
        InControlRequest {
            request_id: str_field(v, "request_id").map(str::to_string),
            request: InControlRequestBody::from_value(v.get("request")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InControlRequestBody {
    /// Permission d'outil, ou question `AskUserQuestion` (`t4`).
    CanUseTool(Box<CanUseTool>),
    /// Toute autre requête : sous-type et corps bruts (`Null` si absent).
    Other { subtype: Option<String>, raw: Value },
}

impl Default for InControlRequestBody {
    fn default() -> Self {
        InControlRequestBody::Other {
            subtype: None,
            raw: Value::Null,
        }
    }
}

impl InControlRequestBody {
    pub fn from_value(req: Option<&Value>) -> InControlRequestBody {
        let raw = req.cloned().unwrap_or(Value::Null);
        match str_field(&raw, "subtype") {
            Some("can_use_tool") => InControlRequestBody::CanUseTool(Box::new(from_value(&raw))),
            sub => InControlRequestBody::Other {
                subtype: sub.map(str::to_string),
                raw,
            },
        }
    }

    /// Sous-type absent de [`KNOWN_CLI_REQUEST_SUBTYPES`].
    pub fn unknown_subtype(&self) -> Option<String> {
        match self {
            InControlRequestBody::Other { subtype, .. } => match subtype.as_deref() {
                Some(s) if KNOWN_CLI_REQUEST_SUBTYPES.contains(&s) => None,
                s => Some(s.unwrap_or("?").to_string()),
            },
            InControlRequestBody::CanUseTool(_) => None,
        }
    }
}

/// `can_use_tool` (spec 4.5, `t4`, `t8`, `t9`, `t10`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CanUseTool {
    #[serde(deserialize_with = "opt")]
    pub tool_name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub display_name: Option<String>,
    /// Entrée de l'outil, brute (`null` conservé) : renvoyée telle quelle dans
    /// `updatedInput`.
    #[serde(deserialize_with = "raw")]
    pub input: Option<Value>,
    #[serde(deserialize_with = "opt")]
    pub tool_use_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
    #[serde(deserialize_with = "raw")]
    pub permission_suggestions: Option<Value>,
    #[serde(deserialize_with = "opt")]
    pub blocked_path: Option<String>,
    #[serde(deserialize_with = "raw")]
    pub decision_reason: Option<Value>,
    #[serde(deserialize_with = "opt")]
    pub agent_id: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub requires_user_interaction: Option<bool>,
}

/// `{"type":"control_response","response":{…}}` lu sur stdout.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InControlResponse {
    #[serde(deserialize_with = "super::lenient::obj")]
    pub response: Option<InControlResponseBody>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InControlResponseBody {
    /// `success` ou `error`.
    #[serde(deserialize_with = "opt")]
    pub subtype: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub request_id: Option<String>,
    /// Charge utile d'un succès (forme propre à la requête).
    #[serde(deserialize_with = "raw")]
    pub response: Option<Value>,
    #[serde(deserialize_with = "opt")]
    pub error: Option<String>,
    #[serde(deserialize_with = "raw")]
    pub pending_permission_requests: Option<Value>,
}

impl InControlResponseBody {
    /// `Ok(response)` (`Null` si absent) pour `success`, `Err(error)` sinon.
    pub fn result(&self) -> Result<Value, String> {
        match self.subtype.as_deref() {
            Some("success") => Ok(self.response.clone().unwrap_or(Value::Null)),
            _ => Err(self
                .error
                .clone()
                .unwrap_or_else(|| "erreur inconnue".into())),
        }
    }
}

/// Réponse à `initialize` (`t11`, spec 4.7bis).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InitializeResponse {
    #[serde(deserialize_with = "vec_or_default")]
    pub commands: Vec<SlashCommand>,
    #[serde(deserialize_with = "vec_or_default")]
    pub agents: Vec<AgentDefinition>,
    #[serde(deserialize_with = "vec_or_default")]
    pub models: Vec<ModelDefinition>,
    #[serde(deserialize_with = "opt")]
    pub claude_code_version: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub current_permission_mode: Option<String>,
    #[serde(deserialize_with = "strings")]
    pub capabilities: Vec<String>,
    #[serde(deserialize_with = "opt")]
    pub output_style: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub session_state: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub pid: Option<u64>,
}

impl InitializeResponse {
    pub fn from_value(v: &Value) -> InitializeResponse {
        from_value(v)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SlashCommand {
    #[serde(deserialize_with = "opt")]
    pub name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
    /// Chaîne vide quand la commande n'a pas d'argument.
    #[serde(rename = "argumentHint", deserialize_with = "opt")]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentDefinition {
    /// Brut, parfois entouré de U+00A0 (spec 4.8).
    #[serde(deserialize_with = "opt")]
    pub name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelDefinition {
    #[serde(deserialize_with = "opt")]
    pub value: Option<String>,
    #[serde(rename = "resolvedModel", deserialize_with = "opt")]
    pub resolved_model: Option<String>,
    #[serde(rename = "displayName", deserialize_with = "opt")]
    pub display_name: Option<String>,
    #[serde(deserialize_with = "opt")]
    pub description: Option<String>,
}

/// Réponse à `interrupt` (`t6`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InterruptResponse {
    /// Messages encore en file ; un élément non textuel est gardé en JSON.
    #[serde(deserialize_with = "queued")]
    pub still_queued: Vec<String>,
}

fn queued<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a
            .into_iter()
            .map(|x| match x {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// Réponse à `set_model` (`t5`) : modèle résolu.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SetModelResponse {
    #[serde(deserialize_with = "opt")]
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// app -> CLI

/// Corps d'une requête de contrôle émise par l'app.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum OutControlRequest {
    Initialize {},
    Interrupt {
        #[serde(skip_serializing_if = "Option::is_none")]
        cancel_queued: Option<bool>,
    },
    SetModel {
        model: String,
    },
    SetPermissionMode {
        mode: String,
    },
    StopTask {
        task_id: String,
    },
    /// Sous-type quelconque avec ses champs (ordre conservé).
    #[serde(untagged)]
    Raw(Map<String, Value>),
}

impl OutControlRequest {
    /// Requête générique : `{"subtype":…, …extra}`.
    pub fn raw(subtype: &str, extra: Map<String, Value>) -> OutControlRequest {
        let mut m = Map::new();
        m.insert("subtype".into(), Value::String(subtype.into()));
        m.extend(extra);
        OutControlRequest::Raw(m)
    }
}

/// Corps d'une réponse de l'app à une requête du CLI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlResponseBody {
    Success { request_id: String, response: Value },
    Error { request_id: String, error: String },
}

/// Décision sur un `can_use_tool` (`PermissionResult`, sdk.d.ts).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: Value,
        #[serde(rename = "updatedPermissions", skip_serializing_if = "Option::is_none")]
        updated_permissions: Option<Value>,
    },
    Deny {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
    },
}

/// `updatedInput` d'une réponse à `AskUserQuestion` (`t4`, `t10`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AskUserQuestionInput {
    /// Questions recopiées telles que reçues.
    pub questions: Value,
    /// Texte de la question -> label(s) choisi(s) ; `{}` pour « ignorer ».
    pub answers: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// Texte de la question -> `{"unanswered": raison}` (`t10`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Map<String, Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn out_requests_serialize_like_the_driver() {
        let s = |r: &OutControlRequest| serde_json::to_string(r).unwrap();
        assert_eq!(
            s(&OutControlRequest::Initialize {}),
            r#"{"subtype":"initialize"}"#
        );
        assert_eq!(
            s(&OutControlRequest::Interrupt {
                cancel_queued: None
            }),
            r#"{"subtype":"interrupt"}"#
        );
        assert_eq!(
            s(&OutControlRequest::SetModel {
                model: "sonnet".into()
            }),
            r#"{"subtype":"set_model","model":"sonnet"}"#
        );
        let mut extra = Map::new();
        extra.insert("cancel_queued".into(), json!(true));
        assert_eq!(
            s(&OutControlRequest::raw("interrupt", extra)),
            r#"{"subtype":"interrupt","cancel_queued":true}"#
        );
    }

    #[test]
    fn permission_results() {
        let allow = PermissionResult::Allow {
            updated_input: json!({"a": 1}),
            updated_permissions: None,
        };
        assert_eq!(
            serde_json::to_string(&allow).unwrap(),
            r#"{"behavior":"allow","updatedInput":{"a":1}}"#
        );
        let deny = PermissionResult::Deny {
            message: "non".into(),
            interrupt: None,
        };
        assert_eq!(
            serde_json::to_string(&deny).unwrap(),
            r#"{"behavior":"deny","message":"non"}"#
        );
    }

    #[test]
    fn in_requests_and_responses() {
        let r = InControlRequest::from_value(
            &json!({"type":"control_request","request_id":"x","request":{"subtype":"can_use_tool","tool_name":"Bash","input":null}}),
        );
        let InControlRequestBody::CanUseTool(c) = &r.request else {
            panic!()
        };
        assert_eq!(c.input, Some(Value::Null), "input nul conservé");
        let r = InControlRequest::from_value(&json!({"type":"control_request"}));
        assert_eq!(r.request_id, None);
        assert_eq!(r.request, InControlRequestBody::default());
        assert_eq!(r.request.unknown_subtype().as_deref(), Some("?"));

        let ok: InControlResponse =
            from_value(&json!({"response":{"subtype":"success","request_id":"i"}}));
        assert_eq!(ok.response.unwrap().result(), Ok(Value::Null));
        let err: InControlResponse =
            from_value(&json!({"response":{"subtype":"error","error":"non"}}));
        assert_eq!(err.response.unwrap().result(), Err("non".into()));
    }

    #[test]
    fn interrupt_and_set_model_responses() {
        let r: InterruptResponse = from_value(&json!({"still_queued": ["a", 3]}));
        assert_eq!(r.still_queued, vec!["a", "3"]);
        let m: SetModelResponse = from_value(&json!({"model": "claude-sonnet-5-5"}));
        assert_eq!(m.model.as_deref(), Some("claude-sonnet-5-5"));
    }
}
