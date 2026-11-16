//! Requêtes en attente (plan 1.6 : `pending_in`).
//!
//! - `can_use_tool` `AskUserQuestion` en attente d'une réponse de l'interface :
//!   table `request_id -> input` (champ `Reducer::questions`, lu par le
//!   superviseur pour construire la réponse).
//! - `initialize` envoyés par nous, dont la réponse produit `ready`.
//!
//! Les requêtes que **nous** envoyons (`pending_out` du plan : `interrupt`,
//! `set_model`…) attendent leur réponse dans le superviseur, qui reçoit
//! l'action `Resolve` du réducteur.

use serde_json::Value;

use crate::api::events::{QuestionCloseReason, QuestionItem, QuestionOption};

/// Capacité qui permet d'« ignorer » une question (`t10`).
pub const CAP_UNANSWERED: &str = "ask_user_question_unanswered_v1";

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// Questions d'un `input` d'`AskUserQuestion` (`t4`).
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

/// Raison de fermeture d'une question d'après la réponse que nous envoyons
/// (`response` du `control_response`, `t4` et `t10`) : `Skipped` quand aucune
/// réponse n'est donnée et qu'au moins une question est annotée
/// `unanswered` (`skip_question`, raisons `skipped`, `replied_in_chat` et
/// `dismissed`) ;
/// `Answered` sinon.
pub fn close_reason_of_response(response: &Value) -> QuestionCloseReason {
    let updated = response.get("updatedInput");
    let no_answer = updated
        .and_then(|u| u.get("answers"))
        .and_then(Value::as_object)
        .is_none_or(|a| a.is_empty());
    let unanswered = updated
        .and_then(|u| u.get("annotations"))
        .and_then(Value::as_object)
        .is_some_and(|a| a.values().any(|x| x.get("unanswered").is_some()));
    if no_answer && unanswered {
        QuestionCloseReason::Skipped
    } else {
        QuestionCloseReason::Answered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Answer, Answers, SkipReason};
    use crate::protocol::outbound;
    use serde_json::json;

    fn response_of(line: &str) -> Value {
        let v: Value = serde_json::from_str(line).unwrap();
        v["response"]["response"].clone()
    }

    #[test]
    fn skip_and_answer_are_told_apart() {
        let input =
            json!({"questions":[{"question":"Q ?","header":"H","options":[],"multiSelect":false}]});
        for reason in [
            SkipReason::Skipped,
            SkipReason::ReplyInChat,
            SkipReason::Dismissed,
        ] {
            let line = outbound::skip_question("r1", &input, reason);
            assert_eq!(
                close_reason_of_response(&response_of(&line)),
                QuestionCloseReason::Skipped
            );
        }
        let mut answers = Answers::new();
        answers.insert("Q ?".into(), Answer::One("Bleu".into()));
        let line = outbound::answer_question("r1", &input, &answers, None);
        assert_eq!(
            close_reason_of_response(&response_of(&line)),
            QuestionCloseReason::Answered
        );
    }

    /// Les trois raisons du protocole (`sdk.d.ts`, capacité
    /// `ask_user_question_unanswered_v1`), à l'octet près dans
    /// `annotations[q].unanswered`, et la valeur reçue de l'interface.
    #[test]
    fn skip_reasons_on_the_wire() {
        let input =
            json!({"questions":[{"question":"Q ?","header":"H","options":[],"multiSelect":true}]});
        for (reason, wire) in [
            (SkipReason::Skipped, "skipped"),
            (SkipReason::ReplyInChat, "replied_in_chat"),
            (SkipReason::Dismissed, "dismissed"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), json!(wire));
            assert_eq!(
                serde_json::from_value::<SkipReason>(json!(wire)).unwrap(),
                reason
            );
            let r = response_of(&outbound::skip_question("r1", &input, reason));
            assert_eq!(r["behavior"], "allow");
            assert_eq!(r["updatedInput"]["answers"], json!({}));
            assert_eq!(
                r["updatedInput"]["annotations"],
                json!({"Q ?": {"unanswered": wire}})
            );
        }
        assert!(serde_json::from_value::<SkipReason>(json!("reply_in_chat")).is_err());
    }
}
