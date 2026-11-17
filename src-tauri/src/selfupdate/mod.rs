//! Auto-amélioration (J14, spec décisions 16 à 19) : clone du dépôt source
//! de l'app, cycle d'une amélioration (worktree `auto/…`, session dédiée,
//! contrôles, tests, build, bascule, fusion fast-forward) et retour arrière.
//!
//! Étape 0 : squelette, signatures figées ; rien n'est cloné ni lancé, les
//! opérations renvoient `CoreError::NotImplemented` et le clone est
//! `Disabled`. Propriétaire : agent C (`src-tauri/src/selfupdate/**`,
//! `tools/self-improve/**`). Le pipeline ne connaît ni Tauri ni `AppState` :
//! la session dédiée et la bascule d'interface passent par [`PipelineHost`],
//! implémenté à l'étape 5 (intégration).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::api::types::{CoreError, ImproveStatus, ImproveStep, RepoStatus};

/// Dépôt distant cloné au premier lancement (remplaçable dans `config.json`).
pub const DEFAULT_ORIGIN: &str = "git@github.com:R3C-0N/Orchestrator-App.git";
/// Préfixe des branches d'amélioration.
pub const BRANCH_PREFIX: &str = "auto/";
/// Dossiers où une amélioration a le droit d'écrire (décision 17).
pub const ALLOWED_DIRS: &[&str] = &["src/", "src-tauri/", "e2e/", "docs/", "tools/"];

/// `%LOCALAPPDATA%\orchestrator`.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("orchestrator"))
}

/// `%LOCALAPPDATA%\orchestrator\config.json`.
pub fn config_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("config.json"))
}

/// Où et quoi cloner (décision 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfig {
    /// URL du dépôt distant (`DEFAULT_ORIGIN`, ou `repo_url` de `config.json`).
    pub url: String,
    /// Dossier du clone : dans le dossier d'installation s'il est
    /// inscriptible, sinon `%LOCALAPPDATA%\orchestrator\repo`.
    pub dir: PathBuf,
    /// Commit sur lequel placer le clone (`build_info::BUILD_COMMIT`).
    pub commit: String,
}

impl RepoConfig {
    /// Lit `config.json` (s'il existe) et complète par les valeurs par
    /// défaut. `install_dir` : dossier de l'exe. Étape 0 : valeurs par
    /// défaut seulement, dossier dans `%LOCALAPPDATA%`.
    pub fn load(install_dir: Option<&Path>) -> RepoConfig {
        let _ = install_dir;
        RepoConfig {
            url: DEFAULT_ORIGIN.into(),
            dir: data_dir().unwrap_or_default().join("repo"),
            commit: crate::build_info::BUILD_COMMIT.into(),
        }
    }
}

/// Clone le dépôt si besoin et le place sur `config.commit` ; renvoie l'état
/// à afficher (`Disabled { reason }` si git, Node, npm, cargo, le réseau ou
/// la clé SSH manquent). Ne panique jamais. Étape 0 : `Disabled`.
pub async fn ensure_clone(config: &RepoConfig) -> RepoStatus {
    let _ = config;
    RepoStatus::Disabled {
        reason: "auto-amélioration pas encore disponible (J14 en cours)".into(),
    }
}

/// Demande d'amélioration (`/ameliorer <texte>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImproveRequest {
    pub id: String,
    pub text: String,
    /// `auto/<AAAAMMJJ-HHMM>-<sujet>`.
    pub branch: String,
}

/// Fin d'un cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImproveOutcome {
    /// Interface activée puis fusionnée dans `main` du clone.
    Merged {
        commit: String,
    },
    /// Fusionnée, cœur Rust modifié : pas d'activation (décision 18).
    RestartRequired {
        commit: String,
    },
    /// Échec : version précédente gardée, branche non fusionnée.
    Failed {
        step: ImproveStep,
        reason: String,
    },
    Cancelled,
}

/// Destination des transitions d'un cycle (événement global
/// `IMPROVE_EVENT` dans l'app, vecteur dans les tests).
pub type StatusSink = Arc<dyn Fn(ImproveStatus) + Send + Sync>;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Ce que le pipeline demande au reste de l'app (implémenté à l'étape 5,
/// simulé dans les tests de l'agent C).
pub trait PipelineHost: Send + Sync {
    /// Lance la session `claude.exe` dédiée (rôle `Improve`) dans `cwd` avec
    /// `prompt`, et attend la fin de son travail (fin du tour, ou sortie).
    fn run_session<'a>(
        &'a self,
        request: &'a ImproveRequest,
        cwd: &'a Path,
        prompt: &'a str,
    ) -> BoxFut<'a, Result<(), CoreError>>;

    /// Installe le build `dir` comme version `hash`, attend que la session
    /// principale soit au repos, bascule et attend le signal « prêt ».
    /// `Ok(true)` : prêt dans le délai ; `Ok(false)` : retour arrière fait.
    fn switch_ui<'a>(&'a self, hash: &'a str, dir: &'a Path)
        -> BoxFut<'a, Result<bool, CoreError>>;
}

/// Cycle d'amélioration sur un clone.
pub struct Pipeline {
    pub repo: RepoConfig,
    pub host: Arc<dyn PipelineHost>,
}

impl Pipeline {
    pub fn new(repo: RepoConfig, host: Arc<dyn PipelineHost>) -> Self {
        Pipeline { repo, host }
    }

    /// Construit la demande (identifiant, nom de branche) à partir du texte.
    pub fn request(&self, text: &str) -> Result<ImproveRequest, CoreError> {
        let _ = text;
        Err(CoreError::NotImplemented("Pipeline::request".into()))
    }

    /// Déroule un cycle complet ; chaque transition passe par `sink`.
    pub async fn run(
        &self,
        request: ImproveRequest,
        sink: StatusSink,
    ) -> Result<ImproveOutcome, CoreError> {
        let _ = (request, sink);
        Err(CoreError::NotImplemented("improve_start".into()))
    }

    /// Annule le cycle `id` en cours (session dédiée arrêtée, branche
    /// laissée non fusionnée).
    pub async fn cancel(&self, id: &str) -> Result<(), CoreError> {
        let _ = id;
        Err(CoreError::NotImplemented("improve_cancel".into()))
    }

    /// `git revert` du commit `commit` par le même cycle (worktree
    /// `auto/…-revert-<hash>`, tests, build, fast-forward).
    pub async fn revert(
        &self,
        commit: &str,
        sink: StatusSink,
    ) -> Result<ImproveOutcome, CoreError> {
        let _ = (commit, sink);
        Err(CoreError::NotImplemented("revert".into()))
    }
}

/// État partagé de l'auto-amélioration, géré par Tauri (`app.manage`).
pub struct SelfUpdate {
    pub repo: Mutex<RepoStatus>,
    /// Cycle en cours, ou le dernier terminé.
    pub current: Mutex<Option<ImproveStatus>>,
    /// Commit fusionné qui attend une relance (décision 18).
    pub restart_required: Mutex<Option<String>>,
}

impl Default for SelfUpdate {
    fn default() -> Self {
        SelfUpdate {
            repo: Mutex::new(RepoStatus::Unknown),
            current: Mutex::new(None),
            restart_required: Mutex::new(None),
        }
    }
}
