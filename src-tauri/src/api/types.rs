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
    /// J11 (décision 11) : vrai si `cwd` vient d'un argument valide
    /// (`<app> .`, `<app> <chemin>`) ; faux sans argument, l'interface ouvre
    /// alors le sélecteur des projets récents (`list_projects`).
    pub cwd_from_args: bool,
    /// J11 : argument donné mais inutilisable (dossier absent…) ; `cwd` est
    /// alors le dossier courant et `cwd_from_args` vaut `false`.
    pub launch_error: Option<String>,
    pub claude_path: Option<String>,
    pub expected_version: String,
}

/// Entrée de `list_projects` (J11, plan 1.8, décision 11) : un sous-dossier
/// de `~/.claude/projects` qui contient au moins un transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSummary {
    /// Nom du dossier (cwd encodé, non réversible).
    pub encoded: String,
    /// Vrai chemin, lu dans le champ `cwd` du transcript le plus récent ;
    /// `None` si aucun transcript n'en contient.
    pub path: Option<String>,
    /// `path` existe encore (sinon : grisé dans le sélecteur). Vrai sans
    /// vérification pour un chemin réseau (voir `exists_checked`).
    pub exists: bool,
    /// Faux si l'existence n'a pas été testée : chemin réseau
    /// (`\\serveur\…`), dont le test peut bloquer longtemps. Le sélecteur
    /// l'affiche « non vérifié » et le laisse choisir.
    pub exists_checked: bool,
    /// Date (ms depuis l'epoch) du transcript le plus récent.
    #[ts(type = "number")]
    pub last_used: u64,
    pub session_count: u32,
}

/// Réponse de `load_history` (J11, décision 9) : l'historique relu dans le
/// transcript, en `CoreEvent` déjà réduits (même réducteur que le flux,
/// messages utilisateur d'origine `History`), numérotés à partir de 0 et
/// horodatés avec le `timestamp` des lignes. Aucun `status` n'y figure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HistoryLoaded {
    pub session_id: String,
    pub events: Vec<super::events::CoreEvent>,
    /// Lignes non vides du transcript.
    pub lines: u32,
    /// Lignes passées au réducteur (`user`, `assistant`, `system` utiles).
    pub used: u32,
    /// Lignes ignorées : types sans affichage, messages internes.
    pub ignored: u32,
    /// Parmi `ignored`, lignes d'un type inconnu du lecteur.
    pub unknown: u32,
    /// Éléments inconnus vus par le réducteur (blocs, sous-types).
    pub unknown_elements: u32,
    /// Lignes illisibles (JSON invalide, trop longues) : notice
    /// `history_damaged` en fin d'`events`.
    pub bad: u32,
    /// Lignes `isSidechain: true` du fichier principal (ignorées).
    pub sidechain: u32,
    /// La dernière ligne est inachevée (transcript tronqué ou en cours
    /// d'écriture) ; elle est ignorée sans avertissement.
    pub truncated: bool,
    /// Fils de sous-agents `subagents/agent-*.jsonl` présents (non relus).
    pub subagent_threads: u32,
    /// Début du transcript non relu (seuls les 20 derniers Mo le sont) :
    /// lignes et octets omis. Notice Info `history_truncated` en tête
    /// d'`events` quand ils sont non nuls.
    pub omitted_lines: u32,
    #[ts(type = "number")]
    pub omitted_bytes: u64,
    /// Chaînes raccourcies à 64 ko (« … [tronqué] ») : contenus de
    /// `tool_result`, `meta` (`toolUseResult`), entrées d'outils, textes.
    pub clipped: u32,
    /// Événements les plus anciens retirés pour garder la réponse sous son
    /// plafond (coupure au début d'un message utilisateur).
    pub dropped_events: u32,
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
