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
/// Les trois valeurs admises par la capacité `ask_user_question_unanswered_v1`
/// (`sdk.d.ts`, `capabilities`) ; un hôte strict refuse toute autre valeur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkipReason {
    Skipped,
    /// « Chat about this » : l'utilisateur répond dans la conversation.
    /// Valeur du protocole au participe (`replied_in_chat`), d'où le `rename`.
    #[serde(rename = "replied_in_chat")]
    ReplyInChat,
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
    /// J14, étape 0 : commande du contrat pas encore implémentée (le message
    /// est le nom de la commande).
    #[error("pas encore implémenté : {0}")]
    NotImplemented(String),
}

/// Alias pratique pour les réponses aux questions.
pub type Answers = BTreeMap<String, Answer>;

// --- J14 : auto-amélioration (spec, décisions 15 à 19) ------------------------

/// Rôle d'une session dans l'app : la session de l'utilisateur, ou la session
/// `claude.exe` dédiée à une amélioration (`improve_start`), lancée par le
/// cœur dans un worktree du clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionRole {
    Main,
    Improve,
}

/// Entrée de `attach_sessions` : tout ce qu'il faut à une interface
/// (re)chargée pour reconstruire une session vivante à l'identique. `events`
/// est le journal complet des `CoreEvent` émis depuis le démarrage (`seq`
/// continus à partir de 0) ; `history` est l'historique repris tel que l'a
/// renvoyé le premier `load_history`, à appliquer avant `events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionResync {
    pub session: SessionKey,
    pub role: SessionRole,
    pub cwd: String,
    pub pid: u32,
    /// `session_id` du CLI passé à `--resume`, s'il y en a un.
    pub resumed_from: Option<String>,
    pub history: Option<HistoryLoaded>,
    pub events: Vec<super::events::CoreEvent>,
    /// Faux si le processus est déjà sorti (son `exited` est dans `events`).
    pub running: bool,
}

/// D'où vient l'interface affichée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UiSource {
    /// Interface embarquée dans l'exe (repli, ou aucune version sur disque).
    Embedded,
    /// `%LOCALAPPDATA%\orchestrator\ui\<hash>\`.
    Disk,
}

/// État du clone du dépôt source de l'app (décision 16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum RepoStatus {
    /// Pas encore examiné (démarrage).
    Unknown,
    /// Clonage en cours.
    Cloning { path: String },
    /// Clone utilisable : `/ameliorer` est permis.
    Ready {
        path: String,
        url: String,
        /// Tête de `main` dans le clone.
        head: String,
    },
    /// `/ameliorer` désactivé (git, Node, npm ou clé SSH absents, pas de
    /// réseau, dossier non inscriptible…) ; `reason` est affichée.
    Disabled { reason: String },
}

/// Réponse de `get_ui_info` et de `ui_rollback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UiInfo {
    pub source: UiSource,
    /// Hash du commit de l'interface affichée (`None` : interface embarquée).
    pub active_hash: Option<String>,
    /// Version à laquelle `ui_rollback` reviendrait.
    pub previous_hash: Option<String>,
    /// Commit à partir duquel l'exe a été compilé (`unknown` si inconnu).
    pub build_commit: String,
    /// Empreinte du contrat de l'exe (liaisons TS, capability, `build.rs`) :
    /// une interface d'empreinte différente n'est jamais activée.
    pub contract: String,
    /// Commit fusionné qui attend une relance (cœur Rust modifié, décision 18).
    pub restart_required: Option<String>,
    pub repo: RepoStatus,
    /// Amélioration en cours, ou la dernière terminée.
    pub improve: Option<ImproveStatus>,
    /// Envoi des branches `auto/…` sur le dépôt distant (propriétaire de
    /// l'app seulement, décision 16) ; `off` pour tous les autres.
    pub push: PushStatus,
}

/// Envoi des améliorations fusionnées sur le dépôt distant (décision 16) :
/// réservé au propriétaire (réglage `push` de `config.json`), et seulement
/// si GitHub lui accorde le droit d'écriture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum PushStatus {
    /// Réglage `push` absent ou faux (cas de tous les utilisateurs sauf le
    /// propriétaire) : rien n'est envoyé, rien n'est affiché.
    Off,
    /// Réglage présent : droit d'écriture en cours de vérification.
    Checking,
    /// Droit d'écriture vérifié : la branche `auto/…` d'une amélioration
    /// fusionnée est envoyée sur `url`.
    Enabled { url: String },
    /// Réglage présent mais envoi désactivé (pas de droit d'écriture,
    /// clone indisponible…) ; `reason` est lisible.
    Disabled { reason: String },
}

/// Envoi de la branche d'une amélioration fusionnée (`ImproveStatus::push`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum PushResult {
    /// Envoi en cours (après la fusion locale).
    Pushing { branch: String },
    /// Branche `auto/…` envoyée sous le même nom.
    Pushed { branch: String },
    /// Fusionnée localement, envoi échoué : la fusion reste acquise et
    /// l'envoi est retenté au cycle suivant et au prochain démarrage.
    Failed { branch: String, reason: String },
}

/// Étape d'une amélioration (décision 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ImproveStep {
    /// Création du worktree et de la branche.
    Preparing,
    /// Session `claude.exe` dédiée au travail (tour demandé par l'utilisateur).
    Working,
    /// Tour de correction automatique : un contrôle ou un test a échoué et
    /// l'erreur a été renvoyée à la session (`ImproveStatus::attempt`).
    Fixing,
    /// En attente de l'utilisateur (message ou « Valider ») ; raison dans
    /// `ImproveStatus::waiting`.
    AwaitingUser,
    /// Contrôles : arbre propre, commits sur la branche, fichiers permis.
    Checking,
    Testing,
    Building,
    /// Interface prête : attente que la session principale soit au repos.
    WaitingIdle,
    /// Rechargement et attente du signal « prêt » (5 s).
    Switching,
    /// Fusion fast-forward faite (fin).
    Merged,
    /// Fusionnée, cœur Rust modifié : relance requise (fin).
    RestartRequired,
    /// Échec (fin) : version précédente gardée, branche non fusionnée.
    Failed,
    /// Annulée par `improve_cancel` (fin).
    Cancelled,
    /// Retour arrière par `ui_rollback` (fin).
    RolledBack,
}

/// Avancement d'une amélioration : émis en événement global Tauri
/// [`IMPROVE_EVENT`] (il survit au rechargement de l'interface) et repris
/// dans `UiInfo::improve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImproveStatus {
    pub id: String,
    /// `auto/<AAAAMMJJ-HHMM>-<sujet>`.
    pub branch: String,
    /// Demande de l'utilisateur (texte après `/ameliorer`).
    pub request: String,
    pub step: ImproveStep,
    /// Session dédiée (rôle `Improve`), dès qu'elle existe.
    pub session: Option<SessionKey>,
    /// Tête de la branche, dès qu'un commit existe.
    pub commit: Option<String>,
    /// Précision lisible (test en échec, raison d'un échec…).
    pub detail: Option<String>,
    /// Envoi de la branche après la fusion (`None` : pas d'envoi, cas de
    /// tous les utilisateurs sauf le propriétaire).
    pub push: Option<PushResult>,
    /// Heure de la dernière transition (ms depuis l'epoch).
    #[ts(type = "number")]
    pub t: u64,
    /// Tours de correction automatique faits depuis le dernier message de
    /// l'utilisateur (0 à `max_attempts`).
    pub attempt: u32,
    /// Plafond des tours de correction automatique
    /// (`selfupdate::MAX_AUTO_ATTEMPTS`).
    pub max_attempts: u32,
    /// Tours de session terminés depuis le début du cycle (0 avant le
    /// premier).
    pub turn: u32,
    /// Raison de l'attente quand `step` vaut `AwaitingUser`, sinon `None`.
    pub waiting: Option<ImproveWaiting>,
}

/// Pourquoi un cycle attend l'utilisateur (`ImproveStep::AwaitingUser`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ImproveWaiting {
    /// Tour terminé sans nouveau commit (question posée en texte, demande
    /// impossible…) : rien à tester.
    NoNewCommit,
    /// Tour interrompu par l'utilisateur (Échap).
    Interrupted,
    /// Les tours de correction automatique sont épuisés.
    AttemptsExhausted,
    /// La session s'est arrêtée avant la fin de son tour, ou a dépassé son
    /// délai.
    SessionStopped,
}

/// Nom de l'événement global Tauri qui porte un [`ImproveStatus`].
pub const IMPROVE_EVENT: &str = "improve_status";
