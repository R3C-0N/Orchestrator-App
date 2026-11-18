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
pub mod tools;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
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
    // `CloneOptions::default()` lit le registre (processus `reg`) : hors du
    // contexte async.
    let opts = match tokio::task::spawn_blocking(repo::CloneOptions::default).await {
        Ok(o) => o,
        Err(e) => {
            return RepoStatus::Disabled {
                reason: format!("lecture de l'environnement interrompue : {e}"),
            }
        }
    };
    repo::ensure_clone_with(config, &opts).await
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
    /// Seul dépôt qui peut recevoir un envoi ([`DEFAULT_ORIGIN`]).
    push_origin: String,
    /// Envoi des branches `auto/…` ([`push`]) : `Off` tant que
    /// [`Pipeline::enable_push`] ne l'a pas activé.
    push: Mutex<PushStatus>,
    /// Réglage `push` demandé (propriétaire).
    push_requested: AtomicBool,
    /// Envoi refusé pour cette exécution (pas de droit d'écriture, autre
    /// dépôt que le canonique, réglage git douteux) : rien n'est tenté ni
    /// mis en attente.
    push_final: AtomicBool,
    /// Un seul envoi ou une seule sonde à la fois.
    push_lock: tokio::sync::Mutex<()>,
    /// Envois en attente : la liste fait foi en mémoire, relue du disque
    /// seulement à la création du pipeline (lancement de l'app) ; le fichier
    /// n'en est qu'une recopie.
    pending: Mutex<Vec<push::Pending>>,
    /// `config.json` et son empreinte SHA-256 au lancement (`None` :
    /// absent) : une session qui le modifie fait refuser le cycle.
    config_file: Option<PathBuf>,
    config_digest: Option<Vec<u8>>,
    /// Programmes de l'envoi, chemins absolus résolus au lancement
    /// ([`push::resolve_program`]) ; faux ssh dans les tests.
    git_program: String,
    ssh_program: String,
    /// `~/.ssh/config` lu une seule fois au lancement (clés `IdentityFile`
    /// de l'envoi, [`push::ssh_identities`]).
    ssh_config_text: Option<String>,
}

/// Envois en attente relus depuis `file` ; illisible : réécrit aussitôt
/// depuis une liste vide (sinon le contrôle d'après session le prendrait
/// pour une modification de la session).
fn load_pending(file: &push::PendingFile) -> Vec<push::Pending> {
    match file.read() {
        Some(v) => v,
        None => {
            tracing::warn!("{} illisible : remis à vide", file.0.display());
            file.write(&[]);
            Vec::new()
        }
    }
}

/// Chemin absolu de `name` dans le `PATH` du lancement (`name.exe` sous
/// Windows), sinon `name`.
fn program_at_launch(name: &str) -> String {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    match push::resolve_program(&file) {
        Some(p) => p.to_string_lossy().replace('\\', "/"),
        None => {
            tracing::warn!("{file} introuvable dans le PATH : envoi par « {name} »");
            name.to_string()
        }
    }
}

/// `%USERPROFILE%`.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// SHA-256 d'un fichier, `None` s'il est absent ou illisible.
fn file_digest(p: &Path) -> Option<Vec<u8>> {
    use sha2::{Digest, Sha256};
    std::fs::read(p).ok().map(|b| Sha256::digest(&b).to_vec())
}

impl Pipeline {
    pub fn new(repo: RepoConfig, host: Arc<dyn PipelineHost>) -> Self {
        let data = data_dir().unwrap_or_else(std::env::temp_dir);
        let config_file = config_path();
        let config_digest = config_file.as_deref().and_then(file_digest);
        let work_root = data.join("work");
        let pending = load_pending(&push::PendingFile(work_root.join(push::PENDING_FILE)));
        let ssh_config_text =
            home_dir().and_then(|h| std::fs::read_to_string(h.join(".ssh").join("config")).ok());
        Pipeline {
            repo,
            host,
            toolchain: Arc::new(pipeline::NpmToolchain::default()),
            work_root,
            ui_root: data.join("ui"),
            contract: crate::build_info::CONTRACT.into(),
            active: Mutex::new(None),
            push_origin: DEFAULT_ORIGIN.into(),
            push: Mutex::new(PushStatus::Off),
            push_requested: AtomicBool::new(false),
            push_final: AtomicBool::new(false),
            push_lock: tokio::sync::Mutex::new(()),
            pending: Mutex::new(pending),
            config_file,
            config_digest,
            git_program: program_at_launch("git"),
            ssh_program: program_at_launch("ssh"),
            ssh_config_text,
        }
    }

    /// Remplace les commandes npm/node/cargo (tests).
    pub fn with_toolchain(mut self, toolchain: Arc<dyn pipeline::Toolchain>) -> Self {
        self.toolchain = toolchain;
        self
    }

    /// Remplace les dossiers des worktrees et des versions d'interface (les
    /// envois en attente sont relus depuis le nouveau dossier).
    pub fn with_dirs(mut self, work_root: PathBuf, ui_root: PathBuf) -> Self {
        self.work_root = work_root;
        self.ui_root = ui_root;
        let pending = load_pending(&self.pending_file());
        self.pending = Mutex::new(pending);
        self
    }

    /// Remplace l'empreinte de contrat de référence.
    pub fn with_contract(mut self, contract: impl Into<String>) -> Self {
        self.contract = contract.into();
        self
    }

    /// Remplace le dépôt canonique, seul destinataire d'un envoi (tests).
    pub fn with_push_origin(mut self, url: impl Into<String>) -> Self {
        self.push_origin = url.into();
        self
    }

    /// Remplace le programme ssh de l'envoi et `~/.ssh/config` (tests).
    pub fn with_ssh(mut self, program: impl Into<String>, config_text: Option<String>) -> Self {
        self.ssh_program = program.into();
        self.ssh_config_text = config_text;
        self
    }

    /// Environnement de la sonde et de l'envoi ([`push::PushEnv`]).
    pub fn push_env(&self) -> push::PushEnv {
        let host = push::ssh_repo_key(&self.push_origin)
            .map(|(h, _)| h)
            .unwrap_or_default();
        let identities = match (&self.ssh_config_text, home_dir()) {
            (Some(text), Some(home)) => push::ssh_identities(text, &host, &home),
            _ => Vec::new(),
        };
        push::PushEnv {
            dir: self.work_root.join(".push-env"),
            git_program: self.git_program.clone(),
            ssh_program: self.ssh_program.clone(),
            host,
            identities,
        }
    }

    /// Remplace `config.json` (tests) ; son empreinte est relevée ici.
    pub fn with_config_file(mut self, path: PathBuf) -> Self {
        self.config_digest = file_digest(&path);
        self.config_file = Some(path);
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

    /// Active l'envoi si `requested` (réglage `push` de `config.json`), si
    /// la destination est le dépôt canonique et si celui-ci accorde le droit
    /// d'écriture ([`push::check_write_access`]) ; sinon `Off` (réglage
    /// absent) ou `Disabled { reason }`. Clone prêt requis. Envoi activé :
    /// les envois en attente sont refaits. Renvoie le nouvel état.
    pub async fn enable_push(&self, requested: bool) -> PushStatus {
        self.enable_push_with(requested, push::PUSH_TIMEOUT).await
    }

    /// [`Pipeline::enable_push`] avec un délai choisi (tests).
    pub async fn enable_push_with(
        &self,
        requested: bool,
        timeout: std::time::Duration,
    ) -> PushStatus {
        self.push_requested.store(requested, Ordering::SeqCst);
        self.push_final.store(false, Ordering::SeqCst);
        if !requested {
            self.set_push(PushStatus::Off);
            return PushStatus::Off;
        }
        self.set_push(PushStatus::Checking);
        let state = self.probe(timeout).await;
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

    /// Sonde du droit d'écriture (destination, clone, `--dry-run`) ; met à
    /// jour l'état et le refus définitif. Renvoie le nouvel état.
    pub(crate) async fn probe(&self, timeout: std::time::Duration) -> PushStatus {
        let _l = self.push_lock.lock().await;
        let (state, final_) = if !repo::is_clone(&self.repo.dir) {
            (
                PushStatus::Disabled {
                    reason: "clone de l'app indisponible".into(),
                },
                false,
            )
        } else {
            match push::check_write_access(
                &self.repo.dir,
                &self.repo.url,
                &self.push_origin,
                &self.push_env(),
                timeout,
            )
            .await
            {
                Ok(()) => (
                    PushStatus::Enabled {
                        url: self.repo.url.clone(),
                    },
                    false,
                ),
                Err(e) => (
                    PushStatus::Disabled {
                        reason: e.reason().to_string(),
                    },
                    e.is_final(),
                ),
            }
        };
        self.push_final.store(final_, Ordering::SeqCst);
        self.set_push(state.clone());
        state
    }

    /// Réglage `push` demandé et pas de refus définitif : une fusion doit
    /// être envoyée ou mise en attente.
    pub(crate) fn push_wanted(&self) -> bool {
        self.push_requested.load(Ordering::SeqCst) && !self.push_final.load(Ordering::SeqCst)
    }

    /// Refus définitif de l'envoi pour cette exécution.
    pub(crate) fn push_refused(&self) -> bool {
        self.push_final.load(Ordering::SeqCst)
    }

    /// Envois en attente (échecs précédents), tels que tenus en mémoire.
    pub fn pending_pushes(&self) -> Vec<push::Pending> {
        self.pending.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// Refait les envois en attente si l'envoi est activé ; renvoie le
    /// nombre de branches envoyées.
    pub async fn retry_pending(&self) -> usize {
        let PushStatus::Enabled { url } = self.push_status() else {
            return 0;
        };
        let _l = self.push_lock.lock().await;
        let mut sent = 0;
        for p in self.pending_pushes() {
            match push::push_branch(
                &self.repo.dir,
                &url,
                &self.push_origin,
                &self.push_env(),
                &p.branch,
                &p.commit,
                push::PUSH_TIMEOUT,
            )
            .await
            {
                Ok(()) => {
                    self.pending_remove(&p.branch);
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

    fn pending_file(&self) -> push::PendingFile {
        push::PendingFile(self.work_root.join(push::PENDING_FILE))
    }

    /// Ajoute (ou remplace) l'envoi en attente de `p.branch`.
    pub(crate) fn pending_add(&self, p: push::Pending) {
        if let Ok(mut v) = self.pending.lock() {
            v.retain(|x| x.branch != p.branch);
            v.push(p);
            self.pending_file().write(&v);
        }
    }

    /// Retire l'envoi en attente de `branch`.
    pub(crate) fn pending_remove(&self, branch: &str) {
        if let Ok(mut v) = self.pending.lock() {
            let n = v.len();
            v.retain(|x| x.branch != branch);
            if v.len() != n {
                self.pending_file().write(&v);
            }
        }
    }

    /// Contrôles d'après session sur les fichiers de l'app que la session
    /// peut atteindre : `config.json` identique à son état au lancement, et
    /// `push-pending.json` identique à la liste en mémoire (sinon il est
    /// réécrit depuis la mémoire). `Err` : raison lisible.
    pub(crate) fn check_app_files(&self) -> Result<(), String> {
        // Liste rétablie d'abord, même si `config.json` fait aussi refuser.
        let pending = self.check_pending_file();
        self.check_config_file()?;
        pending
    }

    /// `config.json` identique à son état au lancement de l'app.
    pub(crate) fn check_config_file(&self) -> Result<(), String> {
        if let Some(cfg) = &self.config_file {
            if file_digest(cfg) != self.config_digest {
                return Err(format!(
                    "{} a changé depuis le lancement de l'app (modifié pendant la session ?) : \
                     cycle refusé ; relancer l'app si la modification est voulue",
                    cfg.display()
                ));
            }
        }
        Ok(())
    }

    /// `push-pending.json` identique à la liste en mémoire, sinon réécrit
    /// depuis elle.
    fn check_pending_file(&self) -> Result<(), String> {
        let mem = self.pending_pushes();
        if self.pending_file().read().as_ref() != Some(&mem) {
            self.pending_file().write(&mem);
            return Err(format!(
                "{} modifié pendant la session : cycle refusé (liste des envois en attente \
                 rétablie)",
                self.pending_file().0.display()
            ));
        }
        Ok(())
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
