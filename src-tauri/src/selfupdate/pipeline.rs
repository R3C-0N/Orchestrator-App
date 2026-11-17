//! Cycle d'une amélioration (J14, décision 17) : worktree `auto/…` depuis le
//! `main` du clone, session dédiée (par [`PipelineHost`]), contrôles
//! déterministes, tests, détection « relance requise » (décision 18), build
//! de l'interface, bascule, fusion `--ff-only`. Les commandes npm/node/cargo
//! passent par [`Toolchain`] (simulé dans les tests).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use super::git::{self, git, git_ok, resolve};
use super::{
    repo, BoxFut, ImproveOutcome, ImproveRequest, Pipeline, StatusSink, ALLOWED_DIRS, BRANCH_PREFIX,
};
use crate::api::types::{CoreError, ImproveStatus, ImproveStep};

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

/// Commandes de dépendances, tests, build et empreinte, lancées par le cycle.
/// `Err` : message lisible (fin de la sortie de la commande en échec).
pub trait Toolchain: Send + Sync {
    /// `npm ci` dans `dir`.
    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>>;
    /// Tests obligatoires dans le worktree ; `rust` : `src-tauri/` touché
    /// (`cargo test` en plus).
    fn test<'a>(&'a self, worktree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>>;
    /// Build de l'interface dans `out` (dossier `<hash>.tmp`), étiqueté `hash`.
    fn build_ui<'a>(
        &'a self,
        worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>>;
    /// Empreinte du contrat du worktree (`tools/contract-fingerprint.mjs`).
    fn contract<'a>(&'a self, worktree: &'a Path) -> BoxFut<'a, Result<String, String>>;
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
        let mut c = git::command(program);
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
            Err(format!("{what} a échoué :\n{}", out.tail(20)))
        }
    }
}

impl Toolchain for NpmToolchain {
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

    fn test<'a>(&'a self, worktree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let npm = git::npm_program();
            self.step(worktree, npm, &["run", "check"], &[], "npm run check")
                .await?;
            self.step(worktree, npm, &["test"], &[], "npm test").await?;
            if !worktree.join(E2E_CONFIG).is_file() {
                return Err(format!(
                    "{E2E_CONFIG} absent du clone : e2e de démarrage impossibles"
                ));
            }
            self.step(
                worktree,
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
                    &worktree.join("src-tauri"),
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

    fn contract<'a>(&'a self, worktree: &'a Path) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            self.step(
                worktree,
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

/// Cycle en cours (un seul à la fois).
pub(super) struct Active {
    pub id: String,
    pub cancel: watch::Sender<bool>,
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
}

impl Reporter {
    fn emit(&mut self, step: ImproveStep, detail: Option<String>) {
        self.status.step = step;
        self.status.detail = detail;
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

/// Attend `fut`, sauf annulation demandée avant sa fin (le futur est alors
/// abandonné : processus tués par `kill_on_drop`, session arrêtée par l'hôte).
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

/// Jonction (Windows) ou lien symbolique `link` -> `target`.
async fn link_dir(link: &Path, target: &Path) -> Result<(), String> {
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

impl Pipeline {
    /// Dossier du worktree de `branch` (`<work_root>\auto\…`).
    pub fn worktree_path(&self, branch: &str) -> PathBuf {
        branch
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .fold(self.work_root.clone(), |p, c| p.join(c))
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
            });
        }
        let _guard = ActiveGuard(&self.active);
        let reverting = commit.is_some();
        let mode = match commit {
            Some(c) => Mode::Revert(c),
            None => Mode::Improve,
        };
        let mut rep = Reporter {
            status: ImproveStatus {
                id: request.id.clone(),
                branch: request.branch.clone(),
                request: request.text.clone(),
                step: ImproveStep::Preparing,
                session: None,
                commit: None,
                detail: None,
                t: now_ms(),
            },
            sink,
        };
        let outcome = match self.cycle(&request, &mode, &mut rep, &mut rx).await {
            Ok(o) => o,
            Err(Stop::Cancelled) => {
                rep.emit(
                    ImproveStep::Cancelled,
                    Some("annulée : branche laissée non fusionnée".into()),
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
                ImproveOutcome::Failed { step, reason }
            }
        };
        Ok(outcome)
    }

    async fn cycle(
        &self,
        req: &ImproveRequest,
        mode: &Mode,
        rep: &mut Reporter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ImproveOutcome, Stop> {
        use ImproveStep as S;
        let clone = self.repo.dir.clone();

        // --- Préparation : worktree, branche, node_modules.
        rep.emit(S::Preparing, None);
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

        // --- Session dédiée.
        if let Mode::Improve = mode {
            rep.emit(S::Working, None);
            let prompt = build_prompt(&wt, req);
            guarded(cancel, self.host.run_session(req, &wt, &prompt))
                .await?
                .map_err(|e| Stop::Failed(S::Working, e.to_string()))?;
        }

        // --- Contrôles déterministes.
        rep.emit(S::Checking, None);
        let files = guarded(cancel, check(&clone, &wt, &req.branch, &base))
            .await?
            .map_err(fail(S::Checking))?;
        let head = resolve(&wt, "HEAD")
            .await
            .ok_or_else(|| Stop::Failed(S::Checking, "tête de branche illisible".into()))?;
        let short = git::short(&wt, "HEAD").await.map_err(fail(S::Checking))?;
        rep.status.commit = Some(short.clone());
        let mut reasons = restart_reasons(&files);
        match guarded(cancel, self.toolchain.contract(&wt)).await? {
            Ok(fp) if fp.trim() == self.contract => {}
            Ok(fp) => reasons.push(format!(
                "empreinte du contrat {} différente de celle de l'exe ({})",
                fp.trim(),
                self.contract
            )),
            Err(e) => reasons.push(format!("empreinte du contrat illisible : {e}")),
        }

        // --- Tests obligatoires.
        rep.emit(S::Testing, None);
        let rust = files.iter().any(|f| f.starts_with("src-tauri/"));
        guarded(cancel, self.toolchain.test(&wt, rust))
            .await?
            .map_err(fail(S::Testing))?;

        // --- Cœur modifié : fusion sans activation (décision 18).
        if !reasons.is_empty() {
            merge(&clone, &base, &req.branch, &head)
                .await
                .map_err(fail(S::Merged))?;
            self.remove_worktree(&wt).await;
            rep.emit(
                S::RestartRequired,
                Some(format!("relance requise : {}", reasons.join(" ; "))),
            );
            return Ok(ImproveOutcome::RestartRequired { commit: head });
        }

        // --- Build de l'interface étiquetée par le hash.
        rep.emit(S::Building, None);
        std::fs::create_dir_all(&self.ui_root)
            .map_err(|e| Stop::Failed(S::Building, format!("{} : {e}", self.ui_root.display())))?;
        let out = self.ui_root.join(format!("{short}.tmp"));
        if out.exists() {
            std::fs::remove_dir_all(&out)
                .map_err(|e| Stop::Failed(S::Building, format!("{} : {e}", out.display())))?;
        }
        guarded(cancel, self.toolchain.build_ui(&wt, &short, &out))
            .await?
            .map_err(fail(S::Building))?;
        if !out.join("index.html").is_file() {
            return Err(Stop::Failed(
                S::Building,
                format!("index.html absent de {}", out.display()),
            ));
        }

        // --- Bascule : attente du repos, puis rechargement et signal « prêt ».
        rep.emit(S::WaitingIdle, None);
        guarded(cancel, self.host.wait_idle())
            .await?
            .map_err(|e| Stop::Failed(S::WaitingIdle, e.to_string()))?;
        // Plus d'annulation à partir d'ici : la bascule va au bout.
        rep.emit(S::Switching, None);
        let ready = self
            .host
            .switch_ui(&short, &out)
            .await
            .map_err(|e| Stop::Failed(S::Switching, e.to_string()))?;
        if !ready {
            return Err(Stop::Failed(
                S::Switching,
                "la nouvelle interface n'a pas signalé « prêt » à temps : version précédente \
                 rétablie, branche non fusionnée"
                    .into(),
            ));
        }

        // --- Fusion fast-forward dans le main du clone.
        merge(&clone, &base, &req.branch, &head)
            .await
            .map_err(fail(S::Merged))?;
        self.remove_worktree(&wt).await;
        let (step, detail) = match mode {
            Mode::Improve => (S::Merged, format!("fusionnée dans main ({short})")),
            Mode::Revert(c) => (
                S::RolledBack,
                format!("{} annulé par {short}", &c[..c.len().min(10)]),
            ),
        };
        rep.emit(step, Some(detail));
        Ok(ImproveOutcome::Merged { commit: head })
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

    /// Supprime le worktree `wt` (jonction `node_modules` retirée d'abord :
    /// jamais de suppression à travers elle).
    pub async fn remove_worktree(&self, wt: &Path) {
        let nm = wt.join("node_modules");
        if is_link(&nm) {
            if std::fs::remove_dir(&nm).is_err() {
                let _ = std::fs::remove_file(&nm);
            }
            if is_link(&nm) {
                tracing::warn!("jonction {} non retirée : worktree gardé", nm.display());
                return;
            }
        }
        let wt_s = wt.to_string_lossy().into_owned();
        let _ = git(&self.repo.dir, &["worktree", "remove", "--force", &wt_s]).await;
        if wt.exists() {
            let _ = std::fs::remove_dir_all(wt);
            let _ = git(&self.repo.dir, &["worktree", "prune"]).await;
        }
    }

    /// Supprime les worktrees `auto/…` sous `work_root` dont la branche est
    /// fusionnée dans `main` ; ceux des échecs restent pour inspection.
    /// Renvoie le nombre de worktrees supprimés.
    pub async fn cleanup_merged(&self) -> usize {
        let Ok(list) = git(&self.repo.dir, &["worktree", "list", "--porcelain"]).await else {
            return 0;
        };
        let mut removed = 0;
        let mut path: Option<PathBuf> = None;
        for line in list.lines().chain(std::iter::once("")) {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                let Some(p) = path.take() else { continue };
                let under = p
                    .canonicalize()
                    .ok()
                    .zip(self.work_root.canonicalize().ok())
                    .is_some_and(|(p, w)| p.starts_with(w));
                if b.starts_with(BRANCH_PREFIX)
                    && under
                    && git_ok(&self.repo.dir, &["merge-base", "--is-ancestor", b, "main"])
                        .await
                        .unwrap_or(false)
                {
                    self.remove_worktree(&p).await;
                    removed += 1;
                }
            } else if line.is_empty() {
                path = None;
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

/// Contrôles déterministes après la session ; renvoie les fichiers touchés
/// par la branche (chemins git).
async fn check(clone: &Path, wt: &Path, branch: &str, base: &str) -> Result<Vec<String>, String> {
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
    let names = git(wt, &["diff", "--name-only", "--no-renames", base, "HEAD"]).await?;
    let files: Vec<String> = names
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if files.is_empty() {
        return Err("les commits ne modifient aucun fichier".into());
    }
    let outside: Vec<&str> = files
        .iter()
        .filter(|f| !is_allowed(f))
        .map(String::as_str)
        .collect();
    if !outside.is_empty() {
        return Err(format!(
            "fichiers hors périmètre : {} (permis : {})",
            outside.join(", "),
            ALLOWED_DIRS.join(", ")
        ));
    }
    Ok(files)
}

/// `git merge --ff-only` de `branch` dans le main du clone, si main est
/// toujours `base` ; vérifie que main vaut ensuite `head`.
async fn merge(clone: &Path, base: &str, branch: &str, head: &str) -> Result<(), String> {
    if resolve(clone, "refs/heads/main").await.as_deref() != Some(base) {
        return Err("main du clone modifié pendant le cycle : fusion abandonnée".into());
    }
    let cur = git(clone, &["symbolic-ref", "--short", "HEAD"])
        .await
        .unwrap_or_default();
    if cur.trim() != "main" {
        return Err("le clone n'est pas sur main : fusion abandonnée".into());
    }
    git(clone, &["merge", "--ff-only", "-q", branch]).await?;
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
    fn allowed_and_restart_rules() {
        assert!(is_allowed("src/App.svelte"));
        assert!(is_allowed("tools/self-improve/prompt.md"));
        assert!(!is_allowed("package.json"));
        assert!(!is_allowed("README.md"));
        assert!(!is_allowed("src/../package.json"));
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
    fn prompt_fallback_has_placeholders() {
        for p in ["{{DEMANDE}}", "{{BRANCHE}}", "{{DOSSIERS}}", "git commit"] {
            assert!(PROMPT_FALLBACK.contains(p), "{p} absent du prompt");
        }
    }
}
