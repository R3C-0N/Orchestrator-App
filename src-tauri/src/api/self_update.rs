//! Commandes Tauri de l'auto-amélioration (J14, spec décisions 15 à 19) et
//! branchement du cycle sur l'app (étape 5). `attach_sessions` est dans
//! `commands.rs` (sessions).
//!
//! - [`setup`] (appelé par `lib.rs`) : configuration du clone, état
//!   `Cloning`, `ensure_clone` en tâche de fond (worktrees jetables des
//!   tests nettoyés, worktrees `auto/…` non fusionnés signalés), puis
//!   `cleanup_merged` ; cycle [`Pipeline`] partagé (un seul à la fois) géré
//!   par Tauri ([`Improver`]).
//! - [`AppHost`] : le [`PipelineHost`] réel. Jalon J16 : une conversation
//!   par cycle (`AppState::improve_conversation`, clé = `id` du cycle), un
//!   processus `claude.exe` par tour (`AppState::run_improve_turn` :
//!   `--session-id` puis `--resume`, outils git dangereux refusés, git sans
//!   réseau ni hooks, environnement en liste blanche), arrêté à la fin de
//!   chaque tour ou à l'abandon du futur ; lignes d'état `notice`
//!   `improve.*` (`note`) ; bascule d'interface par `UiHost` (empreinte du
//!   contrat vérifiée avant d'activer, signal « prêt » attendu, nettoyage).
//! - [`LockingToolchain`] : la toolchain npm réelle, avec le verrou de build
//!   `UiHost::build_lock` tenu pendant le `vite build`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::commands::AppState;
use super::events::NoticeLevel;
use super::types::{
    CoreError, ImproveStatus, ImproveStep, PushStatus, RepoStatus, SessionKey, UiInfo, UiSource,
    IMPROVE_EVENT,
};
use crate::process::locate::{display_path, locate_claude};
use crate::process::spawn::LaunchOptions;
use crate::process::supervisor::SessionConfig;
use crate::selfupdate::pipeline::{self, NpmToolchain, Toolchain};
use crate::selfupdate::{
    BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, RepoConfig, SelfUpdate,
    StatusSink, TurnEnd, MAX_AUTO_ATTEMPTS,
};
use crate::ui::{UiHost, READY_TIMEOUT};

/// Durée maximale du tour de la session d'amélioration.
pub const IMPROVE_SESSION_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Outils refusés à la session d'amélioration (relecture de sécurité de
/// l'étape 3, puis relectures finales) : `git push` sous ses formes
/// écrites (`git push`, `git -C … push`, `git --… push`, `git -c`,
/// `git.exe`), les identifiants (`git credential`, `credential-manager`),
/// la configuration passée autrement (`git --config-env`,
/// `GIT_CONFIG_PARAMETERS`, `GIT_ALLOW_PROTOCOL`), `gh`, `ssh`, `curl`, les
/// dépôts distants, `git config`, les sous-modules, les worktrees, les
/// références à la main ; et pas de question (personne ne la verrait).
/// Refus tenus en `bypassPermissions`, jokers `*` acceptés par le CLI
/// 2.1.326 (vérifié, test `improve_live`). Ce filtre porte sur le texte des
/// commandes : il ne voit pas un `git push` lancé par un autre programme
/// (`cmd /c`, `node -e`), qui échoue sur l'environnement de
/// [`improve_env`]. Ce n'est pas une isolation : la session garde les droits
/// Windows de l'utilisateur (spec, décision 17).
///
/// Formes précises plutôt que `git * push*` : celle-ci refusait aussi
/// `git commit -m "…push…"` ou `git grep push` (le prompt demande de passer
/// un tel message par `git commit -F`).
pub const DISALLOWED_TOOLS: &[&str] = &[
    "Bash(git push:*)",
    "Bash(git push*)",
    "Bash(git -C * push*)",
    "Bash(git --* push*)",
    "Bash(git -c*)",
    "Bash(*git -c *)",
    "Bash(*git.exe*)",
    "Bash(git credential*)",
    "Bash(*credential-manager*)",
    "Bash(git --config-env*)",
    "Bash(*GIT_CONFIG_PARAMETERS*)",
    "Bash(*GIT_ALLOW_PROTOCOL*)",
    "Bash(git remote:*)",
    "Bash(git config:*)",
    "Bash(git submodule:*)",
    "Bash(git worktree:*)",
    "Bash(git update-ref:*)",
    "Bash(git branch -f:*)",
    "Bash(gh:*)",
    "Bash(ssh:*)",
    "Bash(curl:*)",
    "AskUserQuestion",
];

/// Préfixes d'URL réécrits en `no-push://` pour tout push
/// (`url.no-push://.pushInsteadOf`) : un push vers une URL explicite échoue
/// aussi.
pub const PUSH_PREFIXES: &[&str] = &["https://", "http://", "ssh://", "git@", "git://", "file://"];

/// Liste blanche de l'environnement hérité par la session d'amélioration
/// (noms sans casse, `*` final = préfixe) : le système, les dossiers de
/// l'utilisateur et ce dont `claude.exe` a besoin pour joindre l'API. Le
/// CLI s'authentifie par `%USERPROFILE%\.claude\.credentials.json`
/// (`USERPROFILE`), ou par une clé `ANTHROPIC_*` ; `CLAUDE_CONFIG_DIR` et
/// `CLAUDE_CODE_GIT_BASH_PATH` s'ils sont réglés ; mandataire et autorités
/// de certification pour les réseaux d'entreprise. Rien d'autre : ni jeton
/// (`GH_TOKEN`…), ni agent SSH, ni variable git de l'utilisateur. La session
/// principale reçoit tout l'environnement (inchangé).
pub const IMPROVE_ENV_ALLOW: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramFiles*",
    "ProgramW6432",
    "CommonProgramFiles*",
    "CommonProgramW6432",
    "ProgramData",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_*",
    "OS",
    "ANTHROPIC_*",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_GIT_BASH_PATH",
    "NVM_HOME",
    "NVM_SYMLINK",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
];

/// Environnement de la session d'amélioration : git sans SSH ni invite ni
/// aide à la saisie d'identifiants, sans configuration système ni globale
/// (`global_config` : fichier vide hors dépôt ; le clone porte son identité
/// de commit en local, `repo::configure`), donc sans gestionnaire
/// d'identifiants ; sans hooks ; aucun transport permis
/// (`protocol.allow=never`, `ext::` compris) et tout push réécrit vers
/// `no-push://`.
pub fn improve_env(global_config: &Path) -> Vec<(String, String)> {
    let mut config: Vec<(&str, &str)> = vec![
        ("core.hooksPath", "NUL"),
        ("protocol.ext.allow", "never"),
        ("protocol.allow", "never"),
    ];
    config.extend(
        PUSH_PREFIXES
            .iter()
            .map(|p| ("url.no-push://.pushInsteadOf", *p)),
    );
    let mut env: Vec<(String, String)> = [
        ("GIT_SSH_COMMAND", "false"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GCM_INTERACTIVE", "never"),
        ("GIT_ASKPASS", ""),
        ("SSH_ASKPASS", ""),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if let Some(path) = crate::selfupdate::tools::subprocess_path() {
        // PATH enrichi (Node, cargo, registre) : le PATH hérité peut être
        // périmé ; appliqué après la liste blanche.
        env.push(("PATH".into(), path));
    }
    env.push((
        "GIT_CONFIG_GLOBAL".into(),
        global_config.to_string_lossy().into_owned(),
    ));
    env.push(("GIT_CONFIG_COUNT".into(), config.len().to_string()));
    for (i, (k, v)) in config.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), k.to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), v.to_string()));
    }
    env
}

/// Fichier de configuration git globale (vide) de la session
/// d'amélioration : `%LOCALAPPDATA%\orchestrator\improve.gitconfig`, remis à
/// vide avant chaque session.
pub fn empty_global_config() -> std::io::Result<PathBuf> {
    let dir = crate::selfupdate::data_dir().unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("improve.gitconfig");
    std::fs::write(&p, b"")?;
    Ok(p)
}

/// Options de lancement de la session d'amélioration (`model` : clé
/// `improve_model` de `config.json`, sinon le modèle par défaut du CLI ;
/// `global_config` : voir [`improve_env`]).
pub fn improve_launch_options(model: Option<String>, global_config: &Path) -> LaunchOptions {
    let mut extra_args = vec!["--disallowedTools".to_string()];
    extra_args.extend(DISALLOWED_TOOLS.iter().map(|t| t.to_string()));
    LaunchOptions {
        resume: None,
        model,
        extra_args,
        env: improve_env(global_config),
        env_allow: Some(IMPROVE_ENV_ALLOW.iter().map(|k| k.to_string()).collect()),
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

/// Fichier de contrat du build de l'interface (racine du build).
pub const UI_CONTRACT_FILE: &str = "contract.json";

/// La sortie de Vite dans `dir` porte l'empreinte `contract` (constante
/// `__CONTRACT__` inlinée dans les scripts) : l'interface a été construite
/// pour le contrat de cet exe.
pub fn ui_contract_matches(dir: &Path, contract: &str) -> bool {
    // `contract.json` (`{ contract, commit }`, écrit par le plugin de
    // `vite.config.ts`) d'abord ; sans lui (build plus ancien, illisible),
    // recherche du littéral dans les scripts.
    if let Some(found) = std::fs::read(dir.join(UI_CONTRACT_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("contract")?.as_str().map(str::to_string))
    {
        return found == contract;
    }
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
    /// Conversation du cycle en cours (clé stable = `id` du cycle, reportée
    /// dans `ImproveStatus::session`).
    session: Arc<Mutex<Option<SessionKey>>>,
    /// Avis du démarrage (worktrees `auto/…` non fusionnés laissés par une
    /// session précédente, app fermée pendant un cycle par exemple) : émis
    /// en tête de la conversation du prochain cycle (`notice`
    /// [`IMPROVE_UNMERGED`]).
    startup_notice: Arc<Mutex<Option<Vec<String>>>>,
}

/// Code de la `notice` (niveau `warn`) qui ouvre la conversation du premier
/// cycle après un démarrage où des worktrees `auto/…` non fusionnés ont été
/// trouvés. `detail` : `{"branches": ["auto/…", …]}`.
pub const IMPROVE_UNMERGED: &str = "improve.unmerged_worktrees";

/// Code de la `notice` (niveau `warn`) émise quand un tour en reprise ne
/// trouve pas le transcript de la session (premier tour mort avant de
/// l'écrire) : le tour repart par `--session-id`, consigne redonnée.
pub const IMPROVE_RESUME_FALLBACK: &str = "improve.resume_fallback";

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

/// [`PipelineHost`] de l'app.
pub struct AppHost<R: Runtime> {
    pub app: AppHandle<R>,
    pub model: Option<String>,
    /// Conversation du cycle en cours (clé stable : l'`id` du cycle), posée
    /// par `improve_start` ; partagée avec [`Improver`].
    session: Arc<Mutex<Option<SessionKey>>>,
}

/// Attente maximale de l'arrêt effectif de la session dédiée.
pub const IMPROVE_STOP_WAIT: Duration = super::commands::IMPROVE_STOP_WAIT;

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

    /// Clé de la conversation du cycle en cours : celle posée par
    /// `improve_start` (avant le lancement du cycle) ou par `run_turn`.
    /// Jamais déduite de `SelfUpdate::current`, qui peut encore désigner un
    /// cycle précédent (retour arrière qui n'a rien publié).
    fn current_key(&self) -> Option<SessionKey> {
        self.session.lock().unwrap().clone()
    }
}

/// Reprise d'un tour (J16) : `--resume <id>` seulement si le transcript de
/// la session existe sous `projects` (`~/.claude/projects`) pour `cwd` ; un
/// premier tour mort avant d'écrire son transcript ferait échouer
/// `--resume`. Sans dossier des projets connu : reprise tentée.
pub fn can_resume(projects: Option<&Path>, cwd: &Path, id: &str) -> bool {
    match projects {
        Some(root) => crate::sessions::paths::transcript_exists(root, &display_path(cwd), id),
        None => true,
    }
}

/// Message d'un tour en reprise impossible (transcript absent) : la session
/// repart de zéro avec la même clé (`--session-id`) ; la consigne complète
/// du premier tour est redonnée avant `message`.
pub fn fresh_turn_message(first_prompt: &str, message: &str) -> String {
    format!(
        "{first_prompt}\n\n---\n\n(La session précédente s'est arrêtée avant d'enregistrer \
         son historique : la consigne ci-dessus est redonnée. Message suivant :)\n\n{message}"
    )
}

impl<R: Runtime> PipelineHost for AppHost<R> {
    /// Premier tour (J14), par [`PipelineHost::run_turn`].
    fn run_session<'a>(
        &'a self,
        request: &'a ImproveRequest,
        cwd: &'a Path,
        prompt: &'a str,
    ) -> BoxFut<'a, Result<(), CoreError>> {
        Box::pin(async move { self.run_turn(request, cwd, prompt, false).await.map(drop) })
    }

    /// Jalon J16 : un processus `claude.exe` par tour, dans la conversation
    /// du cycle (journal à clé stable `request.id`) : `--session-id
    /// <request.id>` au premier tour, `--resume <request.id>` ensuite, mêmes
    /// options d'isolement ([`improve_launch_options`]). Reprise demandée
    /// mais transcript absent : nouveau départ par `--session-id`, consigne
    /// redonnée, `notice` `improve.resume_fallback`. Jamais
    /// `NotImplemented`. Rend la main après l'arrêt effectif du processus
    /// (`AppState::run_improve_turn`) ; `Ok(TurnEnd::Interrupted)` après
    /// `interrupt_turn`.
    fn run_turn<'a>(
        &'a self,
        request: &'a ImproveRequest,
        cwd: &'a Path,
        message: &'a str,
        resume: bool,
    ) -> BoxFut<'a, Result<TurnEnd, CoreError>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let key = SessionKey(request.id.clone());
            let exe =
                locate_claude().map_err(|tried| CoreError::ClaudeNotFound(tried.join(" ; ")))?;
            let global = empty_global_config()
                .map_err(|e| CoreError::Io(format!("configuration git de la session : {e}")))?;
            let conv = state.improve_conversation(&key, cwd);
            let changed = self.session.lock().unwrap().replace(key.clone()) != Some(key.clone());
            if changed {
                // L'onglet voit la conversation dès maintenant.
                let cur = self
                    .app
                    .state::<SelfUpdate>()
                    .current
                    .lock()
                    .unwrap()
                    .clone();
                if let Some(mut st) = cur.filter(|s| s.id == request.id) {
                    st.session = Some(key.clone());
                    publish(&self.app, st);
                }
            }
            let projects = crate::sessions::paths::projects_root();
            let mut message = std::borrow::Cow::Borrowed(message);
            let exists = can_resume(projects.as_deref(), cwd, &request.id);
            let resume = if !resume && projects.is_some() && exists {
                // Premier tour redemandé alors que la session a déjà un
                // transcript : `--session-id` serait refusé par le CLI.
                tracing::warn!(id = %request.id, "transcript présent : tour relancé par --resume");
                true
            } else if resume && !exists {
                tracing::warn!(id = %request.id, "transcript absent : tour relancé par --session-id");
                conv.notice(
                    NoticeLevel::Warn,
                    IMPROVE_RESUME_FALLBACK,
                    "historique de la session introuvable : nouvelle session, consigne redonnée",
                );
                message = std::borrow::Cow::Owned(fresh_turn_message(
                    &pipeline::build_prompt(cwd, request),
                    &message,
                ));
                false
            } else {
                resume
            };
            state
                .run_improve_turn(
                    &key,
                    &exe,
                    cwd,
                    &message,
                    resume,
                    improve_launch_options(self.model.clone(), &global),
                    SessionConfig::default(),
                    IMPROVE_SESSION_TIMEOUT,
                )
                .await
        })
    }

    fn interrupt_turn(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move {
            let key = self.current_key().ok_or(CoreError::NotRunning)?;
            self.app
                .state::<AppState>()
                .interrupt_improve_turn(&key)
                .await
        })
    }

    fn note(&self, level: NoticeLevel, code: &str, message: &str) {
        note_into(
            &self.app.state::<AppState>(),
            &self.session,
            level,
            code,
            message,
        );
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

    /// Arrêt effectif du processus du tour en cours (futur de `run_turn`
    /// abandonné : annulation) ; sans effet après un tour terminé, déjà
    /// arrêté par `run_turn`.
    fn session_stopped(&self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            if let Some(key) = self.current_key() {
                self.app.state::<AppState>().stop_improve_turn(&key).await;
            }
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

/// Démarrage (J16) : supprime les worktrees jetables des tests
/// (`<work>\.tests\*`) laissés par un arrêt de l'app pendant un cycle,
/// enregistrés ou non dans git. Aucun cycle ne tourne (appelé avant que le
/// clone soit déclaré prêt). La jonction `node_modules` est retirée d'abord
/// (jamais suivie). Renvoie le nombre de dossiers supprimés.
pub async fn clear_test_worktrees(clone: &Path, tests_root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(tests_root) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let nm = p.join("node_modules");
        if is_junction(&nm) {
            if std::fs::remove_dir(&nm).is_err() {
                let _ = std::fs::remove_file(&nm);
            }
            if std::fs::symlink_metadata(&nm).is_ok() {
                tracing::warn!("jonction {} non retirée : dossier gardé", nm.display());
                continue;
            }
        }
        let p_s = p.to_string_lossy().into_owned();
        let _ = crate::selfupdate::git::git(clone, &["worktree", "remove", "--force", &p_s]).await;
        if p.exists() {
            let _ = std::fs::remove_dir_all(&p);
        }
        if p.exists() {
            tracing::warn!("worktree de tests {} non supprimé", p.display());
        } else {
            tracing::info!("worktree de tests resté supprimé : {}", p.display());
            n += 1;
        }
    }
    let _ = crate::selfupdate::git::git(clone, &["worktree", "prune"]).await;
    n
}

/// `p` est une jonction (point de montage Windows) ou un lien.
fn is_junction(p: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(p)
            .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
    }
}

/// Démarrage (J16, décision « pas de reprise ») : branches des worktrees
/// `auto/…` du clone placés sous `work_root` et non fusionnées dans `main`
/// (cycle échoué, annulé, ou app fermée pendant un cycle). Gardés pour
/// inspection ; signalés par un avis.
pub async fn unmerged_worktrees(clone: &Path, work_root: &Path) -> Vec<String> {
    let Ok(list) = crate::selfupdate::git::git(clone, &["worktree", "list", "--porcelain"]).await
    else {
        return Vec::new();
    };
    let work = work_root.canonicalize().ok();
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
            continue;
        }
        let Some(b) = line.strip_prefix("branch refs/heads/") else {
            continue;
        };
        let under = path
            .as_ref()
            .and_then(|p| p.canonicalize().ok())
            .zip(work.as_ref())
            .is_some_and(|(p, w)| p.starts_with(w));
        if !b.starts_with(crate::selfupdate::BRANCH_PREFIX) || !under {
            continue;
        }
        let merged =
            crate::selfupdate::git::git_ok(clone, &["merge-base", "--is-ancestor", b, "main"])
                .await
                .unwrap_or(false);
        if !merged {
            out.push(b.to_string());
        }
    }
    out
}

/// Démarrage (`Builder::setup`) : cycle géré par Tauri, clone en tâche de
/// fond, puis nettoyage des worktrees fusionnés et, pour le propriétaire
/// (réglage `push` de `config.json`), vérification du droit d'écriture
/// avant d'activer l'envoi des branches `auto/…`.
pub fn setup<R: Runtime>(app: &AppHandle<R>) {
    let config = RepoConfig::load(install_dir().as_deref());
    let push_requested =
        crate::selfupdate::push::push_requested(crate::selfupdate::config_path().as_deref());
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
    let startup_notice: Arc<Mutex<Option<Vec<String>>>> = Arc::default();
    app.manage(Improver {
        pipeline: pipeline.clone(),
        running: Arc::default(),
        session,
        startup_notice: startup_notice.clone(),
    });

    let su = app.state::<SelfUpdate>();
    *su.repo.lock().unwrap() = RepoStatus::Cloning {
        path: config.dir.display().to_string(),
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Session dédiée interrompue par un arrêt brutal : `.git/config`
        // rendu inscriptible, verrou retiré (consigné au journal).
        crate::selfupdate::pipeline::recover_interrupted_session(&config.dir);
        let status = crate::selfupdate::ensure_clone(&config).await;
        match &status {
            RepoStatus::Disabled { reason } => {
                tracing::warn!("/ameliorer désactivé : {reason}")
            }
            s => tracing::info!("clone de l'app : {s:?}"),
        }
        let ready = matches!(status, RepoStatus::Ready { .. });
        if ready {
            // J16 : avant que `/ameliorer` soit permis (aucun cycle ne
            // tourne). Worktrees de tests laissés par un arrêt pendant un
            // cycle : supprimés. Worktrees `auto/…` non fusionnés (pas de
            // reprise) : signalés au journal et en tête de la conversation
            // du prochain cycle.
            let work = pipeline.work_root.clone();
            let n = clear_test_worktrees(&config.dir, &work.join(pipeline::TESTS_DIR)).await;
            if n > 0 {
                tracing::info!("{n} worktree(s) de tests resté(s) supprimé(s)");
            }
            let left = unmerged_worktrees(&config.dir, &work).await;
            if !left.is_empty() {
                tracing::warn!(
                    "amélioration(s) non fusionnée(s) gardée(s) pour inspection : {}",
                    left.join(", ")
                );
                *startup_notice.lock().unwrap() = Some(left);
            }
        }
        *app.state::<SelfUpdate>().repo.lock().unwrap() = status;
        if ready {
            let n = pipeline.cleanup_merged().await;
            if n > 0 {
                tracing::info!("{n} worktree(s) d'amélioration fusionné(s) supprimé(s)");
            }
            pipeline.enable_push(push_requested).await;
        } else if push_requested {
            pipeline.set_push(PushStatus::Disabled {
                reason: "clone de l'app indisponible".into(),
            });
        }
    });
}

fn ui_info(ui: &UiHost, su: &SelfUpdate, improver: Option<&Improver>) -> UiInfo {
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
        push: improver
            .map(|i| i.pipeline.push_status())
            .unwrap_or(PushStatus::Off),
    }
}

/// Interface affichée, état du clone et de l'amélioration en cours.
#[tauri::command(rename_all = "snake_case")]
pub fn get_ui_info<R: Runtime>(
    app: AppHandle<R>,
    ui: State<'_, UiHost>,
    su: State<'_, SelfUpdate>,
) -> UiInfo {
    let improver = app.try_state::<Improver>();
    ui_info(&ui, &su, improver.as_deref())
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
    state: State<'_, AppState>,
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
    // J16 : la conversation du cycle existe dès le départ (clé stable =
    // `id` du cycle), avant le lancement de la tâche du cycle : l'onglet
    // s'ouvre sur elle, les lignes d'état (`note`) y vont avant le premier
    // tour.
    let key = SessionKey(req.id.clone());
    let notice = improver.startup_notice.lock().unwrap().take();
    begin_conversation(
        &state,
        &improver.session,
        &key,
        &improver.pipeline.worktree_path(&req.branch),
        notice,
    );
    let status = ImproveStatus {
        id: req.id.clone(),
        branch: req.branch.clone(),
        request: req.text.clone(),
        step: ImproveStep::Preparing,
        session: Some(key),
        commit: None,
        detail: None,
        push: None,
        t: pipeline::now_ms(),
        attempt: 0,
        max_attempts: MAX_AUTO_ATTEMPTS,
        turn: 0,
        waiting: None,
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

/// Jalon J16 : message de l'utilisateur à la session de l'amélioration `id`
/// (onglet d'amélioration), par `Pipeline::send` (mis en file pendant un
/// tour, nouveau tour sinon).
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_send(
    improver: State<'_, Improver>,
    id: String,
    text: String,
) -> Result<(), CoreError> {
    improver.pipeline.send(&id, &text).await
}

/// Jalon J16 : « Valider » : contrôles et tests relancés pour l'amélioration
/// `id` en attente de l'utilisateur (`Pipeline::validate`).
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_validate(improver: State<'_, Improver>, id: String) -> Result<(), CoreError> {
    improver.pipeline.validate(&id).await
}

/// Jalon J16 : interrompt le tour en cours de la session de l'amélioration
/// `id` (Échap), par `Pipeline::interrupt` (puis
/// `PipelineHost::interrupt_turn`).
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_interrupt(improver: State<'_, Improver>, id: String) -> Result<(), CoreError> {
    improver.pipeline.interrupt(&id).await
}

/// Jalon J16 : l'onglet de l'amélioration `id`, terminée, est fermé : le
/// cœur libère le journal de sa conversation (plus rendu par
/// `attach_sessions`). Refusé tant que ce cycle tourne (annuler d'abord :
/// `improve_cancel`) ; sans effet si la conversation est déjà libérée.
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_dismiss(
    improver: State<'_, Improver>,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), CoreError> {
    let active = improver.session.lock().unwrap().clone();
    dismiss_allowed(improver.is_running(), active.as_ref(), &id)?;
    let freed = state.dismiss_improve(&SessionKey(id.clone())).await;
    tracing::info!(id, freed, "conversation d'amélioration libérée");
    Ok(())
}

/// `improve_dismiss` permis ? Comparaison à la conversation du cycle actif
/// (`Improver::session`, posée par `improve_start` avant toute
/// conversation), pas à `SelfUpdate::current` (qui peut manquer ou désigner
/// un autre cycle). Un cycle qui tourne sans conversation connue (retour
/// arrière, ou instant entre le début du cycle et la pose de la clé) :
/// refus par prudence.
pub fn dismiss_allowed(
    running: bool,
    active: Option<&SessionKey>,
    id: &str,
) -> Result<(), CoreError> {
    if running && active.is_none_or(|k| k.0 == id) {
        return Err(CoreError::InvalidArgument(
            "amélioration en cours : l'annuler avant de fermer son onglet".into(),
        ));
    }
    Ok(())
}

/// Ouvre la conversation du cycle `key` et en fait la conversation active
/// (`slot` : `Improver::session`, remis à `None` par `try_begin`) ; avis du
/// démarrage (`branches` non fusionnées) en tête.
pub fn begin_conversation(
    state: &AppState,
    slot: &Mutex<Option<SessionKey>>,
    key: &SessionKey,
    cwd: &Path,
    branches: Option<Vec<String>>,
) {
    let conv = state.improve_conversation(key, cwd);
    *slot.lock().unwrap() = Some(key.clone());
    if let Some(branches) = branches {
        conv.notice_with(
            NoticeLevel::Warn,
            IMPROVE_UNMERGED,
            &format!(
                "amélioration(s) précédente(s) non fusionnée(s), gardée(s) pour inspection \
                 (pas de reprise) : {}",
                branches.join(", ")
            ),
            Some(serde_json::json!({ "branches": branches })),
        );
    }
}

/// `PipelineHost::note` : ligne d'état dans la conversation active
/// (`slot`), seulement ; aucune conversation active (retour arrière) :
/// journal de l'app seul. Jamais dans le fil d'un cycle précédent.
pub fn note_into(
    state: &AppState,
    slot: &Mutex<Option<SessionKey>>,
    level: NoticeLevel,
    code: &str,
    message: &str,
) -> bool {
    tracing::info!(?level, code, "amélioration : {message}");
    let key = slot.lock().unwrap().clone();
    key.is_some_and(|k| state.improve_note(&k, level, code, message))
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
                                push: None,
                                detail: Some(format!(
                                    "interface précédente réactivée ; pas de git revert : {e}"
                                )),
                                t: pipeline::now_ms(),
                                attempt: 0,
                                max_attempts: MAX_AUTO_ATTEMPTS,
                                turn: 0,
                                waiting: None,
                            },
                        );
                        Err(e)
                    }
                    ok => ok,
                }
            });
        }
    }
    Ok(ui_info(&ui, &su, Some(&improver)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn improve_options_refuse_git_tools_and_isolate_git() {
        let g = std::path::Path::new("vide.gitconfig");
        let o = improve_launch_options(Some("haiku".into()), g);
        assert_eq!(o.extra_args[0], "--disallowedTools");
        for t in [
            "Bash(git push:*)",
            "Bash(git push*)",
            "Bash(git -C * push*)",
            "Bash(git --* push*)",
            "Bash(git -c*)",
            "Bash(*git.exe*)",
            "Bash(git credential*)",
            "Bash(*credential-manager*)",
            "Bash(git --config-env*)",
            "Bash(*GIT_CONFIG_PARAMETERS*)",
            "Bash(*GIT_ALLOW_PROTOCOL*)",
            "Bash(git remote:*)",
            "Bash(git config:*)",
            "Bash(git submodule:*)",
            "Bash(git worktree:*)",
            "Bash(git update-ref:*)",
            "Bash(git branch -f:*)",
            "Bash(gh:*)",
            "Bash(ssh:*)",
            "Bash(curl:*)",
        ] {
            assert!(o.extra_args.iter().any(|a| a == t), "{t}");
        }
        // La forme large, qui refusait `git commit -m "…push…"`, a disparu.
        assert!(!o.extra_args.iter().any(|a| a == "Bash(git * push*)"));
        // Liste blanche : système et authentification de l'API, rien d'autre.
        let allow = o.env_allow.as_deref().expect("liste blanche");
        use crate::process::spawn::env_allowed;
        for ok in [
            "PATH",
            "Path",
            "SystemRoot",
            "USERPROFILE",
            "ProgramFiles(x86)",
            "PROCESSOR_ARCHITECTURE",
            "ANTHROPIC_API_KEY",
            "TEMP",
        ] {
            assert!(env_allowed(ok, allow), "{ok}");
        }
        for no in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_SSH",
            "GIT_SSH_COMMAND",
            "SSH_AUTH_SOCK",
            "GIT_CONFIG_PARAMETERS",
            "GIT_ASKPASS",
            "AWS_SECRET_ACCESS_KEY",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "OSX",
        ] {
            assert!(!env_allowed(no, allow), "{no}");
        }
        let env: std::collections::HashMap<_, _> = o.env.iter().cloned().collect();
        assert_eq!(env["GIT_SSH_COMMAND"], "false");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(env["GIT_CONFIG_NOSYSTEM"], "1");
        assert_eq!(env["GIT_CONFIG_GLOBAL"], "vide.gitconfig");
        assert_eq!(env["GCM_INTERACTIVE"], "never");
        assert_eq!(env["GIT_ASKPASS"], "");
        assert_eq!(env["SSH_ASKPASS"], "");
        let n: usize = env["GIT_CONFIG_COUNT"].parse().unwrap();
        let pairs: Vec<(String, String)> = (0..n)
            .map(|i| {
                (
                    env[&format!("GIT_CONFIG_KEY_{i}")].clone(),
                    env[&format!("GIT_CONFIG_VALUE_{i}")].clone(),
                )
            })
            .collect();
        let has = |k: &str, v: &str| pairs.iter().any(|(a, b)| a == k && b == v);
        assert!(has("core.hooksPath", "NUL"));
        assert!(has("protocol.ext.allow", "never"));
        assert!(has("protocol.allow", "never"));
        for p in ["https://", "http://", "ssh://", "git@", "git://", "file://"] {
            assert!(has("url.no-push://.pushInsteadOf", p), "{p}");
        }
        // Les options passent la validation du lancement.
        let o = improve_launch_options(Some("haiku".into()), g);
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

    /// J16 (demande de A) : une note émise avant le premier tour
    /// (`improve.preparing`) arrive dans le fil du cycle en cours, dont la
    /// conversation est ouverte par `improve_start` avant le lancement du
    /// cycle ; jamais dans celui d'un cycle précédent, même entre la fin de
    /// l'un et le début de l'autre, ni pendant un retour arrière.
    #[tokio::test]
    async fn notes_go_to_the_active_cycle_only() {
        let state = AppState::new(std::env::temp_dir());
        let slot: Mutex<Option<SessionKey>> = Mutex::new(None);
        let tmp = std::env::temp_dir();
        let codes = |k: &SessionKey| -> Vec<String> {
            state
                .improve_get(k)
                .unwrap()
                .outlet
                .events()
                .into_iter()
                .filter_map(|e| match e.body {
                    super::super::events::EventBody::Notice { code, .. } => Some(code),
                    _ => None,
                })
                .collect()
        };
        // Cycle 1 : conversation ouverte, puis note de préparation.
        let k1 = SessionKey("11111111-2222-4333-8444-555555555551".into());
        begin_conversation(&state, &slot, &k1, &tmp, Some(vec!["auto/vieux".into()]));
        assert!(note_into(
            &state,
            &slot,
            NoticeLevel::Info,
            "improve.preparing",
            "1"
        ));
        assert_eq!(codes(&k1), vec![IMPROVE_UNMERGED, "improve.preparing"]);
        // Nouveau cycle (`try_begin` remet la clé à `None`) : rien ne part
        // vers le cycle 1 (retour arrière : journal de l'app seul).
        *slot.lock().unwrap() = None;
        assert!(!note_into(
            &state,
            &slot,
            NoticeLevel::Info,
            "improve.preparing",
            "x"
        ));
        let k2 = SessionKey("11111111-2222-4333-8444-555555555552".into());
        begin_conversation(&state, &slot, &k2, &tmp, None);
        assert!(note_into(
            &state,
            &slot,
            NoticeLevel::Info,
            "improve.preparing",
            "2"
        ));
        assert_eq!(codes(&k2), vec!["improve.preparing"]);
        assert_eq!(codes(&k1).len(), 2, "cycle 1 inchangé");
        // Conversation libérée : la note ne recrée rien.
        assert!(state.dismiss_improve(&k2).await);
        assert!(!note_into(
            &state,
            &slot,
            NoticeLevel::Info,
            "improve.preparing",
            "3"
        ));
        assert!(state.improve_get(&k2).is_none());
    }

    /// `improve_dismiss` comparé au cycle actif (`Improver::session`).
    #[test]
    fn dismiss_refused_for_the_running_cycle_only() {
        let a = SessionKey("a".into());
        assert!(dismiss_allowed(false, None, "a").is_ok());
        assert!(dismiss_allowed(false, Some(&a), "a").is_ok(), "cycle fini");
        assert!(dismiss_allowed(true, Some(&a), "a").is_err());
        assert!(
            dismiss_allowed(true, Some(&a), "b").is_ok(),
            "ancien onglet"
        );
        assert!(
            dismiss_allowed(true, None, "a").is_err(),
            "cycle sans clé : prudence"
        );
    }

    /// Point 5 : reprise sans transcript → nouveau départ, consigne redonnée.
    #[test]
    fn resume_falls_back_without_transcript() {
        let root = std::env::temp_dir().join(format!("orch-j16-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = std::path::Path::new(r"C:\x\orchestrator\work\auto\demo");
        let id = "11111111-2222-4333-8444-555555555555";
        assert!(
            can_resume(None, cwd, id),
            "racine inconnue : reprise tentée"
        );
        assert!(!can_resume(Some(&root), cwd, id));
        let dir = root.join(crate::sessions::paths::encode_project_dir(
            r"C:\x\orchestrator\work\auto\demo",
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.jsonl")), "{}\n").unwrap();
        assert!(can_resume(Some(&root), cwd, id));
        let m = fresh_turn_message("CONSIGNE", "corrige");
        assert!(m.starts_with("CONSIGNE") && m.ends_with("corrige"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn contract_file_is_read_first() {
        let d = std::env::temp_dir().join(format!("orch-contract-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("assets")).unwrap();
        // Scripts avec l'empreinte A, contract.json avec B : le fichier gagne.
        std::fs::write(d.join("assets/i.js"), "Ze=`aaaaaaaaaaaaaaaa`").unwrap();
        std::fs::write(
            d.join("contract.json"),
            r#"{"contract":"bbbbbbbbbbbbbbbb","commit":"1234567"}"#,
        )
        .unwrap();
        assert!(ui_contract_matches(&d, "bbbbbbbbbbbbbbbb"));
        assert!(!ui_contract_matches(&d, "aaaaaaaaaaaaaaaa"));
        // Fichier illisible : repli sur les scripts.
        std::fs::write(d.join("contract.json"), "pas du json").unwrap();
        assert!(ui_contract_matches(&d, "aaaaaaaaaaaaaaaa"));
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
