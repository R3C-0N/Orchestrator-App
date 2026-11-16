//! Lignes JSON écrites sur le stdin de `claude.exe` (spec 4.2 et 4.5).

use serde_json::{json, Map, Value};

use crate::api::types::{Answers, SkipReason};

/// `{"type":"user","message":{"role":"user","content":"…"}}`
pub fn user_message(text: &str) -> String {
    json!({"type":"user","message":{"role":"user","content":text}}).to_string()
}

/// `{"type":"control_request","request_id":…,"request":{"subtype":…, …}}`
pub fn control_request(request_id: &str, subtype: &str, extra: Map<String, Value>) -> String {
    let mut req = Map::new();
    req.insert("subtype".into(), Value::String(subtype.into()));
    req.extend(extra);
    json!({"type":"control_request","request_id":request_id,"request":Value::Object(req)})
        .to_string()
}

/// Réponse de succès à une requête de contrôle du CLI.
pub fn control_success(request_id: &str, response: Value) -> String {
    json!({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":response}})
        .to_string()
}

/// Réponse d'erreur (`ControlErrorResponse`, sdk.d.ts).
pub fn control_error(request_id: &str, error: &str) -> String {
    json!({"type":"control_response","response":{"subtype":"error","request_id":request_id,"error":error}})
        .to_string()
}

/// `allow` avec `updatedInput` = `input` (décision 7 : allow par défaut).
pub fn allow(request_id: &str, input: &Value) -> String {
    control_success(request_id, json!({"behavior":"allow","updatedInput":input}))
}

/// Réponse à AskUserQuestion (`t4`) : questions recopiées + answers.
pub fn answer_question(
    request_id: &str,
    input: &Value,
    answers: &Answers,
    response: Option<&str>,
) -> String {
    let mut updated = Map::new();
    updated.insert(
        "questions".into(),
        input
            .get("questions")
            .cloned()
            .unwrap_or(Value::Array(vec![])),
    );
    updated.insert(
        "answers".into(),
        serde_json::to_value(answers).unwrap_or_default(),
    );
    if let Some(r) = response {
        updated.insert("response".into(), Value::String(r.into()));
    }
    control_success(
        request_id,
        json!({"behavior":"allow","updatedInput":Value::Object(updated)}),
    )
}

/// « Ignorer » (`t10`) : `answers:{}` + `annotations[q].unanswered`.
pub fn skip_question(request_id: &str, input: &Value, reason: SkipReason) -> String {
    let reason = match reason {
        SkipReason::Skipped => "skipped",
        SkipReason::Dismissed => "dismissed",
    };
    let questions = input
        .get("questions")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    let mut annotations = Map::new();
    if let Value::Array(qs) = &questions {
        for q in qs {
            if let Some(text) = q.get("question").and_then(Value::as_str) {
                annotations.insert(text.into(), json!({"unanswered":reason}));
            }
        }
    }
    control_success(
        request_id,
        json!({"behavior":"allow","updatedInput":{"questions":questions,"answers":{},"annotations":annotations}}),
    )
}
