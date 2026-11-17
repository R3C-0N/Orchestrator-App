//! Commandes Tauri de l'auto-amélioration (J14, spec décisions 15 à 19) et
//! branchement du cycle sur l'app (étape 5). `attach_sessions` est dans
//! `commands.rs` (sessions).
//!
//! - [`setup`] (appelé par `lib.rs`) : configuration du clone, état
//!   `Cloning`, `ensure_clone` en tâche de fond, puis `cleanup_merged` ;
//!   cycle [`Pipeline`] partagé (un seul à la fois) géré par Tauri
//!   ([`Improver`]).
//! - [`AppHost`] : le [`PipelineHost`] réel. Session dédiée par
//!   `AppState::start_improve` (outils git dangereux refusés, git sans
//!   réseau ni hooks), protégée de la fermeture différée tant que le cycle
//!   la tient, arrêtée à la fin ou à l'abandon du futur ; bascule
//!   d'interface par `UiHost` (empreinte du contrat vérifiée avant
//!   d'activer, signal « prêt » attendu, nettoyage).
//! - [`LockingToolchain`] : la toolchain npm réelle, avec le verrou de build
//!   `UiHost::build_lock` tenu pendant le `vite build`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::commands::AppState;
use super::types::{
    CoreError, ImproveStatus, ImproveStep, RepoStatus, SessionKey, UiInfo, UiSource, IMPROVE_EVENT,
};
use crate::process::spawn::LaunchOptions;
use crate::process::supervisor::TurnWait;
use crate::selfupdate::pipeline::{self, NpmToolchain, Toolchain};
use crate::selfupdate::{
    BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, RepoConfig, SelfUpdate,
    StatusSink,
};
use crate::ui::{UiHost, READY_TIMEOUT};

/// Durée maximale du tour de la session d'amélioration.
pub const IMPROVE_SESSION_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Outils refusés à la session d'amélioration (relecture de sécurité de
/// l'étape 3) : rien qui pousse, change les dépôts distants, la
/// configuration git, les sous-modules, les worktrees ou les références à
/// la main ; et pas de question (personne ne la verrait). Refus tenus même
/// en `bypassPermissions` (vérifié, test `improve_live`).
pub const DISALLOWED_TOOLS: &[&str] = &[
    "Bash(git push:*)",
    "Bash(git remote:*)",
    "Bash(git config:*)",
    "Bash(git submodule:*)",
    "Bash(git worktree:*)",
    "Bash(git update-ref:*)",
    "Bash(git branch -f:*)",
    "AskUserQuestion",
];

/// Environnement de la session d'amélioration : git sans SSH ni invite,
/// sans hooks, sans transport `ext::`.
pub fn improve_env() -> Vec<(String, String)> {
    [
        ("GIT_SSH_COMMAND", "false"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_CONFIG_COUNT", "2"),
        ("GIT_CONFIG_KEY_0", "core.hooksPath"),
        ("GIT_CONFIG_VALUE_0", "NUL"),
        ("GIT_CONFIG_KEY_1", "protocol.ext.allow"),
        ("GIT_CONFIG_VALUE_1", "never"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Options de lancement de la session d'amélioration (`model` : clé
/// `improve_model` de `config.json`, sinon le modèle par défaut du CLI).
pub fn improve_launch_options(model: Option<String>) -> LaunchOptions {
    let mut extra_args = vec!["--disallowedTools".to_string()];
    extra_args.extend(DISALLOWED_TOOLS.iter().map(|t| t.to_string()));
    LaunchOptions {
        resume: None,
        model,
        extra_args,
        env: improve_env(),
    }
}

/// Clé `improve_model` de `config.json` (modèle de la session
/// d'amélioration), validée comme `--model`.
fn configured_model() -> Option<String> {
    let path = crate::selfupdate::config_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let m = v.get("improve_model")?.as_str()?.trim().to_string();
    match super::commands::validate_model(&m) {
        Ok(()) => Some(m),
        Err(e) => {
            tracing::warn!("config.json : improve_model ignoré ({e})");
            None
        }
    }
}

/// La chaîne `contract` apparaît dans `bytes` comme littéral de chaîne
/// (entre `"`, `'` ou `` ` `` : le minifieur de Vite écrit des gabarits).
fn has_string_literal(bytes: &[u8], contract: &str) -> bool {
    let c = contract.as_bytes();
    if c.is_empty() || bytes.len() < c.len() + 2 {
        return false;
    }
    bytes.windows(c.len() + 2).any(|w| {
        let q = w[0];
        matches!(q, b'"' | b'\'' | b'`') && w[w.len() - 1] == q && &w[1..w.len() - 1] == c
    })
}

/// La sortie de Vite dans `dir` porte l'empreinte `contract` (constante
/// `__CONTRACT__` inlinée dans les scripts) : l'interface a été construite
/// pour le contrat de cet exe.
pub fn ui_contract_matches(dir: &Path, contract: &str) -> bool {
    fn walk(d: &Path, contract: &str, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(d) else {
            return false;
        };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                if walk(&p, contract, depth + 1) {
                    return true;
                }
            } else if ft.is_file()
                && p.extension().is_some_and(|x| x == "js")
                && std::fs::read(&p).is_ok_and(|b| has_string_literal(&b, contract))
            {
                return true;
            }
        }
        false
    }
    walk(dir, contract, 0)
}

/// Démarrage : la version d'interface active a été construite pour un autre
/// contrat que celui de l'exe (exe recompilé et réinstallé après une
/// « relance requise », par exemple) ; elle appellerait des commandes
/// absentes ou changées. On la quitte, sans la marquer fautive, pour la
/// précédente compatible, sinon l'interface embarquée. À appeler avant
/// `install_assets`. Renvoie les versions quittées.
pub fn leave_incompatible_ui(ui: &UiHost, contract: &str) -> Vec<String> {
    let mut left = Vec::new();
    for _ in 0..crate::ui::KEEP_RECENT + 2 {
        let Some(h) = ui.store().read_pointer().active else {
            break;
        };
        if ui_contract_matches(&ui.store().version_dir(&h), contract) {
            break;
        }
        tracing::warn!(
            "interface {h} construite pour un autre contrat que celui de l'exe ({contract}) : \
             non servie"
        );
        if ui.step_back().is_err() {
            break;
        }
        left.push(h);
    }
    left
}

/// Supprime les verrous de build `<hash>.lock` laissés par un arrêt brutal
/// (aucun build ne tourne au démarrage) ; sans cela, `build_lock` du même
/// hash échouerait et le `.tmp` resterait protégé du nettoyage. Renvoie le
/// nombre de verrous supprimés.
pub fn clear_orphan_build_locks(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(hash) = name.strip_suffix(crate::ui::store::LOCK_SUFFIX) else {
            continue;
        };
        if !crate::ui::store::valid_hash(hash) || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        if std::fs::remove_file(e.path()).is_ok() {
            tracing::warn!("verrou de build orphelin supprimé : {name}");
            n += 1;
        }
    }
    n
}

/// Cycle d'amélioration de l'app, géré par Tauri.
pub struct Improver {
    pub pipeline: Arc<Pipeline>,
    /// Un cycle (amélioration ou retour arrière) est lancé ou en cours.
    running: Arc<AtomicBool>,
    /// Session dédiée du cycle en cours (reportée dans `ImproveStatus`).
    session: Arc<Mutex<Option<SessionKey>>>,
}

/// Remet `running` à faux à la fin de la tâche du cycle, même en panique.
struct Running(Arc<AtomicBool>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Improver {
    fn try_begin(&self) -> Result<Running, CoreError> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CoreError::InvalidArgument(
                "une amélioration est déjà en cours".into(),
            ));
        }
        *self.session.lock().unwrap() = None;
        Ok(Running(self.running.clone()))
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Transitions d'un cycle : événement global [`IMPROVE_EVENT`],
/// `SelfUpdate.current` et `restart_required` à jour.
fn status_sink<R: Runtime>(
    app: AppHandle<R>,
    session: Arc<Mutex<Option<SessionKey>>>,
) -> StatusSink {
    Arc::new(move |mut st: ImproveStatus| {
        if st.session.is_none() {
            st.session = session.lock().unwrap().clone();
        }
        publish(&app, st);
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, st: ImproveStatus) {
    let su = app.state::<SelfUpdate>();
    if st.step == ImproveStep::RestartRequired {
        *su.restart_required.lock().unwrap() = st.commit.clone();
    }
    *su.current.lock().unwrap() = Some(st.clone());
    tracing::info!(id = %st.id, step = ?st.step, detail = ?st.detail, "amélioration");
    if let Err(e) = app.emit(IMPROVE_EVENT, &st) {
        tracing::warn!("{IMPROVE_EVENT} non émis : {e}");
    }
}

/// Toolchain npm réelle avec le verrou de build de l'interface.
pub struct LockingToolchain<R: Runtime> {
    pub inner: NpmToolchain,
    pub app: AppHandle<R>,
}

impl<R: Runtime> Toolchain for LockingToolchain<R> {
    fn preflight(&self) -> BoxFut<'_, Result<(), String>> {
        self.inner.preflight()
    }
    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>> {
        self.inner.install_deps(dir)
    }
    fn test<'a>(&'a self, tree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>> {
        self.inner.test(tree, rust)
    }
    fn build_ui<'a>(
        &'a self,
        worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let _lock = self
                .app
                .state::<UiHost>()
                .build_lock(hash)
                .map_err(|e| format!("verrou de build de {hash} : {e}"))?;
            self.inner.build_ui(worktree, hash, out).await
        })
    }
    fn contract<'a>(&'a self, tree: &'a Path) -> BoxFut<'a, Result<String, String>> {
        self.inner.contract(tree)
    }
}

/// Arrête la session dédiée quand le cycle la lâche (fin normale ou futur
/// abandonné par l'annulation).
struct StopOnDrop<R: Runtime> {
    app: AppHandle<R>,
    key: SessionKey,
}

impl<R: Runtime> Drop for StopOnDrop<R> {
    fn drop(&mut self) {
        let app = self.app.clone();
        let key = self.key.clone();
        tauri::async_runtime::spawn(async move {
            if app.state::<AppState>().close_session(&key).await.is_some() {
                tracing::info!(session = %key.0, "session d'amélioration arrêtée");
            }
        });
    }
}

/// [`PipelineHost`] de l'app.
pub struct AppHost<R: Runtime> {
    pub app: AppHandle<R>,
    pub model: Option<String>,
    session: Arc<Mutex<Option<SessionKey>>>,
}

impl<R: Runtime> AppHost<R> {
    /// Attend le signal « prêt » de `hash` après `activate` : vrai s'il est
    /// arrivé, faux si le watchdog est revenu en arrière (ou au-delà du
    /// délai, marge comprise).
    async fn wait_ready(&self, hash: &str) -> bool {
        let ui = self.app.state::<UiHost>();
        let deadline = Instant::now() + READY_TIMEOUT + Duration::from_secs(2);
        loop {
            let active = ui.active();
            if active.hash.as_deref() != Some(hash) {
                return false;
            }
            if ui.watchdog_expected().as_deref() != Some(hash) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl<R: Runtime> PipelineHost for AppHost<R> {
    fn run_session<'a>(
        &'a self,
        _request: &'a ImproveRequest,
        cwd: &'a Path,
        prompt: &'a str,
    ) -> BoxFut<'a, Result<(), CoreError>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let s = state
                .start_improve(cwd, prompt, improve_launch_options(self.model.clone()))
                .await?;
            // Dans cet ordre à la destruction : arrêt, puis levée de la
            // protection (la session n'est plus dans la table).
            let _stop = StopOnDrop {
                app: self.app.clone(),
                key: s.key.clone(),
            };
            let _protect = state.protect(&s.key);
            *self.session.lock().unwrap() = Some(s.key.clone());
            // Le bandeau voit la session dès maintenant.
            let cur = self
                .app
                .state::<SelfUpdate>()
                .current
                .lock()
                .unwrap()
                .clone();
            if let Some(mut st) = cur {
                st.session = Some(s.key.clone());
                publish(&self.app, st);
            }
            match s.wait_turn_end(0, IMPROVE_SESSION_TIMEOUT).await {
                TurnWait::Done(_) => Ok(()),
                TurnWait::Exited => Err(CoreError::Io(format!(
                    "la session dédiée s'est arrêtée avant la fin de son tour (code {:?})",
                    s.exit_code().flatten()
                ))),
                TurnWait::Timeout => Err(CoreError::Timeout(format!(
                    "session dédiée ({} min)",
                    IMPROVE_SESSION_TIMEOUT.as_secs() / 60
                ))),
            }
        })
    }

    fn switch_ui<'a>(
        &'a self,
        hash: &'a str,
        dir: &'a Path,
    ) -> BoxFut<'a, Result<bool, CoreError>> {
        Box::pin(async move {
            // Le repos a pu cesser depuis `wait_idle` : on le redemande.
            self.app.state::<AppState>().wait_main_idle().await;
            if !ui_contract_matches(dir, crate::build_info::CONTRACT) {
                return Err(CoreError::InvalidArgument(format!(
                    "l'interface {hash} n'a pas l'empreinte du contrat de l'exe ({}) : \
                     relance requise, version non activée",
                    crate::build_info::CONTRACT
                )));
            }
            let ui = self.app.state::<UiHost>();
            {
                let _lock = ui.build_lock(hash)?;
                ui.install(hash, dir)?;
            }
            ui.activate(&self.app, hash)?;
            let ready = self.wait_ready(hash).await;
            let removed = ui.cleanup();
            tracing::info!(hash, ready, removed, "bascule d'interface");
            Ok(ready)
        })
    }

    fn rollback_ui(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move { self.app.state::<UiHost>().rollback(&self.app).map(drop) })
    }

    fn wait_idle(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move {
            self.app.state::<AppState>().wait_main_idle().await;
            Ok(())
        })
    }
}

/// Dossier de l'exe (clone en release, décision 16) ; `None` en debug, sinon
/// le clone atterrirait dans `target\debug`.
fn install_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        return None;
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Démarrage (`Builder::setup`) : cycle géré par Tauri, clone en tâche de
/// fond, puis nettoyage des worktrees fusionnés.
pub fn setup<R: Runtime>(app: &AppHandle<R>) {
    let config = RepoConfig::load(install_dir().as_deref());
    tracing::info!(url = %config.url, dir = %config.dir.display(), commit = %config.commit, "clone de l'app");
    let session = Arc::new(Mutex::new(None));
    let host = Arc::new(AppHost {
        app: app.clone(),
        model: configured_model(),
        session: session.clone(),
    });
    let ui_root = app.state::<UiHost>().root().to_path_buf();
    let data = crate::selfupdate::data_dir().unwrap_or_else(std::env::temp_dir);
    let pipeline = Arc::new(
        Pipeline::new(config.clone(), host)
            .with_toolchain(Arc::new(LockingToolchain {
                inner: NpmToolchain::default(),
                app: app.clone(),
            }))
            .with_dirs(data.join("work"), ui_root),
    );
    app.manage(Improver {
        pipeline: pipeline.clone(),
        running: Arc::default(),
        session,
    });

    let su = app.state::<SelfUpdate>();
    *su.repo.lock().unwrap() = RepoStatus::Cloning {
        path: config.dir.display().to_string(),
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let status = crate::selfupdate::ensure_clone(&config).await;
        match &status {
            RepoStatus::Disabled { reason } => {
                tracing::warn!("/ameliorer désactivé : {reason}")
            }
            s => tracing::info!("clone de l'app : {s:?}"),
        }
        let ready = matches!(status, RepoStatus::Ready { .. });
        *app.state::<SelfUpdate>().repo.lock().unwrap() = status;
        if ready {
            let n = pipeline.cleanup_merged().await;
            if n > 0 {
                tracing::info!("{n} worktree(s) d'amélioration fusionné(s) supprimé(s)");
            }
        }
    });
}

fn ui_info(ui: &UiHost, su: &SelfUpdate) -> UiInfo {
    let active = ui.active();
    UiInfo {
        source: active.source,
        active_hash: active.hash,
        previous_hash: active.previous,
        build_commit: crate::build_info::BUILD_COMMIT.into(),
        contract: crate::build_info::CONTRACT.into(),
        restart_required: su.restart_required.lock().unwrap().clone(),
        repo: su.repo.lock().unwrap().clone(),
        improve: su.current.lock().unwrap().clone(),
    }
}

/// Interface affichée, état du clone et de l'amélioration en cours.
#[tauri::command(rename_all = "snake_case")]
pub fn get_ui_info(ui: State<'_, UiHost>, su: State<'_, SelfUpdate>) -> UiInfo {
    ui_info(&ui, &su)
}

/// Signal « prêt » de l'interface, après son montage et `attach_sessions`.
/// `hash` : `__UI_HASH__` de l'interface (`None` : embarquée). Renvoie vrai
/// si ce signal était attendu par le watchdog.
#[tauri::command(rename_all = "snake_case")]
pub fn ui_ready(ui: State<'_, UiHost>, hash: Option<String>) -> bool {
    ui.ready(hash.as_deref())
}

/// Lance le cycle `run` (amélioration ou retour arrière) en tâche de fond ;
/// à la fin, worktrees fusionnés nettoyés.
fn spawn_cycle<R, F>(app: AppHandle<R>, guard: Running, run: F)
where
    R: Runtime,
    F: std::future::Future<Output = Result<ImproveOutcome, CoreError>> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let out = run.await;
        match &out {
            Ok(o) => tracing::info!("fin du cycle : {o:?}"),
            Err(e) => tracing::warn!("cycle refusé : {e}"),
        }
        drop(guard);
        let pipeline = app.state::<Improver>().pipeline.clone();
        pipeline.cleanup_merged().await;
    });
}

/// Lance une amélioration (`/ameliorer <request>`) ; l'avancement arrive par
/// l'événement global `improve_status` (`IMPROVE_EVENT`).
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_start(
    app: AppHandle,
    su: State<'_, SelfUpdate>,
    improver: State<'_, Improver>,
    request: String,
) -> Result<ImproveStatus, CoreError> {
    match &*su.repo.lock().unwrap() {
        RepoStatus::Ready { .. } => {}
        RepoStatus::Disabled { reason } => {
            return Err(CoreError::InvalidArgument(format!(
                "/ameliorer désactivé : {reason}"
            )))
        }
        RepoStatus::Cloning { .. } | RepoStatus::Unknown => {
            return Err(CoreError::InvalidArgument(
                "clone de l'app en cours de préparation".into(),
            ))
        }
    }
    let req = improver.pipeline.request(&request)?;
    let guard = improver.try_begin()?;
    let status = ImproveStatus {
        id: req.id.clone(),
        branch: req.branch.clone(),
        request: req.text.clone(),
        step: ImproveStep::Preparing,
        session: None,
        commit: None,
        detail: None,
        t: pipeline::now_ms(),
    };
    *su.current.lock().unwrap() = Some(status.clone());
    let pipeline = improver.pipeline.clone();
    let sink = status_sink(app.clone(), improver.session.clone());
    spawn_cycle(app, guard, async move { pipeline.run(req, sink).await });
    Ok(status)
}

/// Annule l'amélioration `id` en cours.
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_cancel(improver: State<'_, Improver>, id: String) -> Result<(), CoreError> {
    improver.pipeline.cancel(&id).await
}

/// Revient à la version précédente de l'interface, puis, si une version
/// disque était active, lance son `git revert` par le cycle d'amélioration
/// (en tâche de fond, avancement par `IMPROVE_EVENT`).
#[tauri::command(rename_all = "snake_case")]
pub fn ui_rollback(
    app: AppHandle,
    ui: State<'_, UiHost>,
    su: State<'_, SelfUpdate>,
    improver: State<'_, Improver>,
) -> Result<UiInfo, CoreError> {
    if improver.is_running() {
        return Err(CoreError::InvalidArgument(
            "une amélioration est en cours : retour arrière impossible maintenant".into(),
        ));
    }
    let before = ui.active();
    ui.rollback(&app)?;
    let left = match (before.source, before.hash) {
        (UiSource::Disk, Some(h)) => Some(h),
        _ => None,
    };
    let ready = matches!(*su.repo.lock().unwrap(), RepoStatus::Ready { .. });
    if let (Some(commit), true) = (left, ready) {
        if let Ok(guard) = improver.try_begin() {
            let pipeline = improver.pipeline.clone();
            let sink = status_sink(app.clone(), improver.session.clone());
            let app2 = app.clone();
            spawn_cycle(app, guard, async move {
                match pipeline.revert(&commit, sink).await {
                    Err(e) => {
                        // Version absente du main du clone (installée à la
                        // main, ou jamais fusionnée) : l'interface est
                        // revenue en arrière, rien à annuler dans git.
                        publish(
                            &app2,
                            ImproveStatus {
                                id: uuid::Uuid::new_v4().to_string(),
                                branch: String::new(),
                                request: format!("revenir avant {commit}"),
                                step: ImproveStep::RolledBack,
                                session: None,
                                commit: None,
                                detail: Some(format!(
                                    "interface précédente réactivée ; pas de git revert : {e}"
                                )),
                                t: pipeline::now_ms(),
                            },
                        );
                        Err(e)
                    }
                    ok => ok,
                }
            });
        }
    }
    Ok(ui_info(&ui, &su))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn improve_options_refuse_git_tools_and_isolate_git() {
        let o = improve_launch_options(Some("haiku".into()));
        assert_eq!(o.extra_args[0], "--disallowedTools");
        for t in [
            "Bash(git push:*)",
            "Bash(git remote:*)",
            "Bash(git config:*)",
            "Bash(git submodule:*)",
            "Bash(git worktree:*)",
            "Bash(git update-ref:*)",
            "Bash(git branch -f:*)",
        ] {
            assert!(o.extra_args.iter().any(|a| a == t), "{t}");
        }
        let env: std::collections::HashMap<_, _> = o.env.into_iter().collect();
        assert_eq!(env["GIT_SSH_COMMAND"], "false");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        assert_eq!(env["GIT_CONFIG_KEY_0"], "core.hooksPath");
        assert_eq!(env["GIT_CONFIG_VALUE_0"], "NUL");
        assert_eq!(env["GIT_CONFIG_KEY_1"], "protocol.ext.allow");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "never");
        // Les options passent la validation du lancement.
        let o = improve_launch_options(Some("haiku".into()));
        crate::process::spawn::validate_launch_options(&o).unwrap();
    }

    #[test]
    fn contract_is_found_in_built_scripts_only() {
        let d = std::env::temp_dir().join(format!("orch-contract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("assets")).unwrap();
        std::fs::write(d.join("index.html"), "\"0123456789abcdef\"").unwrap();
        assert!(!ui_contract_matches(&d, "0123456789abcdef"));
        // Sortie réelle de Vite 7 (gabarit) : Ze=`<empreinte>`.
        std::fs::write(
            d.join("assets/index-x.js"),
            "Xe=`4e111e3`,Ze=`0123456789abcdef`,",
        )
        .unwrap();
        assert!(ui_contract_matches(&d, "0123456789abcdef"));
        std::fs::write(d.join("assets/index-x.js"), "const c=\"0123456789abcdef\";").unwrap();
        assert!(ui_contract_matches(&d, "0123456789abcdef"));
        // Préfixe d'une autre chaîne : pas un littéral entier.
        std::fs::write(d.join("assets/index-x.js"), "c=`0123456789abcdef0`").unwrap();
        assert!(!ui_contract_matches(&d, "0123456789abcdef"));
        std::fs::write(d.join("assets/index-x.js"), "c=\"0123456789abcdef`").unwrap();
        assert!(!ui_contract_matches(&d, "0123456789abcdef"));
        assert!(!ui_contract_matches(&d, "fedcba9876543210"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn active_ui_of_another_contract_is_left_at_startup() {
        let d = std::env::temp_dir().join(format!("orch-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let build = |name: &str, contract: &str| {
            let b = d.join("builds").join(name);
            std::fs::create_dir_all(b.join("assets")).unwrap();
            std::fs::write(b.join("index.html"), "<!doctype html>").unwrap();
            std::fs::write(b.join("assets/i.js"), format!("Ze=`{contract}`")).unwrap();
            b
        };
        let ui = UiHost::new(d.join("ui"));
        ui.install("aaaaaaa", &build("a", "1111111111111111"))
            .unwrap();
        ui.switch_to("aaaaaaa").unwrap();
        ui.install("bbbbbbb", &build("b", "2222222222222222"))
            .unwrap();
        ui.switch_to("bbbbbbb").unwrap();
        // Contrat de l'exe = celui de « aaaaaaa » : « bbbbbbb » est quittée.
        let fresh = UiHost::new(d.join("ui"));
        assert_eq!(
            leave_incompatible_ui(&fresh, "1111111111111111"),
            vec!["bbbbbbb".to_string()]
        );
        assert_eq!(
            fresh.store().read_pointer().active.as_deref(),
            Some("aaaaaaa")
        );
        assert!(fresh.store().read_state().bad.is_empty(), "pas fautive");
        // Aucun compatible : interface embarquée.
        assert_eq!(
            leave_incompatible_ui(&fresh, "3333333333333333"),
            vec!["aaaaaaa".to_string()]
        );
        assert_eq!(fresh.store().read_pointer().active, None);
        assert!(leave_incompatible_ui(&fresh, "3333333333333333").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn orphan_build_locks_are_cleared() {
        let d = std::env::temp_dir().join(format!("orch-locks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("abc1234.lock"), "").unwrap();
        std::fs::write(d.join("pas-un-hash.lock"), "").unwrap();
        std::fs::create_dir_all(d.join("def5678.lock")).unwrap();
        assert_eq!(clear_orphan_build_locks(&d), 1);
        assert!(!d.join("abc1234.lock").exists());
        assert!(d.join("pas-un-hash.lock").exists());
        assert!(d.join("def5678.lock").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
