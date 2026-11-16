//! Lignes JSON écrites sur le stdin de `claude.exe` (spec 4.2 et 4.5).
//!
//! Chaque fonction rend une ligne sans `\n` final (l'écrivain l'ajoute). Les
//! sorties sont comparées à l'octet près aux lignes `>>> ` des fixtures `t4`,
//! `t5`, `t6`, `t9`, `t10`, `t11` (`tests/fixtures_replay.rs`).

use serde::Serialize;
use serde_json::{Map, Value};

use super::control::{
    AskUserQuestionInput, ControlResponseBody, OutControlRequest, PermissionResult,
};
use crate::api::types::{Answers, SkipReason};

/// Message écrit sur stdin.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outbound {
    /// `{"type":"user","message":{"role":"user","content":"…"}}` (spec 4.2).
    User {
        message: OutUserMessage,
    },
    ControlRequest {
        request_id: String,
        request: OutControlRequest,
    },
    ControlResponse {
        response: ControlResponseBody,
    },
    /// Retire l'une de nos requêtes en attente (`SDKControlCancelRequest`).
    ControlCancelRequest {
        request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutUserMessage {
    pub role: &'static str,
    pub content: String,
}

impl Outbound {
    /// Ligne JSON, sans `\n`.
    pub fn to_line(&self) -> String {
        // Sérialiser des chaînes, des nombres et des `Value` ne peut pas échouer.
        serde_json::to_string(self).expect("sérialisation d'un message sortant")
    }
}

/// `{"type":"user","message":{"role":"user","content":"…"}}`
pub fn user_message(text: &str) -> String {
    Outbound::User {
        message: OutUserMessage {
            role: "user",
            content: text.to_string(),
        },
    }
    .to_line()
}

/// `{"type":"control_request","request_id":…,"request":{"subtype":…, …}}`
pub fn control_request(request_id: &str, subtype: &str, extra: Map<String, Value>) -> String {
    control_request_typed(request_id, OutControlRequest::raw(subtype, extra))
}

/// Requête de contrôle typée.
pub fn control_request_typed(request_id: &str, request: OutControlRequest) -> String {
    Outbound::ControlRequest {
        request_id: request_id.to_string(),
        request,
    }
    .to_line()
}

/// Réponse de succès à une requête de contrôle du CLI.
pub fn control_success(request_id: &str, response: Value) -> String {
    Outbound::ControlResponse {
        response: ControlResponseBody::Success {
            request_id: request_id.to_string(),
            response,
        },
    }
    .to_line()
}

/// Réponse d'erreur (`ControlErrorResponse`, sdk.d.ts).
pub fn control_error(request_id: &str, error: &str) -> String {
    Outbound::ControlResponse {
        response: ControlResponseBody::Error {
            request_id: request_id.to_string(),
            error: error.to_string(),
        },
    }
    .to_line()
}

/// Retire une requête que nous avons émise.
pub fn control_cancel(request_id: &str) -> String {
    Outbound::ControlCancelRequest {
        request_id: request_id.to_string(),
    }
    .to_line()
}

fn permission(request_id: &str, result: PermissionResult) -> String {
    control_success(
        request_id,
        serde_json::to_value(result).expect("sérialisation de PermissionResult"),
    )
}

/// `allow` avec `updatedInput` = `input` (décision 7 : allow par défaut).
pub fn allow(request_id: &str, input: &Value) -> String {
    permission(
        request_id,
        PermissionResult::Allow {
            updated_input: input.clone(),
            updated_permissions: None,
        },
    )
}

/// `deny` avec un message (`t9`).
pub fn deny(request_id: &str, message: &str) -> String {
    permission(
        request_id,
        PermissionResult::Deny {
            message: message.to_string(),
            interrupt: None,
        },
    )
}

fn questions_of(input: &Value) -> Value {
    input
        .get("questions")
        .cloned()
        .unwrap_or(Value::Array(vec![]))
}

fn allow_question(request_id: &str, updated: AskUserQuestionInput) -> String {
    permission(
        request_id,
        PermissionResult::Allow {
            updated_input: serde_json::to_value(updated).expect("sérialisation de updatedInput"),
            updated_permissions: None,
        },
    )
}

/// Réponse à AskUserQuestion (`t4`) : questions recopiées + answers.
pub fn answer_question(
    request_id: &str,
    input: &Value,
    answers: &Answers,
    response: Option<&str>,
) -> String {
    allow_question(
        request_id,
        AskUserQuestionInput {
            questions: questions_of(input),
            answers: serde_json::to_value(answers).unwrap_or_default(),
            response: response.map(str::to_string),
            annotations: None,
        },
    )
}

/// « Ignorer » (`t10`) : `answers:{}` + `annotations[q].unanswered`.
pub fn skip_question(request_id: &str, input: &Value, reason: SkipReason) -> String {
    let reason = match reason {
        SkipReason::Skipped => "skipped",
        SkipReason::Dismissed => "dismissed",
    };
    let questions = questions_of(input);
    let mut annotations = Map::new();
    if let Value::Array(qs) = &questions {
        for q in qs {
            if let Some(text) = q.get("question").and_then(Value::as_str) {
                let mut a = Map::new();
                a.insert("unanswered".into(), Value::String(reason.into()));
                annotations.insert(text.into(), Value::Object(a));
            }
        }
    }
    allow_question(
        request_id,
        AskUserQuestionInput {
            questions,
            answers: Value::Object(Map::new()),
            response: None,
            annotations: Some(annotations),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_raw_newline_in_lines() {
        let l = user_message("a\nb\r\n\u{2028}é");
        assert!(!l.contains('\n') && !l.contains('\r'));
        assert_eq!(
            serde_json::from_str::<Value>(&l).unwrap()["message"]["content"],
            "a\nb\r\n\u{2028}é"
        );
    }

    #[test]
    fn cancel_and_error_lines() {
        assert_eq!(
            control_cancel("r1"),
            r#"{"type":"control_cancel_request","request_id":"r1"}"#
        );
        assert_eq!(
            control_error("r2", "non"),
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"r2","error":"non"}}"#
        );
        assert_eq!(
            control_request_typed(
                "s",
                OutControlRequest::StopTask {
                    task_id: "t".into()
                }
            ),
            r#"{"type":"control_request","request_id":"s","request":{"subtype":"stop_task","task_id":"t"}}"#
        );
    }
}
