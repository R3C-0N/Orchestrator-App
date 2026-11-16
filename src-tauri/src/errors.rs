//! Erreurs de la supervision (plan 1.9) : codes des `notice` émises par le
//! superviseur et limitation de leur fréquence.
//!
//! Les erreurs des commandes (`CoreError`) font partie du contrat et restent
//! dans `api/types.rs`. Ici : ce que le superviseur signale à l'interface sans
//! faire échouer de commande.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::api::events::{EventBody, NoticeLevel};

/// Ligne de stdout illisible (JSON invalide).
pub const BAD_JSON: &str = "bad_json";
/// Ligne de stdout au-delà du plafond de taille (`reader::MAX_LINE_BYTES`).
pub const LINE_TOO_LONG: &str = "line_too_long";
/// Le processus s'est arrêté avec un code non nul sans que l'app l'ait demandé.
pub const EXIT_NONZERO: &str = "exit_nonzero";
/// Le tuyau stdin s'est cassé alors que le processus tournait encore.
pub const STDIN_BROKEN: &str = "stdin_broken";
/// `interrupt` accepté par le CLI, mais aucun `result` dans le délai.
pub const INTERRUPT_NO_RESULT: &str = "interrupt_no_result";

/// Intervalle minimal entre deux notices d'un même code limité (plan 1.9 :
/// « 1 par minute au plus »).
pub const NOTICE_INTERVAL: Duration = Duration::from_secs(60);

/// Limite la fréquence des notices, code par code.
#[derive(Debug, Default)]
pub struct NoticeLimiter {
    last: HashMap<&'static str, Instant>,
    /// Notices supprimées depuis la dernière émise, par code.
    suppressed: HashMap<&'static str, u64>,
}

impl NoticeLimiter {
    /// Vrai si une notice `code` peut partir à l'instant `now` (et l'enregistre).
    /// Sinon, la compte comme supprimée.
    pub fn allow(&mut self, code: &'static str, now: Instant) -> bool {
        match self.last.get(code) {
            Some(&l) if now.saturating_duration_since(l) < NOTICE_INTERVAL => {
                *self.suppressed.entry(code).or_default() += 1;
                false
            }
            _ => {
                self.last.insert(code, now);
                true
            }
        }
    }

    /// Nombre de notices `code` supprimées depuis la dernière émise, remis à 0.
    pub fn take_suppressed(&mut self, code: &'static str) -> u64 {
        self.suppressed.remove(code).unwrap_or(0)
    }
}

/// Début d'une ligne pour le détail d'une notice (500 caractères au plus).
pub fn head(line: &str) -> String {
    line.chars().take(500).collect()
}

pub fn bad_json(error: &str, line: &str, suppressed: u64) -> EventBody {
    EventBody::Notice {
        level: NoticeLevel::Warn,
        code: BAD_JSON.into(),
        message: format!("Ligne illisible ignorée : {error}"),
        detail: Some(json!({ "line": head(line), "suppressed": suppressed })),
    }
}

pub fn line_too_long(len: usize, max: usize, head_text: &str, suppressed: u64) -> EventBody {
    EventBody::Notice {
        level: NoticeLevel::Warn,
        code: LINE_TOO_LONG.into(),
        message: format!("Ligne de {len} octets ignorée (plafond : {max} octets)"),
        detail: Some(json!({
            "len": len,
            "max": max,
            "head": head(head_text),
            "suppressed": suppressed,
        })),
    }
}

pub fn exit_nonzero(code: Option<i32>, stderr_tail: &[String]) -> EventBody {
    let shown = code.map_or_else(|| "inconnu".to_string(), |c| c.to_string());
    EventBody::Notice {
        level: NoticeLevel::Error,
        code: EXIT_NONZERO.into(),
        message: format!("claude.exe s'est arrêté (code {shown})"),
        detail: Some(json!({ "code": code, "stderr_tail": stderr_tail })),
    }
}

pub fn stdin_broken(error: &str) -> EventBody {
    EventBody::Notice {
        level: NoticeLevel::Warn,
        code: STDIN_BROKEN.into(),
        message: format!("Écriture impossible vers claude.exe : {error}"),
        detail: None,
    }
}

pub fn interrupt_no_result(waited: Duration) -> EventBody {
    EventBody::Notice {
        level: NoticeLevel::Warn,
        code: INTERRUPT_NO_RESULT.into(),
        message: format!(
            "Interruption acceptée mais aucun résultat après {} s ; le tour reprend son état.",
            waited.as_secs()
        ),
        detail: None::<Value>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_is_per_code_and_per_minute() {
        let mut l = NoticeLimiter::default();
        let t0 = Instant::now();
        assert!(l.allow(BAD_JSON, t0));
        assert!(!l.allow(BAD_JSON, t0 + Duration::from_secs(10)));
        assert!(!l.allow(BAD_JSON, t0 + Duration::from_secs(59)));
        // Un autre code a sa propre limite.
        assert!(l.allow(LINE_TOO_LONG, t0 + Duration::from_secs(10)));
        assert_eq!(l.take_suppressed(BAD_JSON), 2);
        assert_eq!(l.take_suppressed(BAD_JSON), 0);
        assert!(l.allow(BAD_JSON, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn head_cuts_on_char_boundary() {
        let s = "é".repeat(600);
        assert_eq!(head(&s).chars().count(), 500);
    }
}
