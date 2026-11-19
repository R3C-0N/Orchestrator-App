//! Cycle d'une amélioration (J14, décision 17) : worktree `auto/…` depuis le
//! `main` du clone, session dédiée (par [`PipelineHost`]), contrôles
//! déterministes, tests dans un worktree jetable au harnais restauré,
//! détection « relance requise » (décision 18), build de l'interface,
//! bascule, fusion `--ff-only`. Les commandes npm/node/cargo passent par
//! [`Toolchain`] (simulé dans les tests).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::feedback;
use super::git::{self, git, git_ok, resolve};
use super::repo::NO_PUSH_URL;
use super::{
    push, repo, BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, StatusSink,
    TurnEnd, ALLOWED_DIRS, BRANCH_PREFIX, MAX_AUTO_ATTEMPTS,
};
use crate::api::events::NoticeLevel;
use crate::api::types::{
    CoreError, ImproveStatus, ImproveStep, ImproveWaiting, PushResult, PushStatus,
};

/// Prompt versionné de la session dédiée, relu dans le worktree.
pub const PROMPT_FILE: &str = "tools/self-improve/prompt.md";
/// Config Playwright du sous-ensemble e2e de démarrage (port dédié).
pub const E2E_CONFIG: &str = "tools/self-improve/playwright.startup.config.ts";
/// Variable d'environnement du port du serveur Vite des e2e du cycle.
pub const E2E_PORT_ENV: &str = "ORCH_E2E_PORT";
/// Variable d'environnement du hash passé au build de l'interface.
pub const UI_HASH_ENV: &str = "ORCH_UI_HASH";
/// Prompt de repli (version compilée), si le fichier manque dans le clone.
const PROMPT_FALLBACK: &str = include_str!("../../../tools/self-improve/prompt.md");

/// Harnais entièrement protégé : aucune modification, ni ajout, ni
/// suppression par une amélioration (refus du cycle).
/// `src-tauri/.cargo/` et `src-tauri/build.rs` en font partie : des
/// `rustflags` ou un script de build pourraient empêcher les tests restaurés
/// de s'exécuter.
pub const PROTECTED_TREES: &[&str] = &[
    "tools/self-improve/",
    "src-tauri/src/selfupdate/",
    "src-tauri/.cargo/",
];
/// Fichiers protégés.
pub const PROTECTED_FILES: &[&str] = &["tools/contract-fingerprint.mjs", "src-tauri/build.rs"];
/// Tests existants protégés (modification ou suppression refusée) ; un
/// nouveau fichier de test y est permis et s'exécute en plus.
pub const PROTECTED_EXISTING: &[&str] = &["e2e/", "src-tauri/tests/"];
/// Chemins du harnais restaurés depuis `base` dans le worktree des tests.
pub const HARNESS_PATHS: &[&str] = &[
    "e2e",
    "tools/self-improve",
    "tools/contract-fingerprint.mjs",
    "src-tauri/src/selfupdate",
    "src-tauri/tests",
    "src-tauri/.cargo",
    "src-tauri/build.rs",
];
/// Marqueurs de tests désactivés ou isolés, cherchés dans les lignes ajoutées
/// des seuls fichiers de code (tests et code applicatif) : JS/TS pour
/// [`JS_EXTENSIONS`], Rust pour les `.rs`, manifeste pour les `Cargo.toml`.
/// La documentation et les autres fichiers ne sont jamais concernés (la spec
/// et le prompt citent ces marqueurs).
///
/// JS/TS : méthodes refusées en `.nom(`, `.nom (`, `` .nom` ``, `.nom.each`
/// (et autres chaînes de [`JS_CHAINS`]) ou `["nom"]` / `['nom']`.
pub const JS_SKIP_METHODS: &[&str] = &["skip", "only", "fixme", "todo", "skipIf", "runIf", "fails"];
/// Suites possibles d'une méthode de [`JS_SKIP_METHODS`] (`it.skip.each`).
pub const JS_CHAINS: &[&str] = &[
    "each",
    "concurrent",
    "sequential",
    "shuffle",
    "for",
    "skip",
    "only",
    "todo",
    "fails",
    "skipIf",
    "runIf",
];
/// Fonctions JS refusées en `nom(` (limite de mot : `exit(` passe).
pub const JS_SKIP_FUNCTIONS: &[&str] = &["xit", "xtest", "xdescribe"];
/// Sous-chaînes JS refusées telles quelles.
pub const JS_SKIP_SUBSTRINGS: &[&str] = &["test.fail", "describe.skip"];
/// Extensions des fichiers JS/TS concernés.
pub const JS_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "svelte",
];
/// Rust (`.rs`), cherchés dans la ligne sans espaces ; s'y ajoute
/// `cfg_attr(…, ignore)`.
pub const RUST_SKIP_MARKERS: &[&str] = &[
    "#[ignore",
    "cfg(any())",
    "cfg(FALSE)",
    "cfg(false)",
    "cfg(not(all()))",
];
/// `Cargo.toml`, cherchés dans la ligne sans espaces, en minuscules : ils
/// retirent des cibles de test ou leur harnais.
pub const CARGO_SKIP_MARKERS: &[&str] = &[
    "autotests",
    "autobins",
    "autobenches",
    "test=false",
    "harness",
    "doctest",
];
/// Verrou posé dans le dossier git commun du clone (`.git\improve-active`)
/// pendant la session dédiée : resté après un arrêt brutal, il signale au
/// démarrage suivant que `.git/config` est peut-être encore en lecture seule
/// ([`recover_interrupted_session`]).
pub const IMPROVE_LOCK: &str = "improve-active";

/// Démarrage : une session dédiée a été interrompue (verrou
/// [`IMPROVE_LOCK`] resté dans `.git` du clone) ; l'attribut lecture seule
/// de `.git/config` est retiré, le verrou supprimé, et c'est consigné dans le
/// journal. Renvoie vrai si un verrou a été trouvé.
pub fn recover_interrupted_session(clone: &Path) -> bool {
    let common = clone.join(".git");
    let lock = common.join(IMPROVE_LOCK);
    if !lock.is_file() {
        return false;
    }
    let config = common.join("config");
    if let Ok(m) = std::fs::metadata(&config) {
        let mut perm = m.permissions();
        if perm.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            if let Err(e) = std::fs::set_permissions(&config, perm) {
                tracing::error!("{} : lecture seule non retirée : {e}", config.display());
            }
        }
    }
    let _ = std::fs::remove_file(&lock);
    tracing::warn!(
        "session d'amélioration interrompue (arrêt brutal) : verrou {} retiré, \
         .git/config rendu inscriptible",
        lock.display()
    );
    true
}

/// Verrou [`IMPROVE_LOCK`], supprimé à la destruction.
struct ActiveLock(PathBuf);

impl ActiveLock {
    fn create(common: &Path) -> std::io::Result<ActiveLock> {
        let p = common.join(IMPROVE_LOCK);
        std::fs::write(&p, format!("{}\n", std::process::id()))?;
        Ok(ActiveLock(p))
    }
}

impl Drop for ActiveLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Dossier (sous `work_root`) des worktrees jetables des tests.
pub const TESTS_DIR: &str = ".tests";

/// Commandes de dépendances, tests, build et empreinte, lancées par le cycle.
/// `Err` : message lisible (fin de la sortie de la commande en échec).
pub trait Toolchain: Send + Sync {
    /// Vérifications avant le cycle (port des e2e libre…). Par défaut : rien.
    fn preflight(&self) -> BoxFut<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    /// `npm ci` dans `dir`.
    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>>;
    /// Tests obligatoires dans `tree` (worktree jetable, harnais restauré) ;
    /// `rust` : `src-tauri/` touché (`cargo test` en plus).
    fn test<'a>(&'a self, tree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>>;
    /// Build de l'interface dans `out` (dossier `<hash>.tmp`), étiqueté `hash`.
    fn build_ui<'a>(
        &'a self,
        worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>>;
    /// Empreinte du contrat de `tree` (`tools/contract-fingerprint.mjs`).
    fn contract<'a>(&'a self, tree: &'a Path) -> BoxFut<'a, Result<String, String>>;
}

/// Le port TCP local est libre (ni IPv4 ni IPv6 en écoute).
pub fn port_free(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port));
    if matches!(&v4, Err(e) if e.kind() == std::io::ErrorKind::AddrInUse) {
        return false;
    }
    let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port));
    !matches!(&v6, Err(e) if e.kind() == std::io::ErrorKind::AddrInUse)
        && (v4.is_ok() || v6.is_ok())
}

/// Toolchain réelle : npm, node, Playwright, cargo.
#[derive(Debug, Clone)]
pub struct NpmToolchain {
    /// Port du serveur Vite des e2e de démarrage (distinct de 1420 et 1430).
    pub e2e_port: u16,
    /// `CARGO_TARGET_DIR` partagé entre les cycles.
    pub cargo_target_dir: Option<PathBuf>,
    /// Délai maximal de chaque commande.
    pub step_timeout: Duration,
}

impl Default for NpmToolchain {
    fn default() -> Self {
        NpmToolchain {
            e2e_port: 1431,
            cargo_target_dir: super::data_dir().map(|d| d.join("target")),
            step_timeout: Duration::from_secs(30 * 60),
        }
    }
}

impl NpmToolchain {
    async fn step(
        &self,
        dir: &Path,
        program: &str,
        args: &[&str],
        env: &[(&str, String)],
        what: &str,
    ) -> Result<String, String> {
        let mut c = git::command(super::tools::program(program));
        if let Some(path) = super::tools::subprocess_path() {
            // Node et cargo en tête : `npm.cmd` appelle `node` par le PATH.
            c.env("PATH", path);
        }
        c.current_dir(dir)
            .args(args)
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0");
        for (k, v) in env {
            c.env(k, v);
        }
        let out = git::run(c, self.step_timeout, what).await?;
        if out.ok {
            Ok(out.stdout)
        } else {
            // Fin de sortie longue : renvoyée à la session (jalon J16) ; le
            // statut n'en montre que `feedback::short_detail`.
            Err(format!(
                "{what} a échoué :\n{}",
                out.tail(feedback::MAX_LINES_PER_STEP)
            ))
        }
    }
}

impl Toolchain for NpmToolchain {
    fn preflight(&self) -> BoxFut<'_, Result<(), String>> {
        Box::pin(async move {
            if port_free(self.e2e_port) {
                Ok(())
            } else {
                Err(format!(
                    "le port {} des e2e du cycle est occupé (serveur Vite resté ouvert ?) : \
                     le libérer puis relancer",
                    self.e2e_port
                ))
            }
        })
    }

    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            self.step(
                dir,
                git::npm_program(),
                &["ci", "--no-audit", "--no-fund"],
                &[],
                "npm ci",
            )
            .await
            .map(drop)
        })
    }

    fn test<'a>(&'a self, tree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let npm = git::npm_program();
            self.step(tree, npm, &["run", "check"], &[], "npm run check")
                .await?;
            self.step(tree, npm, &["test"], &[], "npm test").await?;
            if !tree.join(E2E_CONFIG).is_file() {
                return Err(format!(
                    "{E2E_CONFIG} absent du clone : e2e de démarrage impossibles"
                ));
            }
            self.step(
                tree,
                "node",
                &[
                    "node_modules/@playwright/test/cli.js",
                    "test",
                    "-c",
                    E2E_CONFIG,
                ],
                &[(E2E_PORT_ENV, self.e2e_port.to_string())],
                "e2e de démarrage",
            )
            .await?;
            if rust {
                let mut env = Vec::new();
                if let Some(t) = &self.cargo_target_dir {
                    env.push(("CARGO_TARGET_DIR", t.to_string_lossy().into_owned()));
                }
                self.step(
                    &tree.join("src-tauri"),
                    "cargo",
                    &["test"],
                    &env,
                    "cargo test",
                )
                .await?;
            }
            Ok(())
        })
    }

    fn build_ui<'a>(
        &'a self,
        worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let out_s = out.to_string_lossy().into_owned();
            self.step(
                worktree,
                "node",
                &[
                    "node_modules/vite/bin/vite.js",
                    "build",
                    "--outDir",
                    &out_s,
                    "--emptyOutDir",
                ],
                &[(UI_HASH_ENV, hash.to_string())],
                "vite build",
            )
            .await
            .map(drop)
        })
    }

    fn contract<'a>(&'a self, tree: &'a Path) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            self.step(
                tree,
                "node",
                &["tools/contract-fingerprint.mjs"],
                &[],
                "empreinte du contrat",
            )
            .await
            .map(|s| s.trim().to_string())
        })
    }
}

/// Délai maximal d'un tour de la session dédiée (jalon J16) : au-delà, le
/// tour est abandonné (l'hôte arrête la session) et le cycle attend
/// l'utilisateur (`ImproveWaiting::SessionStopped`).
pub const TURN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Cycle en cours (un seul à la fois).
pub(super) struct Active {
    pub id: String,
    pub cancel: watch::Sender<bool>,
    /// Messages, « Valider » et interruption de l'onglet d'amélioration.
    pub(super) ctl: Arc<Control>,
}

/// Où en est le cycle, pour les commandes de l'onglet (jalon J16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Worktree en préparation : un message est mis en file.
    Preparing,
    /// Tour de la session : un message est mis en file et livré au tour
    /// suivant ; l'interruption est possible.
    Turn,
    /// Tour fini (arrêt de la session, garde) : un message est mis en file ;
    /// l'interruption est refusée (le tour n'est plus en cours).
    AfterTurn,
    /// Contrôles ou tests : un message les abandonne.
    Checking,
    /// Attente de l'utilisateur : message ou « Valider ».
    Awaiting,
    /// Build, bascule, fusion, ou retour arrière : plus de message.
    Closed,
}

struct ControlState {
    phase: Phase,
    queue: Vec<String>,
    validate: bool,
    interrupted: bool,
}

/// État partagé entre le cycle et les commandes `send`, `validate`,
/// `interrupt` ; `wake` réveille le cycle en contrôles ou en attente.
pub(super) struct Control {
    state: std::sync::Mutex<ControlState>,
    wake: tokio::sync::Notify,
}

impl Control {
    fn new() -> Self {
        Control {
            state: std::sync::Mutex::new(ControlState {
                phase: Phase::Preparing,
                queue: Vec::new(),
                validate: false,
                interrupted: false,
            }),
            wake: tokio::sync::Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ControlState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Messages en file, joints, retirés de la file.
    fn take_message(&self) -> Option<String> {
        let mut s = self.lock();
        if s.queue.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut s.queue).join("\n\n"))
        }
    }

    fn has_message(&self) -> bool {
        !self.lock().queue.is_empty()
    }

    /// Passe en `phase`, sauf si des messages attendent : ils sont alors
    /// rendus (et la phase ne change pas, un tour va partir).
    fn enter(&self, phase: Phase) -> Option<String> {
        let mut s = self.lock();
        if !s.queue.is_empty() {
            return Some(std::mem::take(&mut s.queue).join("\n\n"));
        }
        s.phase = phase;
        s.validate = false;
        None
    }

    fn begin_turn(&self) {
        let mut s = self.lock();
        s.phase = Phase::Turn;
        s.validate = false;
        s.interrupted = false;
    }

    /// Fin réelle du tour : plus d'interruption possible.
    fn end_turn(&self) {
        self.lock().phase = Phase::AfterTurn;
    }

    fn take_validate(&self) -> bool {
        std::mem::take(&mut self.lock().validate)
    }

    fn was_interrupted(&self) -> bool {
        self.lock().interrupted
    }
}

/// Échec corrigible (jalon J16) : renvoyé à la session pour correction.
struct Fault {
    step: ImproveStep,
    /// Raison (première ligne) et sortie de la commande en échec (fin
    /// gardée, `feedback::MAX_LINES_PER_STEP` lignes au plus).
    text: String,
}

impl Fault {
    fn new(step: ImproveStep, text: impl Into<String>) -> Self {
        Fault {
            step,
            text: text.into(),
        }
    }

    /// Libellé de l'étape dans le retour à la session.
    fn label(&self) -> &'static str {
        match self.step {
            ImproveStep::Checking => "contrôles",
            ImproveStep::Testing => "tests",
            ImproveStep::Building => "build de l'interface (vite build)",
            ImproveStep::Switching => "bascule de l'interface (signal « prêt »)",
            _ => "contrôles",
        }
    }

    /// Ligne d'état de l'échec.
    fn headline(&self) -> &'static str {
        match self.step {
            ImproveStep::Testing => "Tests en échec",
            ImproveStep::Building => "Build de l'interface en échec",
            ImproveStep::Switching => "Interface sans signal « prêt »",
            _ => "Contrôles en échec",
        }
    }

    fn stop(&self) -> Stop {
        Stop::Failed(self.step, feedback::short_detail(&self.text))
    }
}

/// Erreur des contrôles et des tests : corrigible ou non.
enum VErr {
    Fault(Fault),
    Fatal(ImproveStep, String),
}

/// Contrôles et tests passés.
struct Verified {
    files: Vec<String>,
    head: String,
    short: String,
    fingerprint: Result<String, String>,
}

/// Issue d'un passage contrôles → tests → build → bascule → fusion.
enum Verdict {
    Done(ImproveOutcome),
    /// Abandonné par un message de l'utilisateur (nouveau tour).
    Message(String),
    Fault(Fault),
}

/// Ce que l'utilisateur a fait pendant l'attente.
enum UserAction {
    Message(String),
    Validate,
}

/// Suite d'un tour.
enum Next {
    Wait(ImproveWaiting, Option<String>),
    Verify,
}

impl Ctx {
    /// Worktree de tests à supprimer plus tard.
    fn left(&self, p: PathBuf) {
        self.tests_left
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(p);
    }
}

/// Ce qui est fixe pendant un cycle.
struct Ctx {
    clone: PathBuf,
    wt: PathBuf,
    /// Base des worktrees jetables des tests : un par passage
    /// (`<base>-<n>`), pour qu'un dossier encore tenu par un processus en
    /// cours d'arrêt (tests abandonnés) ne bloque pas le passage suivant.
    tests_base: PathBuf,
    tests_run: std::sync::atomic::AtomicU32,
    /// Worktrees de tests pas encore supprimés (abandon, suppression ratée).
    tests_left: std::sync::Mutex<Vec<PathBuf>>,
    common: PathBuf,
    base: String,
    snapshot: Snapshot,
    redact: std::sync::Mutex<feedback::Redactions>,
}

/// Ce que le cycle fait après le worktree.
enum Mode {
    /// Session dédiée puis contrôles.
    Improve,
    /// `git revert` du commit (complet) à la place de la session.
    Revert(String),
}

/// Arrêt du cycle avant sa fin normale.
enum Stop {
    Failed(ImproveStep, String),
    Cancelled,
}

fn fail(step: ImproveStep) -> impl Fn(String) -> Stop {
    move |e| Stop::Failed(step, e)
}

/// Émet les transitions d'un cycle.
struct Reporter {
    status: ImproveStatus,
    sink: StatusSink,
    /// Lignes d'état `improve.*` de la conversation du cycle (`None` : retour
    /// arrière, sans conversation).
    host: Option<Arc<dyn PipelineHost>>,
}

impl Reporter {
    fn emit(&mut self, step: ImproveStep, detail: Option<String>) {
        self.status.step = step;
        self.status.detail = detail;
        if step != ImproveStep::AwaitingUser {
            self.status.waiting = None;
        }
        self.status.t = now_ms();
        (self.sink)(self.status.clone());
    }

    /// Ligne d'état dans la conversation du cycle ([`PipelineHost::note`]).
    fn note(&self, level: NoticeLevel, code: &str, message: &str) {
        if let Some(h) = &self.host {
            h.note(level, code, message);
        }
    }

    /// Réémet l'état courant (même étape), par exemple après l'envoi.
    fn resend(&mut self) {
        self.status.t = now_ms();
        (self.sink)(self.status.clone());
    }
}

/// Libère l'emplacement du cycle actif, même si le cycle est abandonné.
struct ActiveGuard<'a>(&'a std::sync::Mutex<Option<Active>>);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut a) = self.0.lock() {
            *a = None;
        }
    }
}

/// État du clone relevé avant la session, comparé après (contrôles C2, C5).
struct Snapshot {
    /// Contenu de `.git/config` (commun aux worktrees).
    config: Vec<u8>,
    /// Taille et date de modification de `.git/config` : une modification
    /// faite puis annulée pendant la session change au moins la date.
    config_meta: Option<(u64, SystemTime)>,
    /// Hooks présents (hors `.sample`).
    hooks: Vec<String>,
    /// SHA-256 de `node_modules/.package-lock.json` du clone.
    deps: Option<Vec<u8>>,
}

/// Attend `fut`, sauf annulation demandée avant sa fin (le futur est alors
/// abandonné : processus tués par leur Job Object, session arrêtée par
/// l'hôte).
async fn guarded<T>(
    cancel: &mut watch::Receiver<bool>,
    fut: impl Future<Output = T>,
) -> Result<T, Stop> {
    let cancelled = async {
        let fired = cancel.wait_for(|c| *c).await.is_ok();
        if !fired {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        biased;
        _ = cancelled => Err(Stop::Cancelled),
        v = fut => Ok(v),
    }
}

/// Comme [`guarded`], et abandonne aussi `fut` dès qu'un message de
/// l'utilisateur arrive (`Ok(None)`) : contrôles et tests abandonnés, leurs
/// processus tués par leur Job Object.
async fn abortable<T>(
    cancel: &mut watch::Receiver<bool>,
    ctl: &Control,
    fut: impl Future<Output = T>,
) -> Result<Option<T>, Stop> {
    tokio::pin!(fut);
    loop {
        let cancelled = async {
            if cancel.wait_for(|c| *c).await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            _ = cancelled => return Err(Stop::Cancelled),
            _ = ctl.wake.notified() => {
                if ctl.has_message() {
                    return Ok(None);
                }
            }
            v = &mut fut => return Ok(Some(v)),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `AAAAMMJJ-HHMM` (UTC) de `t`.
pub fn stamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Jours depuis 1970 -> date civile (algorithme de H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Sujet de branche : minuscules ASCII, accents retirés, mots séparés par
/// `-`, 5 mots et 40 caractères au plus (`amelioration` si vide).
pub fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let mapped: &str = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => "a",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'î' | 'ï' | 'í' | 'ì' => "i",
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => "o",
            'ù' | 'û' | 'ü' | 'ú' => "u",
            'ç' => "c",
            'ÿ' => "y",
            'ñ' => "n",
            'œ' => "oe",
            'æ' => "ae",
            c if c.is_ascii_alphanumeric() => {
                s.push(c);
                continue;
            }
            _ => "-",
        };
        s.push_str(mapped);
    }
    let words: Vec<&str> = s.split('-').filter(|w| !w.is_empty()).take(5).collect();
    let mut out = String::new();
    for w in words {
        if !out.is_empty() && out.len() + 1 + w.len() > 40 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w[..w.len().min(40)]);
    }
    if out.is_empty() {
        "amelioration".into()
    } else {
        out
    }
}

/// Prompt de la session : modèle (`prompt.md` du worktree, sinon la version
/// compilée) avec `{{BRANCHE}}`, `{{DOSSIERS}}`, `{{DOSSIER}}`, puis
/// `{{DEMANDE}}` en dernier (le texte de l'utilisateur n'est pas réinterprété).
pub fn build_prompt(worktree: &Path, request: &ImproveRequest) -> String {
    let template = std::fs::read_to_string(worktree.join(PROMPT_FILE))
        .unwrap_or_else(|_| PROMPT_FALLBACK.to_string());
    template
        .replace("{{BRANCHE}}", &request.branch)
        .replace("{{DOSSIERS}}", &ALLOWED_DIRS.join(", "))
        .replace("{{DOSSIER}}", &worktree.to_string_lossy())
        .replace("{{DEMANDE}}", request.text.trim())
}

/// Le fichier (chemin git, `/`) est dans un dossier permis (décision 17).
pub fn is_allowed(path: &str) -> bool {
    ALLOWED_DIRS.iter().any(|d| path.starts_with(d))
        && !path.split('/').any(|c| c == ".." || c == ".git")
}

/// Le changement `status` (lettre de `git diff --raw`) de `path` touche le
/// harnais protégé.
pub fn touches_harness(path: &str, status: char) -> bool {
    PROTECTED_TREES.iter().any(|d| path.starts_with(d))
        || PROTECTED_FILES.contains(&path)
        || (status != 'A' && PROTECTED_EXISTING.iter().any(|d| path.starts_with(d)))
}

/// Raisons d'une « relance requise » d'après les fichiers touchés
/// (décision 18) ; l'empreinte du contrat est comparée à part.
pub fn restart_reasons(files: &[String]) -> Vec<String> {
    let mut r = Vec::new();
    if files.iter().any(|f| f.starts_with("src-tauri/")) {
        r.push("cœur Rust (src-tauri/) modifié".to_string());
    }
    for name in ["package.json", "tauri.conf.json"] {
        if files.iter().any(|f| f.rsplit('/').next() == Some(name)) {
            r.push(format!("{name} modifié"));
        }
    }
    if files
        .iter()
        .any(|f| f.starts_with("src/lib/core/bindings/"))
    {
        r.push("liaisons du contrat (src/lib/core/bindings/) modifiées".to_string());
    }
    r
}

/// Famille de marqueurs cherchée dans un fichier.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CodeKind {
    None,
    Js,
    Rust,
    Cargo,
}

/// Famille de marqueurs de `file` d'après son nom.
fn code_kind(file: &str) -> CodeKind {
    let name = file.rsplit('/').next().unwrap_or(file);
    if name == "Cargo.toml" {
        return CodeKind::Cargo;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return CodeKind::None;
    };
    let ext = ext.to_ascii_lowercase();
    if ext == "rs" {
        CodeKind::Rust
    } else if JS_EXTENSIONS.contains(&ext.as_str()) {
        CodeKind::Js
    } else {
        CodeKind::None
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Premier marqueur JS/TS de `line` (voir [`JS_SKIP_METHODS`]).
fn js_marker(line: &str) -> Option<String> {
    if let Some(m) = JS_SKIP_SUBSTRINGS.iter().find(|m| line.contains(*m)) {
        return Some(m.to_string());
    }
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    for name in JS_SKIP_METHODS {
        let dotted = format!(".{name}");
        for (i, _) in line.match_indices(&dotted) {
            let rest = line[i + dotted.len()..].trim_start();
            if rest.starts_with('(') || rest.starts_with('`') {
                return Some(format!(".{name}("));
            }
            if let Some(after) = rest.strip_prefix('.') {
                let next: String = after.chars().take_while(|c| is_ident(*c)).collect();
                if JS_CHAINS.contains(&next.as_str()) {
                    return Some(format!(".{name}.{next}"));
                }
            }
        }
        for q in ['"', '\'', '`'] {
            if compact.contains(&format!("[{q}{name}{q}]")) {
                return Some(format!("[{q}{name}{q}]"));
            }
        }
    }
    for name in JS_SKIP_FUNCTIONS {
        for (i, _) in line.match_indices(name) {
            let before = line[..i].chars().next_back();
            if before.is_some_and(|c| is_ident(c) || c == '.') {
                continue;
            }
            if line[i + name.len()..].trim_start().starts_with('(') {
                return Some(format!("{name}("));
            }
        }
    }
    None
}

/// Premier marqueur Rust de `line` (ligne sans espaces).
fn rust_marker(line: &str) -> Option<String> {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(m) = RUST_SKIP_MARKERS.iter().find(|m| compact.contains(*m)) {
        return Some(m.to_string());
    }
    let attr = compact.find("cfg_attr(")?;
    compact[attr..]
        .contains(",ignore")
        .then(|| "cfg_attr(…, ignore)".to_string())
}

/// Premier marqueur de `Cargo.toml` de `line` (sans espaces, en minuscules).
fn cargo_marker(line: &str) -> Option<String> {
    let compact: String = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    CARGO_SKIP_MARKERS
        .iter()
        .find(|m| compact.contains(*m))
        .map(|m| m.to_string())
}

fn marker_in(kind: CodeKind, line: &str) -> Option<String> {
    match kind {
        CodeKind::None => None,
        CodeKind::Js => js_marker(line),
        CodeKind::Rust => rust_marker(line),
        CodeKind::Cargo => cargo_marker(line),
    }
}

/// Lignes ajoutées (diff `-U0`) d'un fichier de code contenant un marqueur
/// qui désactive ou isole des tests ([`code_kind`]).
pub fn skip_markers(diff: &str) -> Vec<String> {
    let mut file = String::new();
    let mut kind = CodeKind::None;
    // En-tête d'un fichier (de `diff --git` au premier `@@`) : seule zone où
    // `+++ ` nomme le fichier ; ailleurs c'est une ligne ajoutée qui commence
    // par `++ ` et ne doit pas changer le fichier courant.
    let mut header = false;
    let mut found = Vec::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            header = true;
            file.clear();
            kind = CodeKind::None;
        } else if line.starts_with("@@") {
            header = false;
        } else if let Some(f) = line.strip_prefix("+++ ").filter(|_| header) {
            // Chemin avec une espace : git ajoute une tabulation finale.
            let f = f.trim_end_matches('\t');
            // Chemin cité par git (caractères spéciaux) : `"b/…"`.
            let f = f
                .strip_prefix('"')
                .and_then(|f| f.strip_suffix('"'))
                .unwrap_or(f);
            file = f.strip_prefix("b/").unwrap_or(f).to_string();
            kind = code_kind(&file);
        } else if let Some(added) = line.strip_prefix('+').filter(|_| !header) {
            if let Some(m) = marker_in(kind, added) {
                found.push(format!("{file} : « {m} »"));
            }
        }
    }
    found
}

/// Jonction (Windows) ou lien symbolique `link` -> `target`.
pub async fn link_dir(link: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut c = git::command("cmd");
        c.raw_arg(format!(
            "/C mklink /J \"{}\" \"{}\"",
            link.display(),
            target.display()
        ));
        let out = git::run(c, Duration::from_secs(30), "mklink /J").await?;
        if out.ok && link.exists() {
            Ok(())
        } else {
            Err(format!("jonction node_modules : {}", out.tail(3)))
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| format!("lien node_modules : {e}"))
    }
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Taille et date de modification d'un fichier.
fn file_meta(p: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Fichier mis en lecture seule, attribut rétabli à la destruction.
pub struct ReadOnly {
    path: PathBuf,
    was_readonly: bool,
}

impl ReadOnly {
    pub fn set(path: &Path) -> std::io::Result<ReadOnly> {
        let mut perm = std::fs::metadata(path)?.permissions();
        let was_readonly = perm.readonly();
        if !was_readonly {
            perm.set_readonly(true);
            std::fs::set_permissions(path, perm)?;
        }
        Ok(ReadOnly {
            path: path.to_path_buf(),
            was_readonly,
        })
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        if self.was_readonly {
            return;
        }
        if let Ok(m) = std::fs::metadata(&self.path) {
            let mut perm = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            if let Err(e) = std::fs::set_permissions(&self.path, perm) {
                tracing::error!("{} : lecture seule non retirée : {e}", self.path.display());
            }
        }
    }
}

fn sha256_file(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok().map(|b| Sha256::digest(&b).to_vec())
}

/// Dossier git commun du clone (`.git`).
async fn common_dir(dir: &Path) -> Result<PathBuf, String> {
    let c = git(dir, &["rev-parse", "--git-common-dir"]).await?;
    let p = PathBuf::from(c.trim());
    Ok(if p.is_absolute() { p } else { dir.join(p) })
}

fn hooks_in(common: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(common.join("hooks"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.ends_with(".sample"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

impl Pipeline {
    /// Dossier du worktree de `branch` (`<work_root>\auto\…`).
    pub fn worktree_path(&self, branch: &str) -> PathBuf {
        branch
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .fold(self.work_root.clone(), |p, c| p.join(c))
    }

    /// Worktree jetable des tests de `branch`.
    fn tests_path(&self, branch: &str) -> PathBuf {
        let name: String = branch
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        self.work_root.join(TESTS_DIR).join(name)
    }

    fn is_busy(&self) -> bool {
        self.active.lock().map(|a| a.is_some()).unwrap_or(true)
    }

    pub(super) fn make_request(&self, text: &str) -> Result<ImproveRequest, CoreError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::InvalidArgument(
                "demande vide : /ameliorer <demande>".into(),
            ));
        }
        let base = format!("{BRANCH_PREFIX}{}-{}", stamp(SystemTime::now()), slug(text));
        Ok(ImproveRequest {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.to_string(),
            branch: self.unique_branch(&base),
        })
    }

    /// `base`, ou `base-2`, `base-3`… si la branche ou son dossier existent.
    fn unique_branch(&self, base: &str) -> String {
        let taken = |b: &str| {
            if self.worktree_path(b).exists() {
                return true;
            }
            std::process::Command::new("git")
                .arg("-C")
                .arg(&self.repo.dir)
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{b}"),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !taken(base) {
            return base.to_string();
        }
        (2..100)
            .map(|n| format!("{base}-{n}"))
            .find(|b| !taken(b))
            .unwrap_or_else(|| format!("{base}-{}", now_ms()))
    }

    pub(super) async fn run_cycle(
        &self,
        request: ImproveRequest,
        commit: Option<String>,
        sink: StatusSink,
    ) -> Result<ImproveOutcome, CoreError> {
        let (tx, mut rx) = watch::channel(false);
        let ctl = Arc::new(Control::new());
        {
            let mut a = self
                .active
                .lock()
                .map_err(|_| CoreError::Io("état du cycle illisible".into()))?;
            if let Some(cur) = a.as_ref() {
                return Err(CoreError::InvalidArgument(format!(
                    "une amélioration est déjà en cours ({})",
                    cur.id
                )));
            }
            *a = Some(Active {
                id: request.id.clone(),
                cancel: tx,
                ctl: ctl.clone(),
            });
        }
        let _guard = ActiveGuard(&self.active);
        let reverting = commit.is_some();
        let mode = match commit {
            Some(c) => Mode::Revert(c),
            None => Mode::Improve,
        };
        if reverting {
            // Pas de conversation : ni message, ni « Valider ».
            ctl.enter(Phase::Closed);
        }
        let mut rep = Reporter {
            status: ImproveStatus {
                id: request.id.clone(),
                branch: request.branch.clone(),
                request: request.text.clone(),
                step: ImproveStep::Preparing,
                session: None,
                commit: None,
                detail: None,
                push: None,
                t: now_ms(),
                attempt: 0,
                max_attempts: MAX_AUTO_ATTEMPTS,
                turn: 0,
                waiting: None,
            },
            sink,
            host: (!reverting).then(|| self.host.clone()),
        };
        let outcome = match self.cycle(&request, &mode, &mut rep, &mut rx, &ctl).await {
            Ok(o) => o,
            Err(Stop::Cancelled) => {
                rep.emit(
                    ImproveStep::Cancelled,
                    Some("annulée : branche laissée non fusionnée".into()),
                );
                rep.note(
                    NoticeLevel::Warn,
                    "improve.cancelled",
                    &format!(
                        "✗ Amélioration annulée : branche {} gardée non fusionnée",
                        request.branch
                    ),
                );
                ImproveOutcome::Cancelled
            }
            Err(Stop::Failed(step, reason)) => {
                let what = if reverting {
                    "retour arrière"
                } else {
                    "amélioration"
                };
                rep.emit(
                    ImproveStep::Failed,
                    Some(format!("{what} en échec ({step:?}) : {reason}")),
                );
                rep.note(
                    NoticeLevel::Error,
                    "improve.failed",
                    &format!(
                        "✗ Échec ({step:?}) : {} ; branche {} gardée non fusionnée",
                        reason.lines().next().unwrap_or(""),
                        request.branch
                    ),
                );
                ImproveOutcome::Failed { step, reason }
            }
        };
        // Fusion faite (y compris « relance requise » et retour arrière) :
        // envoi de la seule branche `auto/…`, par le pipeline, après l'arrêt
        // de la session et les contrôles. Réglage `push` demandé et pas de
        // refus définitif : envoi, ou mise en attente (sonde en cours ou
        // impossible). Un échec laisse la fusion acquise.
        if let ImproveOutcome::Merged { commit } | ImproveOutcome::RestartRequired { commit } =
            &outcome
        {
            if self.push_wanted() {
                rep.status.push = Some(PushResult::Pushing {
                    branch: request.branch.clone(),
                });
                rep.resend();
                let result = self.push_merged(&request.branch, commit).await;
                let pushed = matches!(result, PushResult::Pushed { .. });
                rep.status.push = Some(result);
                rep.resend();
                // Envoi réussi : les envois en attente (échecs précédents)
                // sont refaits, après l'affichage du résultat.
                if pushed {
                    self.retry_pending().await;
                }
            }
        }
        Ok(outcome)
    }

    /// Envoie `branch` (tête `commit`, fusionnée dans le `main` du clone)
    /// vers le dépôt canonique, après une nouvelle sonde (destination,
    /// clone, droit d'écriture ; refaite aussi quand la précédente était
    /// impossible ou en cours). Échec : branche mise en attente, sauf refus
    /// définitif (pas de droit, autre dépôt, réglage git douteux).
    async fn push_merged(&self, branch: &str, commit: &str) -> PushResult {
        let res = match self.probe(push::PUSH_TIMEOUT).await {
            PushStatus::Enabled { url } => {
                let _l = self.push_lock.lock().await;
                push::push_branch(
                    &self.repo.dir,
                    &url,
                    &self.push_origin,
                    &self.push_env(),
                    branch,
                    commit,
                    push::PUSH_TIMEOUT,
                )
                .await
            }
            PushStatus::Disabled { reason } if self.push_refused() => {
                tracing::warn!(branch, "envoi refusé : {reason}");
                return PushResult::Failed {
                    branch: branch.to_string(),
                    reason,
                };
            }
            PushStatus::Disabled { reason } => Err(reason),
            other => Err(format!("état d'envoi inattendu : {other:?}")),
        };
        match res {
            Ok(()) => {
                self.pending_remove(branch);
                tracing::info!(branch, "branche envoyée");
                PushResult::Pushed {
                    branch: branch.to_string(),
                }
            }
            Err(reason) => {
                self.pending_add(push::Pending {
                    branch: branch.to_string(),
                    commit: commit.to_string(),
                });
                tracing::warn!(branch, "envoi échoué (fusion locale acquise) : {reason}");
                PushResult::Failed {
                    branch: branch.to_string(),
                    reason,
                }
            }
        }
    }

    async fn cycle(
        &self,
        req: &ImproveRequest,
        mode: &Mode,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
        ctl: &Control,
    ) -> Result<ImproveOutcome, Stop> {
        use ImproveStep as S;
        let clone = self.repo.dir.clone();

        // --- Préparation : vérifications, worktree, branche, node_modules.
        rep.emit(S::Preparing, None);
        rep.note(
            NoticeLevel::Info,
            "improve.preparing",
            &format!("✻ Préparation de la branche {}", req.branch),
        );
        guarded(cancel, self.toolchain.preflight())
            .await?
            .map_err(fail(S::Preparing))?;
        if !repo::is_clone(&clone) {
            return Err(Stop::Failed(
                S::Preparing,
                format!("clone absent : {}", clone.display()),
            ));
        }
        let base = resolve(&clone, "refs/heads/main")
            .await
            .ok_or_else(|| Stop::Failed(S::Preparing, "branche main absente du clone".into()))?;
        let wt = self.worktree_path(&req.branch);
        if wt.exists() {
            return Err(Stop::Failed(
                S::Preparing,
                format!("le worktree {} existe déjà", wt.display()),
            ));
        }
        if let Some(parent) = wt.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Stop::Failed(S::Preparing, format!("{} : {e}", parent.display())))?;
        }
        let wt_s = wt.to_string_lossy().into_owned();
        guarded(
            cancel,
            git(
                &clone,
                &["worktree", "add", "-q", "-b", &req.branch, &wt_s, &base],
            ),
        )
        .await?
        .map_err(fail(S::Preparing))?;
        if let Mode::Revert(commit) = mode {
            guarded(cancel, git(&wt, &["revert", "--no-edit", commit]))
                .await?
                .map_err(fail(S::Preparing))?;
        }
        self.prepare_deps(&clone, &wt, cancel).await?;
        let common = common_dir(&clone).await.map_err(fail(S::Preparing))?;
        let snapshot = Snapshot {
            config: std::fs::read(common.join("config")).unwrap_or_default(),
            config_meta: file_meta(&common.join("config")),
            hooks: hooks_in(&common),
            deps: sha256_file(&clone.join("node_modules").join(".package-lock.json")),
        };
        let tests_base = self.tests_path(&req.branch);
        // Chemins locaux retirés du retour d'erreur (le plus précis d'abord ;
        // chaque worktree de tests y est ajouté à sa création).
        let mut redact = feedback::Redactions::new()
            .add(&wt, ".")
            .add(&clone, "<clone>");
        if let Some(d) = super::data_dir() {
            redact = redact.add(&d, "<app>");
        }
        if let Some(h) = super::home_dir() {
            redact = redact.add(&h, "~");
        }
        let ctx = Ctx {
            clone,
            wt,
            tests_base,
            tests_run: std::sync::atomic::AtomicU32::new(0),
            tests_left: std::sync::Mutex::new(Vec::new()),
            common,
            base,
            snapshot,
            redact: std::sync::Mutex::new(redact),
        };

        match mode {
            Mode::Improve => self.converse(req, &ctx, rep, cancel, ctl).await,
            // Retour arrière : pas de session, tout échec est définitif.
            Mode::Revert(_) => {
                let tests = self.next_tests_tree(&ctx).await;
                let res = guarded(cancel, self.verify(&ctx, &tests, rep)).await;
                self.drop_tests_tree(&ctx, &tests).await;
                let v = match res? {
                    Ok(v) => v,
                    Err(VErr::Fault(f)) => return Err(f.stop()),
                    Err(VErr::Fatal(step, reason)) => return Err(Stop::Failed(step, reason)),
                };
                match self.finish(req, mode, &ctx, v, rep, cancel).await? {
                    Ok(o) => Ok(o),
                    Err(f) => Err(f.stop()),
                }
            }
        }
    }

    /// Conversation du cycle (jalon J16) : tours de la session, garde après
    /// chacun, contrôles et tests quand la tête a avancé, corrections
    /// automatiques (au plus [`MAX_AUTO_ATTEMPTS`]), attente de
    /// l'utilisateur, jusqu'à la fusion.
    async fn converse(
        &self,
        req: &ImproveRequest,
        ctx: &Ctx,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
        ctl: &Control,
    ) -> Result<ImproveOutcome, Stop> {
        use ImproveStep as S;
        // `config.json` déjà changé depuis le lancement : échec immédiat.
        self.check_config_file()
            .map_err(|e| Stop::Failed(S::Preparing, e))?;
        let mut message = build_prompt(&ctx.wt, req);
        // `Some` : ce tour corrige cet échec (tour `Fixing`).
        let mut fixing: Option<Fault> = None;
        let mut resume = false;
        // Tête de la branche au début du tour : `base` au premier, puis celle
        // relevée après le tour précédent (le pipeline ne commite jamais).
        let mut head = Some(ctx.base.clone());
        loop {
            // --- Tour de la session.
            let before = head.clone();
            ctl.begin_turn();
            match &fixing {
                Some(f) => {
                    rep.emit(
                        S::Fixing,
                        Some(f.text.lines().next().unwrap_or("").to_string()),
                    );
                    rep.note(
                        NoticeLevel::Info,
                        "improve.fixing",
                        &format!(
                            "✻ Correction automatique (tentative {}/{})",
                            rep.status.attempt, rep.status.max_attempts
                        ),
                    );
                }
                None => {
                    // Message de l'utilisateur (ou demande initiale).
                    rep.status.attempt = 0;
                    rep.emit(S::Working, None);
                }
            }
            let end = self
                .one_turn(req, ctx, &message, resume, cancel, ctl)
                .await?;
            rep.status.turn += 1;

            // --- Garde de sécurité, après chaque tour : une violation arrête
            // le cycle, rien n'est renvoyé à la session.
            if let Err(e) = self.guard(ctx, &req.branch).await {
                let reason = match &end {
                    Err(se) => format!("{e} ; session : {se}"),
                    Ok(_) => e,
                };
                return Err(Stop::Failed(S::Checking, reason));
            }
            rep.note(
                NoticeLevel::Info,
                "improve.guard",
                &format!("✓ Garde de sécurité (tour {})", rep.status.turn),
            );
            // Hôte sans reprise de la session (implémentation par défaut de
            // `run_turn`) : comportement J14, l'échec corrigible est définitif.
            if resume {
                if let Err(CoreError::NotImplemented(m)) = &end {
                    return Err(match &fixing {
                        Some(f) => f.stop(),
                        None => Stop::Failed(S::Working, m.clone()),
                    });
                }
            }
            resume = true;
            let after = resolve(&ctx.wt, "HEAD").await;
            head = after.clone();

            // --- Suite : message en file, attente ou contrôles.
            if let Some(m) = ctl.take_message() {
                message = m;
                fixing = None;
                continue;
            }
            let mut next = if ctl.was_interrupted() || matches!(end, Ok(TurnEnd::Interrupted)) {
                Next::Wait(ImproveWaiting::Interrupted, Some("tour interrompu".into()))
            } else if let Err(e) = &end {
                Next::Wait(
                    ImproveWaiting::SessionStopped,
                    Some(format!(
                        "la session s'est arrêtée avant la fin de son tour : {e}"
                    )),
                )
            } else if after == before {
                Next::Wait(
                    ImproveWaiting::NoNewCommit,
                    Some("tour terminé sans nouveau commit : rien à tester".into()),
                )
            } else {
                Next::Verify
            };
            let (m, f) = loop {
                match next {
                    Next::Wait(why, detail) => {
                        match self.await_user(why, detail, rep, cancel, ctl).await? {
                            UserAction::Message(m) => break (m, None),
                            UserAction::Validate => next = Next::Verify,
                        }
                    }
                    Next::Verify => {
                        match self
                            .verify_cycle(req, &Mode::Improve, ctx, rep, cancel, ctl)
                            .await?
                        {
                            Verdict::Done(o) => return Ok(o),
                            Verdict::Message(m) => {
                                rep.note(
                                    NoticeLevel::Info,
                                    "improve.aborted",
                                    "✻ Contrôles abandonnés : nouveau message",
                                );
                                break (m, None);
                            }
                            Verdict::Fault(f) => {
                                let max = rep.status.max_attempts;
                                if rep.status.attempt >= max {
                                    rep.note(
                                        NoticeLevel::Warn,
                                        "improve.fault",
                                        &format!(
                                            "✗ {} : {max} corrections automatiques sans succès, \
                                             à vous de reprendre",
                                            f.headline()
                                        ),
                                    );
                                    next = Next::Wait(
                                        ImproveWaiting::AttemptsExhausted,
                                        Some(feedback::short_detail(&f.text)),
                                    );
                                    continue;
                                }
                                rep.status.attempt += 1;
                                let n = rep.status.attempt;
                                rep.note(
                                    NoticeLevel::Warn,
                                    "improve.fault",
                                    &format!(
                                        "✗ {} (tentative {n}/{max}) : erreur renvoyée à la session",
                                        f.headline()
                                    ),
                                );
                                let fb = feedback::format_feedback(
                                    n,
                                    max,
                                    f.label(),
                                    &f.text,
                                    &ctx.redact.lock().unwrap_or_else(|e| e.into_inner()),
                                );
                                break (fb, Some(f));
                            }
                        }
                    }
                }
            };
            message = m;
            fixing = f;
        }
    }

    /// Un tour : verrou [`IMPROVE_LOCK`] et `.git/config` en lecture seule
    /// pendant le tour, arrêt effectif de la session attendu avant de les
    /// lever. Délai : [`TURN_TIMEOUT`]. `Err` : annulation (fichiers de l'app
    /// rétablis).
    async fn one_turn(
        &self,
        req: &ImproveRequest,
        ctx: &Ctx,
        message: &str,
        resume: bool,
        cancel: &mut watch::Receiver<bool>,
        ctl: &Control,
    ) -> Result<Result<TurnEnd, CoreError>, Stop> {
        use ImproveStep as S;
        // Verrou posé avant : un arrêt brutal pendant le tour laisse de quoi
        // rétablir l'attribut au démarrage suivant.
        let lock = ActiveLock::create(&ctx.common)
            .map_err(|e| Stop::Failed(S::Working, format!("verrou {IMPROVE_LOCK} : {e}")))?;
        let ro = ReadOnly::set(&ctx.common.join("config"))
            .map_err(|e| Stop::Failed(S::Working, format!(".git/config en lecture seule : {e}")))?;
        let turn = async {
            match tokio::time::timeout(
                TURN_TIMEOUT,
                self.host.run_turn(req, &ctx.wt, message, resume),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err(CoreError::Timeout(format!(
                    "tour de la session ({} min)",
                    TURN_TIMEOUT.as_secs() / 60
                ))),
            }
        };
        let done = guarded(cancel, turn).await;
        ctl.end_turn();
        // Arrêt effectif de la session (fin, délai ou annulation) avant de
        // rendre `.git/config` inscriptible et avant la garde.
        self.host.session_stopped().await;
        drop(ro);
        drop(lock);
        if done.is_err() {
            // Annulé : fichiers de l'app rétablis, annulation rapportée.
            if let Err(e) = self.check_app_files() {
                tracing::warn!("après la session annulée : {e}");
            }
        }
        done
    }

    /// Garde rejouée avant le build et avant la bascule : garde d'après tour,
    /// et la branche pointe toujours sur la tête testée `head` (un commit
    /// arrivé après les tests ne sera ni activé ni fusionné).
    async fn guard_tested(&self, ctx: &Ctx, branch: &str, head: &str) -> Result<(), String> {
        self.guard(ctx, branch).await?;
        let now = resolve(&ctx.wt, "HEAD").await;
        let tip = resolve(&ctx.clone, &format!("refs/heads/{branch}")).await;
        if now.as_deref() != Some(head) || tip.as_deref() != Some(head) {
            return Err(format!(
                "la branche {branch} a bougé après les tests : commit non testé,                  ni activé ni fusionné"
            ));
        }
        Ok(())
    }

    /// Worktree jetable des tests du passage suivant (`<base>-<n>`) ; les
    /// restes des passages précédents sont supprimés si possible d'abord.
    async fn next_tests_tree(&self, ctx: &Ctx) -> PathBuf {
        let left = std::mem::take(&mut *ctx.tests_left.lock().unwrap_or_else(|e| e.into_inner()));
        for t in left {
            if !self.remove_tree(&t, true).await {
                ctx.left(t);
            }
        }
        let n = ctx
            .tests_run
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let p = PathBuf::from(format!("{}-{n}", ctx.tests_base.to_string_lossy()));
        let mut r = ctx.redact.lock().unwrap_or_else(|e| e.into_inner());
        let cur = std::mem::take(&mut *r);
        *r = cur.add(&p, ".");
        p
    }

    /// Supprime le worktree de tests `tests` ; s'il résiste (processus en
    /// cours d'arrêt), il est retenté au passage suivant.
    async fn drop_tests_tree(&self, ctx: &Ctx, tests: &Path) {
        if !self.remove_tree(tests, true).await {
            tracing::warn!(
                "worktree de tests {} non supprimé : retenté plus tard",
                tests.display()
            );
            ctx.left(tests.to_path_buf());
        }
    }

    /// Garde de sécurité d'après tour : fichiers de l'app (rétablis),
    /// configuration git, hooks, `node_modules` du clone, `main`, branche.
    async fn guard(&self, ctx: &Ctx, branch: &str) -> Result<(), String> {
        self.check_app_files()?;
        guard_git(
            &ctx.clone,
            &ctx.wt,
            branch,
            &ctx.base,
            &ctx.common,
            &ctx.snapshot,
        )
        .await
    }

    /// Attente de l'utilisateur (`AwaitingUser`) : message ou « Valider ».
    async fn await_user(
        &self,
        why: ImproveWaiting,
        detail: Option<String>,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
        ctl: &Control,
    ) -> Result<UserAction, Stop> {
        if let Some(m) = ctl.enter(Phase::Awaiting) {
            return Ok(UserAction::Message(m));
        }
        rep.status.waiting = Some(why);
        rep.emit(ImproveStep::AwaitingUser, detail);
        let text = match why {
            ImproveWaiting::NoNewCommit => {
                "⏸ Tour terminé sans nouveau commit : répondez, ou « Valider »"
            }
            ImproveWaiting::Interrupted => "⏸ Tour interrompu : en attente de votre message",
            ImproveWaiting::AttemptsExhausted => {
                "⏸ Corrections automatiques épuisées : en attente de votre message"
            }
            ImproveWaiting::SessionStopped => {
                "⏸ La session s'est arrêtée avant la fin de son tour : en attente de votre message"
            }
        };
        rep.note(NoticeLevel::Info, "improve.awaiting", text);
        loop {
            guarded(cancel, ctl.wake.notified()).await?;
            if let Some(m) = ctl.take_message() {
                return Ok(UserAction::Message(m));
            }
            if ctl.take_validate() {
                return Ok(UserAction::Validate);
            }
        }
    }

    /// Contrôles puis tests (abandonnés par un message), puis build,
    /// bascule et fusion.
    async fn verify_cycle(
        &self,
        req: &ImproveRequest,
        mode: &Mode,
        ctx: &Ctx,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
        ctl: &Control,
    ) -> Result<Verdict, Stop> {
        if let Some(m) = ctl.enter(Phase::Checking) {
            return Ok(Verdict::Message(m));
        }
        let tests = self.next_tests_tree(ctx).await;
        let res = abortable(cancel, ctl, self.verify(ctx, &tests, rep)).await;
        // Abandon par un message : le tour part tout de suite ; le worktree
        // des tests (processus en cours d'arrêt) est supprimé au passage
        // suivant, ou par `cleanup_merged` et au démarrage.
        if matches!(res, Ok(None)) {
            ctx.left(tests);
        } else {
            self.drop_tests_tree(ctx, &tests).await;
        }
        let v = match res? {
            None => return Ok(Verdict::Message(ctl.take_message().unwrap_or_default())),
            Some(Err(VErr::Fault(f))) => return Ok(Verdict::Fault(f)),
            Some(Err(VErr::Fatal(step, reason))) => return Err(Stop::Failed(step, reason)),
            Some(Ok(v)) => v,
        };
        // Message arrivé juste à la fin des tests : il passe d'abord.
        if let Some(m) = ctl.enter(Phase::Closed) {
            return Ok(Verdict::Message(m));
        }
        Ok(match self.finish(req, mode, ctx, v, rep, cancel).await? {
            Ok(o) => Verdict::Done(o),
            Err(f) => Verdict::Fault(f),
        })
    }

    /// Contrôles corrigibles puis tests dans un worktree jetable au harnais
    /// restauré (supprimé par l'appelant).
    async fn verify(&self, ctx: &Ctx, tests: &Path, rep: &mut Reporter) -> Result<Verified, VErr> {
        use ImproveStep as S;
        rep.emit(S::Checking, None);
        rep.note(NoticeLevel::Info, "improve.checking", "✻ Contrôles…");
        let files = check_work(&ctx.wt, &ctx.base)
            .await
            .map_err(|e| VErr::Fault(Fault::new(S::Checking, e)))?;
        let head = resolve(&ctx.wt, "HEAD")
            .await
            .ok_or_else(|| VErr::Fatal(S::Checking, "tête de branche illisible".into()))?;
        let short = git::short(&ctx.wt, "HEAD")
            .await
            .map_err(|e| VErr::Fatal(S::Checking, e))?;
        rep.status.commit = Some(short.clone());
        rep.emit(S::Testing, None);
        rep.note(NoticeLevel::Info, "improve.testing", "✻ Tests…");
        let rust = files.iter().any(|f| f.starts_with("src-tauri/"));
        let fingerprint = self
            .run_tests(&ctx.clone, &ctx.wt, tests, &head, &ctx.base, rust)
            .await?;
        Ok(Verified {
            files,
            head,
            short,
            fingerprint,
        })
    }

    /// Après des contrôles et des tests verts : « relance requise »
    /// (fusion sans activation), sinon build, attente du repos, bascule et
    /// fusion. `Ok(Err(..))` : échec corrigible (build, interface sans
    /// signal « prêt »).
    async fn finish(
        &self,
        req: &ImproveRequest,
        mode: &Mode,
        ctx: &Ctx,
        v: Verified,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Result<ImproveOutcome, Fault>, Stop> {
        use ImproveStep as S;
        let clone = &ctx.clone;
        let (head, short) = (v.head, v.short);
        // Garde rejouée après les tests (ils partagent le `node_modules` du
        // clone et peuvent atteindre les fichiers de l'app) : violation
        // fatale, jamais renvoyée à la session.
        self.guard_tested(ctx, &req.branch, &head)
            .await
            .map_err(|e| Stop::Failed(S::Checking, e))?;
        let mut reasons = restart_reasons(&v.files);
        match v.fingerprint {
            Ok(fp) if fp.trim() == self.contract => {}
            Ok(fp) => reasons.push(format!(
                "empreinte du contrat {} différente de celle de l'exe ({})",
                fp.trim(),
                self.contract
            )),
            Err(e) => reasons.push(format!("empreinte du contrat illisible : {e}")),
        }

        // --- Cœur modifié : fusion sans activation (décision 18).
        if !reasons.is_empty() {
            merge(clone, &ctx.base, &req.branch, &head)
                .await
                .map_err(fail(S::Merged))?;
            self.remove_tree(&ctx.wt, false).await;
            rep.emit(
                S::RestartRequired,
                Some(format!("relance requise : {}", reasons.join(" ; "))),
            );
            rep.note(
                NoticeLevel::Info,
                "improve.restart_required",
                &format!(
                    "✓ Fusionné {short} : relance requise ({})",
                    reasons.join(" ; ")
                ),
            );
            return Ok(Ok(ImproveOutcome::RestartRequired { commit: head }));
        }

        // --- Build de l'interface étiquetée par le hash.
        rep.emit(S::Building, None);
        rep.note(
            NoticeLevel::Info,
            "improve.building",
            "✻ Construction de l'interface…",
        );
        std::fs::create_dir_all(&self.ui_root)
            .map_err(|e| Stop::Failed(S::Building, format!("{} : {e}", self.ui_root.display())))?;
        let out = self.ui_root.join(format!("{short}.tmp"));
        if out.exists() {
            std::fs::remove_dir_all(&out)
                .map_err(|e| Stop::Failed(S::Building, format!("{} : {e}", out.display())))?;
        }
        if let Err(e) = guarded(cancel, self.toolchain.build_ui(&ctx.wt, &short, &out)).await? {
            return Ok(Err(Fault::new(S::Building, e)));
        }
        if !out.join("index.html").is_file() {
            return Ok(Err(Fault::new(
                S::Building,
                "vite build n'a pas produit index.html",
            )));
        }

        // --- Bascule : attente du repos, puis rechargement et signal « prêt ».
        rep.emit(S::WaitingIdle, None);
        rep.note(
            NoticeLevel::Info,
            "improve.waiting_idle",
            "✻ Attente du repos de la session principale…",
        );
        guarded(cancel, self.host.wait_idle())
            .await?
            .map_err(|e| Stop::Failed(S::WaitingIdle, e.to_string()))?;
        // Conditions de la fusion revalidées avant d'activer quoi que ce soit.
        can_merge(clone, &ctx.base, &head)
            .await
            .map_err(|e| Stop::Failed(S::Switching, format!("fusion impossible : {e}")))?;
        // Garde rejouée juste avant la bascule (build et tests partagent le
        // `node_modules` du clone) : la branche doit toujours pointer sur la
        // tête testée.
        self.guard_tested(ctx, &req.branch, &head)
            .await
            .map_err(|e| Stop::Failed(S::Checking, e))?;
        // Plus d'annulation à partir d'ici : la bascule va au bout.
        rep.emit(S::Switching, None);
        rep.note(
            NoticeLevel::Info,
            "improve.switching",
            "✻ Bascule vers la nouvelle interface…",
        );
        let ready = self
            .host
            .switch_ui(&short, &out)
            .await
            .map_err(|e| Stop::Failed(S::Switching, e.to_string()))?;
        if !ready {
            return Ok(Err(Fault::new(
                S::Switching,
                "la nouvelle interface n'a pas signalé « prêt » à temps : version précédente \
                 rétablie, branche non fusionnée\nCause probable : une erreur au montage de \
                 l'interface (exception dans un composant, import manquant) ; l'app attend \
                 l'appel `ui_ready` après le montage.",
            )));
        }

        // --- Fusion fast-forward dans le main du clone.
        if let Err(e) = merge(clone, &ctx.base, &req.branch, &head).await {
            // Interface active mais pas fusionnée : retour à la précédente.
            let back = match self.host.rollback_ui().await {
                Ok(()) => "interface précédente rétablie".to_string(),
                Err(r) => format!("retour à l'interface précédente impossible : {r}"),
            };
            return Err(Stop::Failed(S::Merged, format!("{e} ; {back}")));
        }
        self.remove_tree(&ctx.wt, false).await;
        let (step, detail) = match mode {
            Mode::Improve => (S::Merged, format!("fusionnée dans main ({short})")),
            Mode::Revert(c) => (
                S::RolledBack,
                format!("{} annulé par {short}", &c[..c.len().min(10)]),
            ),
        };
        rep.emit(step, Some(detail));
        rep.note(
            NoticeLevel::Info,
            "improve.merged",
            &format!("✓ Fusionné {short}"),
        );
        Ok(Ok(ImproveOutcome::Merged { commit: head }))
    }

    /// Worktree jetable (détaché) sur `head`, harnais protégé restauré depuis
    /// `base`, puis tests et empreinte du contrat dedans. Renvoie l'empreinte
    /// (ou son erreur) si les tests passent. Abandon (annulation, message) :
    /// par l'appelant, qui supprime aussi le worktree.
    async fn run_tests(
        &self,
        clone: &Path,
        wt: &Path,
        tests: &Path,
        head: &str,
        base: &str,
        rust: bool,
    ) -> Result<Result<String, String>, VErr> {
        let fatal = |e: String| VErr::Fatal(ImproveStep::Testing, e);
        if tests.exists() {
            self.remove_tree(tests, true).await;
        }
        if let Some(parent) = tests.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| fatal(format!("{} : {e}", parent.display())))?;
        }
        let tests_s = tests.to_string_lossy().into_owned();
        git(
            clone,
            &["worktree", "add", "-q", "--detach", &tests_s, head],
        )
        .await
        .map_err(fatal)?;
        let mut restore: Vec<&str> = Vec::new();
        for p in HARNESS_PATHS {
            let listed = git(clone, &["ls-tree", "--name-only", base, "--", p])
                .await
                .map_err(fatal)?;
            if !listed.trim().is_empty() {
                restore.push(p);
            }
        }
        if !restore.is_empty() {
            let mut args = vec!["checkout", base, "--"];
            args.extend(restore.iter().copied());
            git(tests, &args)
                .await
                .map_err(|e| fatal(format!("restauration du harnais : {e}")))?;
        }
        let wt_nm = wt.join("node_modules");
        let target = if is_link(&wt_nm) {
            clone.join("node_modules")
        } else {
            wt_nm
        };
        if target.is_dir() {
            link_dir(&tests.join("node_modules"), &target)
                .await
                .map_err(fatal)?;
        }
        self.toolchain
            .test(tests, rust)
            .await
            .map_err(|e| VErr::Fault(Fault::new(ImproveStep::Testing, e)))?;
        Ok(self.toolchain.contract(tests).await)
    }

    /// `node_modules` du worktree : jonction vers celui du clone si
    /// `package-lock.json` est identique, sinon `npm ci`.
    async fn prepare_deps(
        &self,
        clone: &Path,
        wt: &Path,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), Stop> {
        let failed = fail(ImproveStep::Preparing);
        let clone_nm = clone.join("node_modules");
        if !clone_nm.is_dir() {
            guarded(cancel, self.toolchain.install_deps(clone))
                .await?
                .map_err(|e| failed(format!("dépendances du clone : {e}")))?;
        }
        let lock = |d: &Path| std::fs::read(d.join("package-lock.json")).ok();
        if clone_nm.is_dir() && lock(clone) == lock(wt) {
            link_dir(&wt.join("node_modules"), &clone_nm)
                .await
                .map_err(&failed)
        } else {
            guarded(cancel, self.toolchain.install_deps(wt))
                .await?
                .map_err(failed)
        }
    }

    /// Supprime le worktree `wt` ; la jonction `node_modules` est retirée
    /// d'abord (jamais de suppression à travers elle). Sans `force`, un
    /// worktree non propre est gardé. Renvoie vrai si le worktree a disparu.
    pub async fn remove_worktree(&self, wt: &Path) -> bool {
        self.remove_tree(wt, false).await
    }

    async fn remove_tree(&self, wt: &Path, force: bool) -> bool {
        let nm = wt.join("node_modules");
        if is_link(&nm) {
            if std::fs::remove_dir(&nm).is_err() {
                let _ = std::fs::remove_file(&nm);
            }
            if is_link(&nm) {
                tracing::warn!("jonction {} non retirée : worktree gardé", nm.display());
                return false;
            }
        }
        if !force {
            match git(wt, &["status", "--porcelain", "--untracked-files=all"]).await {
                Ok(s) if s.trim().is_empty() => {}
                _ => {
                    tracing::warn!("worktree {} non propre : gardé", wt.display());
                    return false;
                }
            }
        }
        let wt_s = wt.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&wt_s);
        let _ = git(&self.repo.dir, &args).await;
        if wt.exists() && force {
            let _ = std::fs::remove_dir_all(wt);
            let _ = git(&self.repo.dir, &["worktree", "prune"]).await;
        }
        !wt.exists()
    }

    /// Supprime les worktrees `auto/…` sous `work_root` dont la branche a au
    /// moins un commit, est fusionnée dans `main` et dont l'arbre est propre,
    /// ainsi que les worktrees jetables des tests. Ne fait rien pendant un
    /// cycle. Ceux des échecs restent pour inspection. Renvoie le nombre de
    /// worktrees `auto/…` supprimés.
    pub async fn cleanup_merged(&self) -> usize {
        if self.is_busy() {
            return 0;
        }
        let clone = &self.repo.dir;
        let Ok(list) = git(clone, &["worktree", "list", "--porcelain"]).await else {
            return 0;
        };
        let work = self.work_root.canonicalize().ok();
        let tests_root = self.work_root.join(TESTS_DIR).canonicalize().ok();
        let mut removed = 0;
        let mut path: Option<PathBuf> = None;
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
                continue;
            }
            if line.is_empty() {
                path = None;
                continue;
            }
            let Some(p) = path.clone() else { continue };
            let canon = p.canonicalize().ok();
            if line == "detached" {
                // Worktree jetable des tests resté après une annulation.
                let in_tests = canon
                    .as_ref()
                    .zip(tests_root.as_ref())
                    .is_some_and(|(p, t)| p.starts_with(t));
                if in_tests && !self.is_busy() {
                    self.remove_tree(&p, true).await;
                }
                continue;
            }
            let Some(b) = line.strip_prefix("branch refs/heads/") else {
                continue;
            };
            let under = canon
                .as_ref()
                .zip(work.as_ref())
                .is_some_and(|(p, w)| p.starts_with(w));
            if !b.starts_with(BRANCH_PREFIX) || !under || self.is_busy() {
                continue;
            }
            if branch_has_commits(clone, b).await
                && git_ok(clone, &["merge-base", "--is-ancestor", b, "main"])
                    .await
                    .unwrap_or(false)
                && self.remove_tree(&p, false).await
            {
                removed += 1;
            }
        }
        removed
    }

    pub(super) fn cancel_active(&self, id: &str) -> Result<(), CoreError> {
        let a = self
            .active
            .lock()
            .map_err(|_| CoreError::Io("état du cycle illisible".into()))?;
        match a.as_ref() {
            Some(cur) if cur.id == id => {
                let _ = cur.cancel.send(true);
                Ok(())
            }
            _ => Err(CoreError::UnknownRequest(format!(
                "aucune amélioration en cours d'identifiant {id}"
            ))),
        }
    }

    /// Commandes de l'onglet du cycle `id`.
    fn control(&self, id: &str) -> Result<Arc<Control>, CoreError> {
        let a = self
            .active
            .lock()
            .map_err(|_| CoreError::Io("état du cycle illisible".into()))?;
        match a.as_ref() {
            Some(cur) if cur.id == id => Ok(cur.ctl.clone()),
            _ => Err(CoreError::UnknownRequest(format!(
                "aucune amélioration en cours d'identifiant {id}"
            ))),
        }
    }

    /// Message de l'utilisateur pour le cycle `id` ([`Pipeline::send`]) :
    /// pendant un tour (ou la préparation), mis en file et livré au tour
    /// suivant ; pendant les contrôles ou les tests, ceux-ci sont abandonnés
    /// et un tour part ; en attente, un tour part. Refusé à partir du build
    /// et pour un retour arrière. Le tour livré remet `attempt` à 0.
    pub(super) fn send_active(&self, id: &str, text: &str) -> Result<(), CoreError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::InvalidArgument("message vide".into()));
        }
        let ctl = self.control(id)?;
        let wake = {
            let mut s = ctl.lock();
            if s.phase == Phase::Closed {
                return Err(CoreError::InvalidArgument(
                    "l'amélioration ne prend plus de message : contrôles et tests passés, \
                     construction, bascule ou fusion en cours"
                        .into(),
                ));
            }
            s.queue.push(text.to_string());
            matches!(s.phase, Phase::Checking | Phase::Awaiting)
        };
        if wake {
            ctl.wake.notify_one();
        }
        Ok(())
    }

    /// « Valider » ([`Pipeline::validate`]) : seulement en attente de
    /// l'utilisateur ; relance garde, contrôles et tests (sans remettre
    /// `attempt` à 0).
    pub(super) fn validate_active(&self, id: &str) -> Result<(), CoreError> {
        let ctl = self.control(id)?;
        {
            let mut s = ctl.lock();
            if s.phase != Phase::Awaiting {
                return Err(CoreError::InvalidArgument(
                    "« Valider » n'agit que quand l'amélioration attend l'utilisateur".into(),
                ));
            }
            s.validate = true;
        }
        ctl.wake.notify_one();
        Ok(())
    }

    /// Interruption du tour en cours ([`Pipeline::interrupt`]) : le cycle
    /// passe ensuite en attente (`Interrupted`), sans tests.
    pub(super) async fn interrupt_active(&self, id: &str) -> Result<(), CoreError> {
        let ctl = self.control(id)?;
        {
            let mut s = ctl.lock();
            if s.phase != Phase::Turn {
                return Err(CoreError::InvalidArgument(
                    "aucun tour de la session d'amélioration en cours".into(),
                ));
            }
            s.interrupted = true;
        }
        let r = self.host.interrupt_turn().await;
        if r.is_err() {
            ctl.lock().interrupted = false;
        }
        r
    }

    pub(super) async fn revert_request(
        &self,
        commit: &str,
    ) -> Result<(ImproveRequest, String), CoreError> {
        let clone = &self.repo.dir;
        let full = resolve(clone, commit)
            .await
            .ok_or_else(|| CoreError::InvalidArgument(format!("commit inconnu : {commit}")))?;
        if !git_ok(clone, &["merge-base", "--is-ancestor", &full, "main"])
            .await
            .unwrap_or(false)
        {
            return Err(CoreError::InvalidArgument(format!(
                "{commit} n'est pas dans le main du clone"
            )));
        }
        let subject = git(clone, &["log", "-1", "--format=%s", &full])
            .await
            .map_err(CoreError::Io)?;
        let short = git::short(clone, &full).await.map_err(CoreError::Io)?;
        let base = format!("{BRANCH_PREFIX}{}-revert-{short}", stamp(SystemTime::now()));
        Ok((
            ImproveRequest {
                id: uuid::Uuid::new_v4().to_string(),
                text: format!("Revert \"{subject}\""),
                branch: self.unique_branch(&base),
            },
            full,
        ))
    }
}

/// La branche a avancé depuis sa création (reflog : tête différente de
/// l'entrée la plus ancienne). Sans reflog : faux (rien n'est supprimé).
async fn branch_has_commits(clone: &Path, branch: &str) -> bool {
    let r = format!("refs/heads/{branch}");
    let Ok(log) = git(clone, &["log", "-g", "--format=%H", &r]).await else {
        return false;
    };
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match (lines.first(), lines.last()) {
        (Some(head), Some(created)) => head != created,
        _ => false,
    }
}

/// Garde de sécurité après chaque tour (jalon J16) : branche, `main`,
/// configuration git, hooks, remotes, `node_modules` du clone. Une violation
/// arrête le cycle ; sa raison n'est jamais renvoyée à la session.
async fn guard_git(
    clone: &Path,
    wt: &Path,
    branch: &str,
    base: &str,
    common: &Path,
    snapshot: &Snapshot,
) -> Result<(), String> {
    // Branche, main.
    let cur = git(wt, &["symbolic-ref", "--short", "HEAD"])
        .await
        .unwrap_or_default();
    if cur.trim() != branch {
        return Err(format!(
            "le worktree n'est plus sur la branche {branch} ({})",
            if cur.is_empty() {
                "HEAD détachée"
            } else {
                cur.trim()
            }
        ));
    }
    if resolve(clone, "refs/heads/main").await.as_deref() != Some(base) {
        return Err("main a été modifié pendant la session".into());
    }

    // Configuration git : rien d'ajouté (push, hooks, ssh…).
    if std::fs::read(common.join("config")).unwrap_or_default() != snapshot.config
        || file_meta(&common.join("config")) != snapshot.config_meta
    {
        return Err("la configuration git du clone (.git/config) a été modifiée".into());
    }
    if hooks_in(common) != snapshot.hooks {
        return Err("des hooks git ont été ajoutés au clone".into());
    }
    let gitdir = git(wt, &["rev-parse", "--absolute-git-dir"]).await?;
    if Path::new(gitdir.trim()).join("config.worktree").exists() {
        return Err("une configuration git propre au worktree a été ajoutée".into());
    }
    let push = git(clone, &["config", "--get", "remote.origin.pushurl"])
        .await
        .unwrap_or_default();
    if push.trim() != NO_PUSH_URL {
        return Err("le push du clone n'est plus désactivé".into());
    }
    let remotes = git(clone, &["remote"]).await?;
    if remotes.split_whitespace().collect::<Vec<_>>() != ["origin"] {
        return Err(format!(
            "dépôts distants inattendus : {}",
            remotes.split_whitespace().collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(sneaky) = push::sneaky_config(clone).await {
        return Err(format!("réglage git refusé dans le clone : {sneaky}"));
    }

    // node_modules partagé avec le clone.
    if sha256_file(&clone.join("node_modules").join(".package-lock.json")) != snapshot.deps {
        return Err(
            "les dépendances (node_modules) du clone ont été modifiées pendant la session".into(),
        );
    }
    Ok(())
}

/// Contrôles corrigibles (renvoyés à la session) : arbre propre, commits sur
/// la branche, fichiers dans le périmètre, harnais intact, pas de marqueur
/// de test désactivé. Renvoie les fichiers touchés par la branche.
async fn check_work(wt: &Path, base: &str) -> Result<Vec<String>, String> {
    let status = git(wt, &["status", "--porcelain", "--untracked-files=all"]).await?;
    if !status.trim().is_empty() {
        let first: Vec<&str> = status.lines().take(10).collect();
        return Err(format!(
            "arbre non propre (modifications non commitées) :\n{}",
            first.join("\n")
        ));
    }
    if !git_ok(wt, &["merge-base", "--is-ancestor", base, "HEAD"]).await? {
        return Err("la branche ne part plus du main du clone".into());
    }
    let range = format!("{base}..HEAD");
    let count: u32 = git(wt, &["rev-list", "--count", &range])
        .await?
        .trim()
        .parse()
        .unwrap_or(0);
    if count == 0 {
        return Err("aucun commit sur la branche".into());
    }

    // Fichiers : modes, périmètre, harnais.
    let raw = git(wt, &["diff", "--raw", "--no-renames", "-z", base, "HEAD"]).await?;
    let mut parts = raw.split('\0').filter(|s| !s.is_empty());
    let mut files = Vec::new();
    let mut refused = Vec::new();
    while let (Some(meta), Some(path)) = (parts.next(), parts.next()) {
        let fields: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let (old_mode, new_mode, status) = (
            fields.first().copied().unwrap_or(""),
            fields.get(1).copied().unwrap_or(""),
            fields.get(4).and_then(|s| s.chars().next()).unwrap_or('M'),
        );
        let name = path.rsplit('/').next().unwrap_or(path);
        if [old_mode, new_mode]
            .iter()
            .any(|m| *m == "120000" || *m == "160000")
        {
            refused.push(format!("{path} (lien symbolique ou sous-module)"));
        } else if name == ".gitattributes" || name == ".gitmodules" {
            refused.push(format!("{path} (réglage git)"));
        } else if !is_allowed(path) {
            refused.push(format!("{path} (hors périmètre)"));
        } else if touches_harness(path, status) {
            refused.push(format!(
                "{path} (harnais de test protégé : à modifier à la main dans le dépôt de développement)"
            ));
        }
        files.push(path.to_string());
    }
    if files.is_empty() {
        return Err("les commits ne modifient aucun fichier".into());
    }
    if !refused.is_empty() {
        return Err(format!(
            "fichiers refusés : {} (permis : {})",
            refused.join(", "),
            ALLOWED_DIRS.join(", ")
        ));
    }

    // Tests désactivés ou isolés.
    let diff = git(wt, &["diff", "-U0", "--no-color", base, "HEAD"]).await?;
    let markers = skip_markers(&diff);
    if !markers.is_empty() {
        return Err(format!(
            "tests désactivés ou isolés dans les lignes ajoutées : {}",
            markers.join(", ")
        ));
    }
    Ok(files)
}

/// Conditions d'une fusion fast-forward de `head` dans le main du clone.
async fn can_merge(clone: &Path, base: &str, head: &str) -> Result<(), String> {
    if resolve(clone, "refs/heads/main").await.as_deref() != Some(base) {
        return Err("main du clone modifié pendant le cycle".into());
    }
    let cur = git(clone, &["symbolic-ref", "--short", "HEAD"])
        .await
        .unwrap_or_default();
    if cur.trim() != "main" {
        return Err("le clone n'est pas sur main".into());
    }
    let status = git(clone, &["status", "--porcelain", "--untracked-files=no"]).await?;
    if !status.trim().is_empty() {
        return Err("le clone a des modifications locales".into());
    }
    if !git_ok(clone, &["merge-base", "--is-ancestor", base, head]).await? {
        return Err("la branche ne descend plus de main".into());
    }
    Ok(())
}

/// `git merge --ff-only` du commit testé `head` (pas de la tête de `_branch`,
/// qui a pu bouger) dans le main du clone, si les conditions tiennent
/// toujours ; vérifie que main vaut ensuite `head`.
async fn merge(clone: &Path, base: &str, _branch: &str, head: &str) -> Result<(), String> {
    can_merge(clone, base, head)
        .await
        .map_err(|e| format!("{e} : fusion abandonnée"))?;
    // Le commit testé, pas la tête de la branche (qui a pu bouger).
    git(clone, &["merge", "--ff-only", "-q", head]).await?;
    if resolve(clone, "refs/heads/main").await.as_deref() != Some(head) {
        return Err("main ne pointe pas sur la tête de la branche après la fusion".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_folds_accents_and_limits_words() {
        assert_eq!(
            slug("Ajouter un bouton « Éditer » à la barre"),
            "ajouter-un-bouton-editer-a"
        );
        assert_eq!(slug("  !!  "), "amelioration");
        assert_eq!(slug("Cœur"), "coeur");
        assert!(slug(&"x".repeat(100)).len() <= 40);
    }

    #[test]
    fn stamp_is_utc_civil_date() {
        // 2026-11-17 13:05:00 UTC.
        let t = UNIX_EPOCH + Duration::from_secs(1_794_920_700);
        assert_eq!(stamp(t), "20261117-1305");
        assert_eq!(stamp(UNIX_EPOCH), "19700101-0000");
    }

    #[test]
    fn allowed_harness_and_restart_rules() {
        assert!(is_allowed("src/App.svelte"));
        assert!(is_allowed("tools/self-improve/prompt.md"));
        assert!(!is_allowed("package.json"));
        assert!(!is_allowed("README.md"));
        assert!(!is_allowed("src/../package.json"));
        assert!(touches_harness("tools/self-improve/prompt.md", 'M'));
        assert!(touches_harness("tools/self-improve/nouveau.md", 'A'));
        assert!(touches_harness("tools/contract-fingerprint.mjs", 'M'));
        assert!(touches_harness("src-tauri/src/selfupdate/git.rs", 'M'));
        assert!(touches_harness("e2e/startup.spec.ts", 'M'));
        assert!(touches_harness("e2e/unit/keys.test.ts", 'D'));
        assert!(!touches_harness("e2e/unit/nouveau.test.ts", 'A'));
        assert!(!touches_harness("src-tauri/tests/nouveau.rs", 'A'));
        assert!(touches_harness("src-tauri/tests/ui_host.rs", 'M'));
        assert!(!touches_harness("src/App.svelte", 'M'));
        assert!(restart_reasons(&["src/App.svelte".into()]).is_empty());
        assert_eq!(restart_reasons(&["src-tauri/src/lib.rs".into()]).len(), 1);
        assert_eq!(
            restart_reasons(&["src-tauri/tauri.conf.json".into()]).len(),
            2
        );
        assert_eq!(
            restart_reasons(&["src/lib/core/bindings/UiInfo.ts".into()]).len(),
            1
        );
    }

    #[test]
    fn skip_markers_in_added_lines_only() {
        let diff = "diff --git a/e2e/unit/x.test.ts b/e2e/unit/x.test.ts\n\
                    --- a/e2e/unit/x.test.ts\n\
                    +++ b/e2e/unit/x.test.ts\n\
                    @@ -1 +1 @@\n\
                    -it.skip(\"retiré\", () => {});\n\
                    +it.only(\"ajouté\", () => {});\n\
                    +#[ignore]\n";
        let found = skip_markers(diff);
        // `#[ignore]` n'est cherché que dans les `.rs`.
        assert_eq!(found, ["e2e/unit/x.test.ts : « .only( »"]);
        assert!(skip_markers(&one_file("a.ts", "const ok = 1;")).is_empty());
    }

    /// Diff `-U0` d'un fichier `path` qui ajoute la ligne `added`.
    fn one_file(path: &str, added: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1 @@\n+{added}\n"
        )
    }

    #[test]
    fn skip_markers_only_in_code_files() {
        let n = |path: &str, added: &str| skip_markers(&one_file(path, added)).len();
        // Documentation et fichiers non-code : jamais concernés.
        assert_eq!(n("docs/spec.md", "ajoute `.skip(` ou `#[ignore]`"), 0);
        assert_eq!(
            n("tools/self-improve/prompt.md", "test.fail, describe.skip"),
            0
        );
        assert_eq!(n("src/data.json", "\"it.only(\""), 0);
        assert_eq!(n("Makefile", "x.skip(1)"), 0);
        // Code JS/TS/Svelte, test ou code applicatif.
        for p in [
            "e2e/a.spec.ts",
            "src/App.svelte",
            "src/x.tsx",
            "src/x.mjs",
            "src/X.TS",
        ] {
            assert_eq!(n(p, "it.skip(\"x\", () => {});"), 1, "{p}");
        }
        assert_eq!(n("src/lib/x.ts", "test.only(\"x\", () => {});"), 1);
        // Rust : `#[ignore]` seulement.
        assert_eq!(n("src-tauri/src/a.rs", "#[ignore]"), 1);
        assert_eq!(n("src-tauri/src/a.rs", "v.iter().skip(1)"), 0);
        assert_eq!(n("src/a.ts", "#[ignore]"), 0);
        // Chemin cité par git.
        let quoted = "diff --git \"a/caf\\303\\251.ts\" \"b/caf\\303\\251.ts\"\n\
                      +++ \"b/caf\\303\\251.ts\"\n@@ -0,0 +1 @@\n+it.only(1)\n";
        assert_eq!(skip_markers(quoted).len(), 1);
        // Chemin avec une espace : git termine la ligne `+++` par une tabulation.
        let spaced = "diff --git a/e2e/unit/my test.test.ts b/e2e/unit/my test.test.ts\n\
                      --- /dev/null\n+++ b/e2e/unit/my test.test.ts\t\n@@ -0,0 +1 @@\n+it.only(1)\n";
        assert_eq!(
            skip_markers(spaced),
            ["e2e/unit/my test.test.ts : « .only( »"]
        );
        // Une ligne ajoutée qui commence par `++ ` ne change pas de fichier.
        let sneaky = one_file("src/a.ts", "++ x.md") + "+it.only(1)\n";
        assert_eq!(skip_markers(&sneaky), ["src/a.ts : « .only( »"]);
    }

    #[test]
    fn js_markers_variants() {
        let hit = |l: &str| js_marker(l);
        for l in [
            "it.skipIf(ci)(\"x\", () => {});",
            "test.runIf(false)(\"x\", () => {});",
            "it.todo(\"x\");",
            "it.fails(\"x\", () => {});",
            "it.skip (\"x\", () => {});",
            "it.only.each([1])(\"x\", () => {});",
            "it[\"skip\"](\"x\", () => {});",
            "it['only'](\"x\", () => {});",
            "it[ 'skip' ](\"x\", () => {});",
            "xit(\"x\", () => {});",
            "  xtest (\"x\", () => {});",
            "xdescribe(\"x\", () => {});",
            "test.failing(\"x\", () => {});",
            "describe.skip\n",
        ] {
            assert!(hit(l).is_some(), "{l}");
        }
        for l in [
            "process.exit(1);",
            "maxit(2);",
            "obj.xit(1);",
            "const skipped = list.skipped;",
            "item.todo.title",
            "x.skipItems(2)",
            "const only = 1;",
            "a[\"skipper\"]",
        ] {
            assert_eq!(hit(l), None, "{l}");
        }
        assert_eq!(hit("it.skipIf(x)(1)").as_deref(), Some(".skipIf("));
        assert_eq!(hit("xit(1)").as_deref(), Some("xit("));
    }

    #[test]
    fn rust_and_cargo_markers() {
        let n = |path: &str, added: &str| skip_markers(&one_file(path, added)).len();
        for l in [
            "#[ignore = \"lent\"]",
            "# [ ignore ]",
            "#[cfg(any())]",
            "#[cfg( any( ) )]",
            "#[cfg(FALSE)]",
            "#[cfg(false)]",
            "#[cfg(not(all()))]",
            "#[cfg_attr(windows, ignore)]",
            "#[cfg_attr(test, ignore = \"x\")]",
        ] {
            assert_eq!(n("src-tauri/src/a.rs", l), 1, "{l}");
        }
        assert_eq!(n("src-tauri/src/a.rs", "#[cfg(test)]"), 0);
        assert_eq!(
            n("src-tauri/src/a.rs", "#[cfg_attr(test, derive(Debug))]"),
            0
        );
        for l in [
            "autotests = false",
            "autobins=false",
            "autobenches = false",
            "test = false",
            "test=false",
            "harness = false",
            "doctest = false",
        ] {
            assert_eq!(n("src-tauri/Cargo.toml", l), 1, "{l}");
        }
        assert_eq!(n("src-tauri/Cargo.toml", "serde = \"1\""), 0);
        assert_eq!(n("src-tauri/Cargo.lock", "harness"), 0);
    }

    #[test]
    fn cargo_config_and_build_script_are_protected() {
        for p in ["src-tauri/.cargo/config.toml", "src-tauri/build.rs"] {
            assert!(touches_harness(p, 'M'), "{p}");
            assert!(touches_harness(p, 'A'), "{p}");
        }
        assert!(touches_harness("src-tauri/.cargo/autre.toml", 'A'));
        assert!(!touches_harness("src-tauri/Cargo.toml", 'M'));
        for p in ["src-tauri/.cargo", "src-tauri/build.rs"] {
            assert!(HARNESS_PATHS.contains(&p), "{p}");
        }
    }

    #[test]
    fn port_check_sees_listener() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_free(port));
        drop(l);
        assert!(port_free(port));
    }

    #[test]
    fn prompt_fallback_has_placeholders() {
        for p in ["{{DEMANDE}}", "{{BRANCHE}}", "{{DOSSIERS}}", "git commit"] {
            assert!(PROMPT_FALLBACK.contains(p), "{p} absent du prompt");
        }
    }
}
