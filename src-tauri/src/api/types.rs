//! Types du contrat cœur Rust <-> interface (section 1.5 du plan).
//!
//! Tout type exporté ici est projeté en TypeScript par ts-rs dans
//! `src/lib/core/bindings/` (`cargo test export_bindings`). Les entiers 64 bits
//! sont annotés `number` : serde les sérialise en nombre JSON, pas en `bigint`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Identifiant local d'une session dans l'app (pas le `session_id` du CLI).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionKey(pub String);

/// Réponse de `start_session`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionStarted {
    pub session: SessionKey,
    pub pid: u32,
    pub claude_path: String,
}

/// Réponse de `interrupt`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InterruptAck {
    pub still_queued: Vec<String>,
}

/// Réponse à une question d'AskUserQuestion : un label, ou plusieurs (multi-sélection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Answer {
    One(String),
    Many(Vec<String>),
}

/// Raison d'une question ignorée (`annotations[q].unanswered`, `t10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkipReason {
    Skipped,
    Dismissed,
}

/// Entrée de `list_sessions` (lecture des transcripts, section 1.8).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    #[ts(type = "number")]
    pub modified: u64,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub entrypoint: Option<String>,
}

/// Réponse de `stop_session`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExitInfo {
    pub code: Option<i32>,
    /// Vrai si le processus a dû être tué (Job Object) faute de sortie dans le délai.
    pub killed: bool,
}

/// Réponse de `get_diagnostics`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Diagnostics {
    pub claude_version: Option<String>,
    pub expected_version: String,
    pub claude_path: Option<String>,
    pub capabilities: Vec<String>,
    pub log_path: Option<String>,
    pub stderr_tail: Vec<String>,
    /// Types et sous-types inconnus ignorés par le réducteur (plan 1.9).
    #[ts(type = "number")]
    pub unknown_count: u64,
}

/// Réponse de `get_launch_info` (ajout au plan : l'interface doit connaître le
/// dossier de projet passé en argument, `<app> .`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchInfo {
    /// Dossier de projet : premier argument de l'app, sinon dossier courant.
    pub cwd: String,
    pub claude_path: Option<String>,
    pub expected_version: String,
}

/// Erreurs des commandes, sérialisées `{ kind, message? }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
#[ts(export)]
pub enum CoreError {
    #[error("claude.exe introuvable : {0}")]
    ClaudeNotFound(String),
    #[error("échec du démarrage : {0}")]
    SpawnFailed(String),
    #[error("dossier de travail invalide : {0}")]
    BadCwd(String),
    #[error("argument invalide : {0}")]
    InvalidArgument(String),
    #[error("aucune session en cours")]
    NotRunning,
    #[error("une question attend une réponse")]
    QuestionPending,
    #[error("délai dépassé : {0}")]
    Timeout(String),
    #[error("refusé par le CLI : {0}")]
    Rejected(String),
    #[error("requête inconnue : {0}")]
    UnknownRequest(String),
    #[error("non pris en charge : {0}")]
    Unsupported(String),
    #[error("entrée/sortie : {0}")]
    Io(String),
    #[error("transcript illisible : {0}")]
    TranscriptFormat(String),
}

/// Alias pratique pour les réponses aux questions.
pub type Answers = BTreeMap<String, Answer>;
