//! Auto-amélioration (J14, spec décisions 16 à 19) : clone du dépôt source
//! de l'app, cycle d'une amélioration (worktree `auto/…`, session dédiée,
//! contrôles, tests, build, bascule, fusion fast-forward) et retour arrière.
//!
//! Étape 3 : clone ([`repo`]), git ([`git`]) et cycle ([`pipeline`]) ; envoi
//! des branches `auto/…` réservé au propriétaire ([`push`]).
//! Propriétaire : agent C (`src-tauri/src/selfupdate/**`,
//! `tools/self-improve/**`). Le pipeline ne connaît ni Tauri ni `AppState` :
//! la session dédiée et la bascule d'interface passent par [`PipelineHost`],
//! implémenté à l'étape 5 (intégration) ; npm, node, Playwright et cargo
//! passent par [`pipeline::Toolchain`] ([`pipeline::NpmToolchain`] en vrai).

pub mod git;
pub mod pipeline;
pub mod push;
pub mod repo;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::api::types::{CoreError, ImproveStatus, ImproveStep, PushStatus, RepoStatus};

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
    /// défaut (clé `repo_url`). `install_dir` : dossier de l'exe, retenu
    /// (`repo` dedans) s'il est inscriptible ; sinon
    /// `%LOCALAPPDATA%\orchestrator\repo`.
    pub fn load(install_dir: Option<&Path>) -> RepoConfig {
        let data = data_dir();
        repo::load_from(
            config_path().as_deref(),
            install_dir,
            data.as_deref(),
            crate::build_info::BUILD_COMMIT,
        )
    }
}

/// Clone le dépôt si besoin et le place sur `config.commit` ; renvoie l'état
/// à afficher (`Disabled { reason }` si git, Node, npm, cargo, le réseau ou
/// la clé SSH manquent). Ne panique jamais. Clonage non interactif, borné
/// par [`repo::CloneOptions::clone_timeout`] : à lancer en tâche de fond
/// (état `Cloning` affiché pendant ce temps).
pub async fn ensure_clone(config: &RepoConfig) -> RepoStatus {
    repo::ensure_clone_with(config, &repo::CloneOptions::default()).await
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
    /// Si le futur est abandonné avant sa fin (annulation), l'hôte doit
    /// arrêter la session.
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

    /// Revient à l'interface active avant le dernier `switch_ui` réussi
    /// (fusion impossible après la bascule : la version activée n'est pas
    /// dans `main`). Étape 5 : `UiHost::rollback`.
    fn rollback_ui(&self) -> BoxFut<'_, Result<(), CoreError>>;

    /// Attend que la session principale soit au repos (étape `WaitingIdle`,
    /// annulable), avant [`PipelineHost::switch_ui`]. Par défaut : immédiat
    /// (`switch_ui` attend de toute façon le repos).
    fn wait_idle(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async { Ok(()) })
    }

    /// Attend l'arrêt effectif de la session dédiée (processus terminé, Job
    /// fermé), après [`PipelineHost::run_session`], qu'il ait abouti ou été
    /// abandonné (annulation) ; appelé avant de rendre `.git/config`
    /// inscriptible et avant les contrôles. Par défaut : immédiat.
    fn session_stopped(&self) -> BoxFut<'_, ()> {
        Box::pin(async {})
    }
}

/// Cycle d'amélioration sur un clone. Un seul cycle à la fois.
pub struct Pipeline {
    pub repo: RepoConfig,
    pub host: Arc<dyn PipelineHost>,
    /// npm, node, Playwright, cargo ([`pipeline::NpmToolchain`] par défaut).
    pub toolchain: Arc<dyn pipeline::Toolchain>,
    /// Dossier des worktrees : `%LOCALAPPDATA%\orchestrator\work`.
    pub work_root: PathBuf,
    /// Dossier des versions d'interface (`%LOCALAPPDATA%\orchestrator\ui`,
    /// celui de `ui::UiHost`) ; le build va dans `<ui_root>\<hash>.tmp`.
    pub ui_root: PathBuf,
    /// Empreinte du contrat de l'exe (`build_info::CONTRACT`).
    pub contract: String,
    active: Mutex<Option<pipeline::Active>>,
    /// Envoi des branches `auto/…` ([`push`]) : `Off` tant que
    /// [`Pipeline::enable_push`] ne l'a pas activé.
    push: Mutex<PushStatus>,
    /// Un seul envoi à la fois (cycle, envois en attente).
    push_lock: tokio::sync::Mutex<()>,
}

impl Pipeline {
    pub fn new(repo: RepoConfig, host: Arc<dyn PipelineHost>) -> Self {
        let data = data_dir().unwrap_or_else(std::env::temp_dir);
        Pipeline {
            repo,
            host,
            toolchain: Arc::new(pipeline::NpmToolchain::default()),
            work_root: data.join("work"),
            ui_root: data.join("ui"),
            contract: crate::build_info::CONTRACT.into(),
            active: Mutex::new(None),
            push: Mutex::new(PushStatus::Off),
            push_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Remplace les commandes npm/node/cargo (tests).
    pub fn with_toolchain(mut self, toolchain: Arc<dyn pipeline::Toolchain>) -> Self {
        self.toolchain = toolchain;
        self
    }

    /// Remplace les dossiers des worktrees et des versions d'interface.
    pub fn with_dirs(mut self, work_root: PathBuf, ui_root: PathBuf) -> Self {
        self.work_root = work_root;
        self.ui_root = ui_root;
        self
    }

    /// Remplace l'empreinte de contrat de référence.
    pub fn with_contract(mut self, contract: impl Into<String>) -> Self {
        self.contract = contract.into();
        self
    }

    /// Construit la demande (identifiant, nom de branche
    /// `auto/<AAAAMMJJ-HHMM>-<sujet>`, heure UTC) à partir du texte.
    pub fn request(&self, text: &str) -> Result<ImproveRequest, CoreError> {
        self.make_request(text)
    }

    /// Déroule un cycle complet ; chaque transition passe par `sink`. Un
    /// échec d'étape est un `Ok(Failed { .. })` ; `Err` : un cycle est déjà
    /// en cours.
    pub async fn run(
        &self,
        request: ImproveRequest,
        sink: StatusSink,
    ) -> Result<ImproveOutcome, CoreError> {
        self.run_cycle(request, None, sink).await
    }

    /// Annule le cycle `id` en cours (session dédiée arrêtée, branche
    /// laissée non fusionnée). Sans effet une fois la bascule commencée.
    pub async fn cancel(&self, id: &str) -> Result<(), CoreError> {
        self.cancel_active(id)
    }

    /// `git revert` du commit `commit` par le même cycle (worktree
    /// `auto/…-revert-<hash>`, tests, build, fast-forward).
    pub async fn revert(
        &self,
        commit: &str,
        sink: StatusSink,
    ) -> Result<ImproveOutcome, CoreError> {
        let (request, full) = self.revert_request(commit).await?;
        self.run_cycle(request, Some(full), sink).await
    }

    /// État de l'envoi des branches `auto/…` (affiché par `get_ui_info`).
    pub fn push_status(&self) -> PushStatus {
        self.push
            .lock()
            .map(|p| p.clone())
            .unwrap_or(PushStatus::Off)
    }

    /// Active l'envoi si `requested` (réglage `push` de `config.json`) et si
    /// le dépôt distant accorde le droit d'écriture
    /// ([`push::check_write_access`]) ; sinon `Off` (réglage absent) ou
    /// `Disabled { reason }`. Clone prêt requis. Envoi activé : les envois
    /// en attente sont refaits. Renvoie le nouvel état.
    pub async fn enable_push(&self, requested: bool) -> PushStatus {
        self.enable_push_with(requested, push::PUSH_TIMEOUT).await
    }

    /// [`Pipeline::enable_push`] avec un délai choisi (tests).
    pub async fn enable_push_with(
        &self,
        requested: bool,
        timeout: std::time::Duration,
    ) -> PushStatus {
        if !requested {
            self.set_push(PushStatus::Off);
            return PushStatus::Off;
        }
        self.set_push(PushStatus::Checking);
        let state = {
            let _l = self.push_lock.lock().await;
            if !repo::is_clone(&self.repo.dir) {
                PushStatus::Disabled {
                    reason: "clone de l'app indisponible".into(),
                }
            } else {
                match push::check_write_access(&self.repo.dir, &self.repo.url, timeout).await {
                    Ok(()) => PushStatus::Enabled {
                        url: self.repo.url.clone(),
                    },
                    Err(e) => PushStatus::Disabled {
                        reason: e.reason().to_string(),
                    },
                }
            }
        };
        self.set_push(state.clone());
        match &state {
            PushStatus::Enabled { url } => {
                tracing::info!("envoi des améliorations activé vers {url}");
                let n = self.retry_pending().await;
                if n > 0 {
                    tracing::info!("{n} envoi(s) en attente refait(s)");
                }
            }
            PushStatus::Disabled { reason } => {
                tracing::warn!("envoi des améliorations désactivé : {reason}")
            }
            _ => {}
        }
        state
    }

    /// Envois en attente (échecs précédents).
    pub fn pending_pushes(&self) -> Vec<push::Pending> {
        self.pending().read()
    }

    /// Refait les envois en attente si l'envoi est activé ; renvoie le
    /// nombre de branches envoyées.
    pub async fn retry_pending(&self) -> usize {
        let PushStatus::Enabled { url } = self.push_status() else {
            return 0;
        };
        let _l = self.push_lock.lock().await;
        let list = self.pending();
        let mut sent = 0;
        for p in list.read() {
            match push::push_branch(
                &self.repo.dir,
                &url,
                &p.branch,
                &p.commit,
                push::PUSH_TIMEOUT,
            )
            .await
            {
                Ok(()) => {
                    list.remove(&p.branch);
                    sent += 1;
                    tracing::info!(branch = %p.branch, "envoi en attente fait");
                }
                Err(e) => tracing::warn!(branch = %p.branch, "envoi en attente échoué : {e}"),
            }
        }
        sent
    }

    pub(crate) fn set_push(&self, state: PushStatus) {
        if let Ok(mut p) = self.push.lock() {
            *p = state;
        }
    }

    pub(crate) fn pending(&self) -> push::PendingList {
        push::PendingList(self.work_root.join(push::PENDING_FILE))
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
