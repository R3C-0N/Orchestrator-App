//! J14, étape 3 : clone et cycle git de l'auto-amélioration, sur des dépôts
//! git temporaires (un « origin » nu local comme source du clone), avec un
//! `PipelineHost` et une `Toolchain` simulés. Aucun accès réseau. Chaque test
//! nettoie son dossier, même en cas d'échec (garde `Drop`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::events::NoticeLevel;
use orchestrator_lib::api::types::{
    CoreError, ImproveStatus, ImproveStep, ImproveWaiting, RepoStatus,
};
use orchestrator_lib::selfupdate::feedback::MAX_FEEDBACK_BYTES;
use orchestrator_lib::selfupdate::pipeline::{
    recover_interrupted_session, NpmToolchain, Toolchain, IMPROVE_LOCK,
};
use orchestrator_lib::selfupdate::repo::{
    check_url, ensure_clone_with, load_from, CloneOptions, ToolCheck, NO_PUSH_URL,
};
use orchestrator_lib::selfupdate::{
    BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, RepoConfig, StatusSink,
    TurnEnd, MAX_AUTO_ATTEMPTS,
};

// --- Outils de test -----------------------------------------------------------

/// Charge du binaire de test : chaque test la prend en lecture (via
/// [`TempRoot::new`]) ; une mesure de latence la prend en écriture
/// ([`TempRoot::exclusive`]) et tourne donc seule, sans dépendre de la
/// saturation de la machine par les autres tests (processus git en rafale).
static LOAD: std::sync::RwLock<()> = std::sync::RwLock::new(());

enum LoadGuard {
    Shared(#[allow(dead_code)] std::sync::RwLockReadGuard<'static, ()>),
    Exclusive(#[allow(dead_code)] std::sync::RwLockWriteGuard<'static, ()>),
}

/// Dossier temporaire d'un test, supprimé au drop (même après un échec).
struct TempRoot(PathBuf, #[allow(dead_code)] LoadGuard);

impl TempRoot {
    fn new(name: &str) -> Self {
        let g = LOAD.read().unwrap_or_else(|e| e.into_inner());
        Self::with(name, LoadGuard::Shared(g))
    }

    /// Comme [`TempRoot::new`], seul test en cours (mesure de latence).
    fn exclusive(name: &str) -> Self {
        let g = LOAD.write().unwrap_or_else(|e| e.into_inner());
        Self::with(name, LoadGuard::Exclusive(g))
    }

    fn with(name: &str, guard: LoadGuard) -> Self {
        let d = std::env::temp_dir().join(format!("orch-c-{name}-{}", std::process::id()));
        if d.exists() {
            let _ = std::fs::remove_dir_all(&d);
        }
        std::fs::create_dir_all(&d).unwrap();
        TempRoot(d, guard)
    }
}

impl std::ops::Deref for TempRoot {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        // `remove_dir_all` ne suit pas les jonctions.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `git -C dir args` synchrone, identité fixée ; panique si échec.
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = sh_out(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} dans {} : {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sh_out(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args)
        .output()
        .expect("git introuvable")
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// Contenu d'un fichier extrait, sans `\r` (`core.autocrlf` de l'utilisateur).
fn read_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap().replace('\r', "")
}

/// Dépôt de départ (2 commits) et son « origin » nu. Renvoie (url, c1, c2).
fn make_origin(root: &Path) -> (String, String, String) {
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    sh(&seed, &["init", "-q", "-b", "main"]);
    write(&seed, "src/app.txt", "v1\n");
    write(&seed, "src-tauri/core.rs", "fn main() {}\n");
    write(
        &seed,
        "e2e/existing.spec.ts",
        "test(\"démarre\", () => {});\n",
    );
    write(&seed, "README.md", "# app\n");
    write(&seed, ".gitignore", "node_modules/\n");
    sh(
        &seed,
        &[
            "add",
            "--",
            "src",
            "src-tauri",
            "e2e",
            "README.md",
            ".gitignore",
        ],
    );
    sh(&seed, &["commit", "-q", "-m", "c1"]);
    let c1 = sh(&seed, &["rev-parse", "HEAD"]);
    write(&seed, "src/app.txt", "v2\n");
    sh(&seed, &["commit", "-q", "-m", "c2", "--", "src/app.txt"]);
    let c2 = sh(&seed, &["rev-parse", "HEAD"]);
    let bare = root.join("origin.git");
    sh(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            &seed.to_string_lossy(),
            &bare.to_string_lossy(),
        ],
    );
    (bare.to_string_lossy().into_owned(), c1, c2)
}

fn git_only() -> CloneOptions {
    CloneOptions {
        required_tools: vec![ToolCheck::new("git", "git", &["--version"])],
        clone_timeout: Duration::from_secs(60),
        ..CloneOptions::default()
    }
}

/// Clone prêt, placé sur c1 (commit de compilation simulé).
async fn ready_clone(root: &Path) -> (RepoConfig, String, String) {
    let (url, c1, c2) = make_origin(root);
    let cfg = RepoConfig {
        url,
        dir: root.join("install").join("repo"),
        commit: c1.clone(),
    };
    let st = ensure_clone_with(&cfg, &git_only()).await;
    assert!(matches!(st, RepoStatus::Ready { .. }), "{st:?}");
    (cfg, c1, c2)
}

type Hook = Option<Box<dyn Fn(&Path) + Send + Sync>>;

/// Un commit fait par la session simulée.
struct FakeCommit {
    files: Vec<(&'static str, &'static str)>,
    msg: &'static str,
}

fn commit_src(files: Vec<(&'static str, &'static str)>, msg: &'static str) -> FakeCommit {
    FakeCommit { files, msg }
}

#[derive(Default)]
struct FakeHost {
    commits: Vec<FakeCommit>,
    /// Action libre de la session, après les commits (reçoit le worktree).
    extra: Hook,
    /// Fichier laissé non commité.
    dirty: Option<(&'static str, &'static str)>,
    /// La session ne finit jamais (annulation).
    hang: bool,
    /// La session se termine en erreur (après ses actions).
    fail: bool,
    switch_ok: bool,
    /// Action pendant la bascule (reçoit le dossier du build).
    on_switch: Hook,
    switched: Mutex<Vec<(String, PathBuf)>>,
    rollbacks: AtomicUsize,
    prompts: Mutex<Vec<String>>,
    /// `node_modules` du worktree était une jonction pendant la session.
    junction_seen: Mutex<Option<bool>>,
    /// `.git` du clone observé par `session_stopped` : (config en lecture
    /// seule, verrou `improve-active` présent) à ce moment.
    watch_git: Option<PathBuf>,
    stopped_seen: Mutex<Option<(bool, bool)>>,
}

impl PipelineHost for FakeHost {
    fn run_session<'a>(
        &'a self,
        _request: &'a ImproveRequest,
        cwd: &'a Path,
        prompt: &'a str,
    ) -> BoxFut<'a, Result<(), CoreError>> {
        Box::pin(async move {
            self.prompts.lock().unwrap().push(prompt.to_string());
            *self.junction_seen.lock().unwrap() = Some(
                std::fs::symlink_metadata(cwd.join("node_modules"))
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false),
            );
            if self.hang {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            for c in &self.commits {
                let mut names = Vec::new();
                for (rel, content) in &c.files {
                    write(cwd, rel, content);
                    names.push(*rel);
                }
                let mut args = vec!["add", "--"];
                args.extend(&names);
                sh(cwd, &args);
                let mut args = vec!["commit", "-q", "-m", c.msg, "--"];
                args.extend(&names);
                sh(cwd, &args);
            }
            if let Some(f) = &self.extra {
                f(cwd);
            }
            if let Some((rel, content)) = self.dirty {
                write(cwd, rel, content);
            }
            if self.fail {
                return Err(CoreError::Io("session simulée en échec".into()));
            }
            Ok(())
        })
    }

    fn switch_ui<'a>(
        &'a self,
        hash: &'a str,
        dir: &'a Path,
    ) -> BoxFut<'a, Result<bool, CoreError>> {
        Box::pin(async move {
            self.switched
                .lock()
                .unwrap()
                .push((hash.to_string(), dir.to_path_buf()));
            if let Some(f) = &self.on_switch {
                f(dir);
            }
            Ok(self.switch_ok)
        })
    }

    fn rollback_ui(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move {
            self.rollbacks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn session_stopped(&self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            if let Some(git) = &self.watch_git {
                let ro = std::fs::metadata(git.join("config"))
                    .map(|m| m.permissions().readonly())
                    .unwrap_or(false);
                let lock = git.join(IMPROVE_LOCK).is_file();
                *self.stopped_seen.lock().unwrap() = Some((ro, lock));
            }
        })
    }
}

/// Appel de `Toolchain::test` : dossier, Rust, fichiers de `e2e/` à ce
/// moment, contenu de `e2e/existing.spec.ts`.
type TestCall = (PathBuf, bool, Vec<String>, String);

struct FakeToolchain {
    contract: String,
    tests_ok: bool,
    /// Issues des appels successifs de `test` (jalon J16), avant `tests_ok`.
    script: Mutex<VecDeque<TestStep>>,
    tests: Mutex<Vec<TestCall>>,
    installs: Mutex<Vec<PathBuf>>,
    on_build: Hook,
    /// Action pendant les tests (reçoit le dossier des tests).
    on_test: Hook,
}

impl FakeToolchain {
    fn new() -> Self {
        FakeToolchain {
            contract: CONTRACT.into(),
            tests_ok: true,
            script: Mutex::default(),
            tests: Mutex::new(Vec::new()),
            installs: Mutex::new(Vec::new()),
            on_build: None,
            on_test: None,
        }
    }

    fn rust_flags(&self) -> Vec<bool> {
        self.tests.lock().unwrap().iter().map(|t| t.1).collect()
    }
}

const CONTRACT: &str = "00c0ffee00c0ffee";

/// Issue d'un appel de `FakeToolchain::test` (jalon J16).
enum TestStep {
    Ok,
    /// Échec avec cette sortie (`{TREE}` : dossier des tests).
    Fail(String),
    /// Vrai processus sous Job Object, dossier courant = dossier des tests
    /// (il le tient ouvert), qui ne finit jamais (abandon par un message).
    HangProcess,
}

fn list_rel(root: &Path, sub: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(sub)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy();
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

impl Toolchain for FakeToolchain {
    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            self.installs.lock().unwrap().push(dir.to_path_buf());
            write(dir, "node_modules/marker.txt", "deps");
            Ok(())
        })
    }

    fn test<'a>(&'a self, tree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let spec =
                std::fs::read_to_string(tree.join("e2e/existing.spec.ts")).unwrap_or_default();
            self.tests.lock().unwrap().push((
                tree.to_path_buf(),
                rust,
                list_rel(tree, "e2e"),
                spec.replace('\r', ""),
            ));
            if let Some(f) = &self.on_test {
                f(tree);
            }
            let step = self.script.lock().unwrap().pop_front();
            match step {
                Some(TestStep::Ok) => Ok(()),
                Some(TestStep::Fail(text)) => Err(text.replace("{TREE}", &tree.to_string_lossy())),
                Some(TestStep::HangProcess) => {
                    let mut c = if cfg!(windows) {
                        let mut c = orchestrator_lib::selfupdate::git::command("cmd");
                        c.args(["/C", "ping", "-n", "600", "127.0.0.1"]);
                        c
                    } else {
                        let mut c = orchestrator_lib::selfupdate::git::command("sleep");
                        c.arg("600");
                        c
                    };
                    c.current_dir(tree);
                    let _ = orchestrator_lib::selfupdate::git::run(
                        c,
                        Duration::from_secs(3600),
                        "attente",
                    )
                    .await;
                    Ok(())
                }
                None if self.tests_ok => Ok(()),
                None => Err("npm test a échoué :\n1 test en échec".into()),
            }
        })
    }

    fn build_ui<'a>(
        &'a self,
        worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            write(out, "index.html", &format!("<!-- {hash} -->"));
            if let Some(f) = &self.on_build {
                f(worktree);
            }
            Ok(())
        })
    }

    fn contract<'a>(&'a self, _tree: &'a Path) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move { Ok(self.contract.clone()) })
    }
}

fn pipeline(root: &Path, cfg: RepoConfig, host: Arc<FakeHost>, tc: Arc<FakeToolchain>) -> Pipeline {
    Pipeline::new(cfg, host)
        .with_toolchain(tc)
        .with_dirs(root.join("work"), root.join("ui"))
        .with_contract(CONTRACT)
        .with_config_file(root.join("config.json"))
}

fn collector() -> (StatusSink, Arc<Mutex<Vec<ImproveStatus>>>) {
    let v: Arc<Mutex<Vec<ImproveStatus>>> = Arc::default();
    let w = v.clone();
    (Arc::new(move |s| w.lock().unwrap().push(s)), v)
}

fn steps(v: &Arc<Mutex<Vec<ImproveStatus>>>) -> Vec<ImproveStep> {
    v.lock().unwrap().iter().map(|s| s.step).collect()
}

fn main_of(cfg: &RepoConfig) -> String {
    sh(&cfg.dir, &["rev-parse", "main"])
}

/// Lance un cycle et renvoie son issue et les statuts émis.
async fn run(p: &Pipeline, text: &str) -> (ImproveOutcome, Vec<ImproveStatus>) {
    let (sink, log) = collector();
    let out = p.run(p.request(text).unwrap(), sink).await.unwrap();
    let log = log.lock().unwrap().clone();
    (out, log)
}

/// Le cycle est refusé au contrôle, avec une raison contenant `needle`.
fn assert_refused(out: &ImproveOutcome, needle: &str) {
    match out {
        ImproveOutcome::Failed {
            step: ImproveStep::Checking,
            reason,
        } => assert!(reason.contains(needle), "« {needle} » absent de : {reason}"),
        other => panic!("refus attendu ({needle}) : {other:?}"),
    }
}

fn success_host(commits: Vec<FakeCommit>) -> FakeHost {
    FakeHost {
        commits,
        switch_ok: true,
        ..Default::default()
    }
}

// --- Clone ---------------------------------------------------------------------

#[tokio::test]
async fn clone_is_placed_on_build_commit_and_cannot_push() {
    let root = TempRoot::new("clone");
    let (cfg, c1, c2) = ready_clone(&root).await;
    assert_eq!(
        main_of(&cfg),
        c1,
        "main du clone sur le commit de compilation"
    );
    assert_ne!(c1, c2);
    assert_eq!(sh(&cfg.dir, &["rev-parse", "origin/main"]), c2);
    assert_eq!(sh(&cfg.dir, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(
        sh(&cfg.dir, &["remote", "get-url", "--push", "origin"]),
        NO_PUSH_URL
    );
    assert!(!root.join("install").join("repo.clone-tmp").exists());
    assert_eq!(read_text(&cfg.dir.join("src/app.txt")), "v1\n");

    // Un vrai push vers l'origin temporaire échoue.
    write(&cfg.dir, "src/local.txt", "x\n");
    sh(&cfg.dir, &["add", "--", "src/local.txt"]);
    sh(
        &cfg.dir,
        &["commit", "-q", "-m", "local", "--", "src/local.txt"],
    );
    let push = sh_out(&cfg.dir, &["push", "origin", "main:refs/heads/pousse"]);
    assert!(!push.status.success(), "le push doit échouer");
    let bare = PathBuf::from(&cfg.url);
    assert!(
        !sh_out(&bare, &["rev-parse", "--verify", "-q", "refs/heads/pousse"])
            .status
            .success()
    );

    // Deuxième lancement : clone existant gardé tel quel.
    match ensure_clone_with(&cfg, &git_only()).await {
        RepoStatus::Ready { path, .. } => assert_eq!(PathBuf::from(path), cfg.dir),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn clone_into_existing_empty_dir() {
    let root = TempRoot::new("cloneempty");
    let (url, c1, _) = make_origin(&root);
    let cfg = RepoConfig {
        url,
        dir: root.join("vide").join("repo"),
        commit: c1.clone(),
    };
    std::fs::create_dir_all(&cfg.dir).unwrap();
    match ensure_clone_with(&cfg, &git_only()).await {
        RepoStatus::Ready { head, .. } => assert_eq!(head, c1),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn clone_failures_disable_with_reason() {
    let root = TempRoot::new("clonefail");
    let (url, c1, _) = make_origin(&root);

    // Dépôt distant introuvable.
    let bad = RepoConfig {
        url: root.join("absent.git").to_string_lossy().into_owned(),
        dir: root.join("a").join("repo"),
        commit: c1.clone(),
    };
    match ensure_clone_with(&bad, &git_only()).await {
        RepoStatus::Disabled { reason } => assert!(reason.contains("clonage"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!bad.dir.exists() && !root.join("a").join("repo.clone-tmp").exists());

    // URL qui injecterait une option, transport `ext::`, forme inconnue.
    for url in [
        "--upload-pack=calc.exe",
        "-uhack",
        "ext::sh -c touch% /tmp/x",
        "relatif/origin.git",
        "http://exemple.org/a.git",
    ] {
        let cfg = RepoConfig {
            url: url.into(),
            dir: root.join("u").join("repo"),
            commit: c1.clone(),
        };
        match ensure_clone_with(&cfg, &git_only()).await {
            RepoStatus::Disabled { reason } => {
                assert!(reason.contains("URL du dépôt refusée"), "{url} : {reason}")
            }
            other => panic!("{url} : {other:?}"),
        }
    }
    for ok in [
        "git@github.com:R3C-0N/Orchestrator-App.git",
        "ssh://git@host/x.git",
        "https://host/x.git",
        "file:///C:/x.git",
    ] {
        assert!(check_url(ok).is_ok(), "{ok}");
    }
    assert!(check_url(&url).is_ok());

    // Outil absent.
    let mut opts = git_only();
    opts.required_tools.push(ToolCheck::new(
        "Node",
        "outil-absent-orch-c",
        &["--version"],
    ));
    let cfg = RepoConfig {
        url: url.clone(),
        dir: root.join("b").join("repo"),
        commit: c1.clone(),
    };
    match ensure_clone_with(&cfg, &opts).await {
        RepoStatus::Disabled { reason } => {
            assert!(reason.starts_with("Node : introuvable"), "{reason}")
        }
        other => panic!("{other:?}"),
    }

    // Commit de compilation absent de l'origin.
    let cfg = RepoConfig {
        url: url.clone(),
        dir: root.join("c").join("repo"),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
    };
    match ensure_clone_with(&cfg, &git_only()).await {
        RepoStatus::Disabled { reason } => assert!(reason.contains("introuvable"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!cfg.dir.exists());

    // Exe compilé hors dépôt git.
    let cfg = RepoConfig {
        url,
        dir: root.join("d").join("repo"),
        commit: "unknown".into(),
    };
    assert!(matches!(
        ensure_clone_with(&cfg, &git_only()).await,
        RepoStatus::Disabled { .. }
    ));
}

#[test]
fn config_url_and_clone_location() {
    let root = TempRoot::new("config");
    let install = root.join("install");
    let data = root.join("data");
    std::fs::create_dir_all(&install).unwrap();
    let conf = root.join("config.json");
    std::fs::write(
        &conf,
        r#"{ "repo_url": "C:/autre/origin.git", "theme": "x" }"#,
    )
    .unwrap();

    let c = load_from(Some(&conf), Some(&install), Some(&data), "abc");
    assert_eq!(c.url, "C:/autre/origin.git");
    assert_eq!(
        c.dir,
        install.join("repo"),
        "dossier d'installation inscriptible"
    );
    assert_eq!(c.commit, "abc");

    // Pas de config, dossier d'installation absent : %LOCALAPPDATA%.
    let c = load_from(None, Some(&root.join("absent")), Some(&data), "abc");
    assert_eq!(c.url, orchestrator_lib::selfupdate::DEFAULT_ORIGIN);
    assert_eq!(c.dir, data.join("repo"));
}

// --- Cycle ---------------------------------------------------------------------

#[tokio::test]
async fn request_names_branch() {
    let root = TempRoot::new("request");
    let (cfg, _, _) = ready_clone(&root).await;
    let p = pipeline(
        &root,
        cfg,
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    );
    let r = p.request("Ajouter un bouton « Copier »").unwrap();
    let rest = r.branch.strip_prefix("auto/").expect(&r.branch);
    let (stamp, subject) = rest.split_at(13);
    assert!(
        stamp[..8].chars().all(|c| c.is_ascii_digit()) && &stamp[8..9] == "-",
        "{rest}"
    );
    assert!(stamp[9..13].chars().all(|c| c.is_ascii_digit()), "{rest}");
    assert_eq!(subject, "-ajouter-un-bouton-copier");
    assert!(p.request("   ").is_err());
}

#[tokio::test]
async fn success_fast_forwards_main_with_branch_commits_only() {
    let root = TempRoot::new("success");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![
        commit_src(vec![("src/a.txt", "a\n")], "ajoute a"),
        commit_src(
            vec![
                ("src/b.txt", "b\n"),
                ("docs/n.md", "n\n"),
                ("e2e/unit/nouveau.test.ts", "test(\"b\", () => {});\n"),
            ],
            "ajoute b",
        ),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    let p = pipeline(&root, cfg.clone(), host.clone(), tc.clone());
    let req = p.request("ajouter a et b").unwrap();
    let wt = p.worktree_path(&req.branch);
    let (sink, log) = collector();

    let out = p.run(req.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?} / {:?}", log.lock().unwrap().last())
    };
    use ImproveStep as S;
    assert_eq!(
        steps(&log),
        [
            S::Preparing,
            S::Working,
            S::Checking,
            S::Testing,
            S::Building,
            S::WaitingIdle,
            S::Switching,
            S::Merged
        ]
    );
    // main avancé en fast-forward des seuls 2 commits de la branche.
    assert_eq!(main_of(&cfg), commit);
    assert_eq!(
        sh(&cfg.dir, &["rev-list", "--count", &format!("{c1}..main")]),
        "2"
    );
    assert_eq!(
        sh(&cfg.dir, &["rev-list", "--merges", &format!("{c1}..main")]),
        ""
    );
    assert_eq!(sh(&cfg.dir, &["rev-parse", "main~2"]), c1);
    assert_eq!(read_text(&cfg.dir.join("src/b.txt")), "b\n");
    // Bascule avec le hash court de main et le build `<hash>.tmp`.
    let switched = host.switched.lock().unwrap().clone();
    assert_eq!(switched.len(), 1);
    assert_eq!(
        switched[0].0,
        sh(&cfg.dir, &["rev-parse", "--short", "main"])
    );
    assert_eq!(
        switched[0].1,
        root.join("ui").join(format!("{}.tmp", switched[0].0))
    );
    assert!(switched[0].1.join("index.html").is_file());
    assert_eq!(
        log.lock().unwrap().last().unwrap().commit.as_deref(),
        Some(switched[0].0.as_str())
    );
    // Prompt versionné, rempli.
    let prompt = host.prompts.lock().unwrap()[0].clone();
    assert!(
        prompt.contains("ajouter a et b") && prompt.contains(&req.branch),
        "{prompt}"
    );
    assert!(
        prompt.contains("git commit") && prompt.contains("-- <"),
        "{prompt}"
    );
    // Tests dans un worktree jetable (pas celui de la branche), avec le
    // nouveau test de la session et le harnais d'origine ; supprimé ensuite.
    let calls = tc.tests.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (tree, rust, e2e, spec) = &calls[0];
    assert!(!rust);
    assert_ne!(tree, &wt);
    assert!(
        tree.starts_with(root.join("work").join(".tests")),
        "{tree:?}"
    );
    assert_eq!(
        e2e,
        &["e2e/existing.spec.ts", "e2e/unit/nouveau.test.ts"].map(String::from)
    );
    assert_eq!(spec, "test(\"démarre\", () => {});\n");
    assert!(!tree.exists(), "worktree des tests supprimé");
    // node_modules : npm ci une fois dans le clone, vraie jonction ensuite.
    assert_eq!(*tc.installs.lock().unwrap(), std::slice::from_ref(&cfg.dir));
    assert_eq!(*host.junction_seen.lock().unwrap(), Some(true));
    // Worktree fusionné supprimé, sans toucher au node_modules du clone.
    assert!(!wt.exists(), "worktree fusionné supprimé");
    assert!(cfg.dir.join("node_modules/marker.txt").is_file());
    assert_eq!(
        sh(&cfg.dir, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1
    );
    // Clone propre.
    assert_eq!(sh(&cfg.dir, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn autocrlf_true_does_not_dirty_tree() {
    let root = TempRoot::new("crlf");
    let (cfg, _, _) = ready_clone(&root).await;
    sh(&cfg.dir, &["config", "core.autocrlf", "true"]);
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/lf.txt", "a\nb\n"), ("src/crlf.txt", "a\r\nb\r\n")],
        "fins de ligne",
    )]));
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let (out, log) = run(&p, "fins de ligne").await;
    assert!(
        matches!(out, ImproveOutcome::Merged { .. }),
        "{out:?} / {:?}",
        log.last()
    );
    assert_eq!(sh(&cfg.dir, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn file_out_of_scope_is_refused() {
    let root = TempRoot::new("scope");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/a.txt", "a\n"), ("README.md", "piraté\n")],
        "hors périmètre",
    )]));
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let req = p.request("toucher le readme").unwrap();
    let (sink, log) = collector();
    let out = p.run(req.clone(), sink).await.unwrap();
    assert_refused(&out, "README.md (hors périmètre)");
    assert_eq!(main_of(&cfg), c1);
    assert!(host.switched.lock().unwrap().is_empty());
    assert_eq!(steps(&log).last(), Some(&ImproveStep::Failed));
    assert!(
        p.worktree_path(&req.branch).exists(),
        "worktree d'un échec gardé"
    );
}

#[tokio::test]
async fn protected_harness_and_skip_markers_are_refused() {
    let root = TempRoot::new("harness");
    let (cfg, c1, _) = ready_clone(&root).await;
    let cases: Vec<(FakeHost, &str)> = vec![
        (
            success_host(vec![commit_src(
                vec![("e2e/existing.spec.ts", "// vidé\n")],
                "affaiblit",
            )]),
            "e2e/existing.spec.ts (harnais",
        ),
        (
            FakeHost {
                extra: Some(Box::new(|wt| {
                    sh(wt, &["rm", "-q", "--", "e2e/existing.spec.ts"]);
                    sh(wt, &["commit", "-q", "-m", "supprime"]);
                })),
                switch_ok: true,
                ..Default::default()
            },
            "e2e/existing.spec.ts (harnais",
        ),
        (
            success_host(vec![commit_src(
                vec![("tools/self-improve/prompt.md", "autre\n")],
                "prompt",
            )]),
            "tools/self-improve/prompt.md (harnais",
        ),
        (
            success_host(vec![commit_src(
                vec![("src-tauri/src/selfupdate/x.rs", "\n")],
                "selfupdate",
            )]),
            "src-tauri/src/selfupdate/x.rs (harnais",
        ),
        (
            success_host(vec![commit_src(
                vec![("e2e/unit/seul.test.ts", "it.only(\"x\", () => {});\n")],
                "only",
            )]),
            "« .only( »",
        ),
        (
            success_host(vec![commit_src(
                vec![("src-tauri/tests/t.rs", "#[ignore]\n#[test]\nfn t() {}\n")],
                "ignore",
            )]),
            "src-tauri/tests/t.rs : « #[ignore »",
        ),
        (
            // Code applicatif, pas seulement les fichiers de test.
            success_host(vec![commit_src(
                vec![(
                    "src/Vue.svelte",
                    "<script>it.skip(\"x\", () => {});</script>\n",
                )],
                "skip svelte",
            )]),
            "src/Vue.svelte : « .skip( »",
        ),
        (
            // Chemin avec une espace (tabulation finale dans le diff).
            success_host(vec![commit_src(
                vec![("e2e/unit/mon test.test.ts", "xit(\"x\", () => {});\n")],
                "espace",
            )]),
            "e2e/unit/mon test.test.ts : « xit( »",
        ),
        (
            success_host(vec![commit_src(
                vec![("src-tauri/Cargo.toml", "[package]\nautotests = false\n")],
                "autotests",
            )]),
            "src-tauri/Cargo.toml : « autotests »",
        ),
        (
            success_host(vec![commit_src(
                vec![("src-tauri/.cargo/config.toml", "[build]\n")],
                "cargo config",
            )]),
            "src-tauri/.cargo/config.toml (harnais",
        ),
        (
            success_host(vec![commit_src(
                vec![("src-tauri/build.rs", "fn main() {}\n")],
                "build",
            )]),
            "src-tauri/build.rs (harnais",
        ),
        (
            success_host(vec![commit_src(
                vec![("src/.gitattributes", "*.bin binary\n")],
                "attributs",
            )]),
            "src/.gitattributes (réglage git)",
        ),
    ];
    for (i, (host, needle)) in cases.into_iter().enumerate() {
        let p = pipeline(
            &root,
            cfg.clone(),
            Arc::new(host),
            Arc::new(FakeToolchain::new()),
        );
        let (out, _) = run(&p, &format!("cas {i}")).await;
        assert_refused(&out, needle);
        assert_eq!(main_of(&cfg), c1);
    }
}

/// La documentation (la spec cite les marqueurs) n'est jamais refusée pour
/// un marqueur de test désactivé : seuls les fichiers de code le sont.
#[tokio::test]
async fn skip_markers_in_docs_are_accepted() {
    let root = TempRoot::new("skipdoc");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![
            (
                "docs/regle.md",
                "Refusés : `.skip(`, `.only(`, `.fixme(`, `test.fail`, `describe.skip`, `#[ignore]`.\n",
            ),
            ("src/note.txt", "it.only(\n"),
        ],
        "documente la règle",
    )]));
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let (out, log) = run(&p, "documenter").await;
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?} / {:?}", log.last())
    };
    assert_eq!(main_of(&cfg), commit);
    assert_ne!(commit, c1);
}

#[tokio::test]
async fn committed_symlink_is_refused() {
    let root = TempRoot::new("symlink");
    let (cfg, _, _) = ready_clone(&root).await;
    // Lien extrait en fichier ordinaire (pas de privilège de lien requis).
    sh(&cfg.dir, &["config", "core.symlinks", "false"]);
    let host = Arc::new(FakeHost {
        extra: Some(Box::new(|wt| {
            write(wt, "src/lien", "../README.md");
            let blob = sh(wt, &["hash-object", "-w", "src/lien"]);
            sh(
                wt,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("120000,{blob},src/lien"),
                ],
            );
            sh(wt, &["commit", "-q", "-m", "lien"]);
        })),
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(&root, cfg, host, Arc::new(FakeToolchain::new()));
    let (out, _) = run(&p, "lien").await;
    assert_refused(&out, "src/lien (lien symbolique");
}

#[tokio::test]
async fn git_config_hooks_and_deps_changes_are_refused() {
    let root = TempRoot::new("gitconf");
    let (cfg, c1, _) = ready_clone(&root).await;
    type Extra = Box<dyn Fn(&Path) + Send + Sync>;
    let cases: Vec<(Extra, &str)> = vec![
        (
            Box::new(|wt| sh(wt, &["config", "core.sshCommand", "ssh -i x"]).truncate(0)),
            ".git/config",
        ),
        (
            Box::new(|wt| {
                sh(
                    wt,
                    &["remote", "set-url", "--push", "origin", "C:/ailleurs.git"],
                )
                .truncate(0)
            }),
            ".git/config",
        ),
        (
            Box::new(|wt| {
                let common = PathBuf::from(sh(
                    wt,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                ));
                write(&common, "hooks/pre-commit", "#!/bin/sh\n");
            }),
            "hooks",
        ),
        (
            Box::new(|wt| write(wt, "node_modules/.package-lock.json", "{\"modifié\":1}")),
            "node_modules",
        ),
    ];
    for (i, (extra, needle)) in cases.into_iter().enumerate() {
        let host = Arc::new(FakeHost {
            commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
            extra: Some(extra),
            switch_ok: true,
            ..Default::default()
        });
        let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
        let (out, _) = run(&p, &format!("conf {i}")).await;
        assert_refused(&out, needle);
        assert_eq!(main_of(&cfg), c1);
        // Remise en état du clone pour le cas suivant.
        let _ = sh_out(&cfg.dir, &["config", "--unset", "core.sshCommand"]);
        sh(
            &cfg.dir,
            &["remote", "set-url", "--push", "origin", NO_PUSH_URL],
        );
        let _ = std::fs::remove_file(cfg.dir.join(".git/hooks/pre-commit"));
        let _ = std::fs::remove_file(cfg.dir.join("node_modules/.package-lock.json"));
    }
}

#[tokio::test]
async fn dirty_tree_and_no_commit_are_refused_and_kept() {
    let root = TempRoot::new("dirty");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        dirty: Some(("src/oubli.txt", "x")),
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let dirty_req = p.request("sale").unwrap();
    let (sink, _) = collector();
    let out = p.run(dirty_req.clone(), sink).await.unwrap();
    assert_refused(&out, "arbre non propre");
    assert!(format!("{out:?}").contains("src/oubli.txt"));

    // Jalon J16 (décision 1) : un tour sans commit ne lance pas les tests et
    // attend l'utilisateur ; ici, il annule.
    let tc = Arc::new(FakeToolchain::new());
    let p = Arc::new(pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![])),
        tc.clone(),
    ));
    let empty_req = p.request("rien").unwrap();
    let (sink, log) = collector();
    let task = {
        let (p, req) = (p.clone(), empty_req.clone());
        tokio::spawn(async move { p.run(req, sink).await })
    };
    assert!(
        wait_until(&log, 600, |v| last_step(v)
            == Some(ImproveStep::AwaitingUser))
        .await,
        "{:?}",
        steps(&log)
    );
    let last = log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.waiting, Some(ImproveWaiting::NoNewCommit));
    assert!(tc.tests.lock().unwrap().is_empty(), "pas de tests");
    p.cancel(&empty_req.id).await.unwrap();
    let out = task.await.unwrap().unwrap();
    assert_eq!(out, ImproveOutcome::Cancelled);
    assert_eq!(main_of(&cfg), c1);

    // Nettoyage : la branche sans commit est « fusionnée » (égale à main)
    // mais n'est pas supprimée ; l'arbre non propre non plus.
    assert_eq!(p.cleanup_merged().await, 0);
    assert!(p.worktree_path(&empty_req.branch).exists());
    assert!(p
        .worktree_path(&dirty_req.branch)
        .join("src/oubli.txt")
        .exists());
}

#[tokio::test]
async fn cleanup_removes_only_clean_merged_branches_with_commits() {
    let root = TempRoot::new("cleanup");
    let (cfg, _, _) = ready_clone(&root).await;
    let p = pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    );
    // Deux worktrees restés après une fusion (arrêt brutal simulé).
    let mut wts = Vec::new();
    for name in ["auto/20261117-1200-reste", "auto/20261117-1201-sale"] {
        let wt = p.worktree_path(name);
        sh(
            &cfg.dir,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                name,
                &wt.to_string_lossy(),
                "main",
            ],
        );
        let file = format!("src/{}.txt", name.rsplit('-').next().unwrap());
        write(&wt, &file, "x\n");
        sh(&wt, &["add", "--", &file]);
        sh(&wt, &["commit", "-q", "-m", name, "--", &file]);
        sh(&cfg.dir, &["merge", "--ff-only", "-q", name]);
        wts.push(wt);
    }
    write(&wts[1], "src/non-commite.txt", "x");
    assert_eq!(p.cleanup_merged().await, 1);
    assert!(!wts[0].exists(), "fusionnée et propre : supprimée");
    assert!(wts[1].exists(), "non propre : gardée");
}

#[tokio::test]
async fn failing_tests_keep_main() {
    let root = TempRoot::new("tests");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/a.txt", "a\n")],
        "a",
    )]));
    let mut tc = FakeToolchain::new();
    tc.tests_ok = false;
    let p = pipeline(&root, cfg.clone(), host.clone(), Arc::new(tc));
    let (out, _) = run(&p, "tests rouges").await;
    match out {
        ImproveOutcome::Failed {
            step: ImproveStep::Testing,
            reason,
        } => assert!(reason.contains("1 test en échec"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(main_of(&cfg), c1);
    assert!(host.switched.lock().unwrap().is_empty());
    assert!(!root
        .join("work")
        .join(".tests")
        .read_dir()
        .unwrap()
        .any(|_| true));
}

#[tokio::test]
async fn failed_switch_leaves_main_and_branch_unmerged() {
    let root = TempRoot::new("switch");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(
            vec![("src/app.txt", "cassé\n")],
            "casse l'interface",
        )],
        switch_ok: false,
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let req = p.request("interface cassée").unwrap();
    let (sink, log) = collector();
    match p.run(req.clone(), sink).await.unwrap() {
        ImproveOutcome::Failed {
            step: ImproveStep::Switching,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(main_of(&cfg), c1, "main inchangé");
    assert_eq!(host.switched.lock().unwrap().len(), 1);
    let unmerged = sh(&cfg.dir, &["branch", "--no-merged", "main"]);
    assert!(
        unmerged
            .lines()
            .any(|l| l.trim_start_matches(['*', '+', ' ']) == req.branch),
        "{unmerged}"
    );
    assert_eq!(steps(&log).last(), Some(&ImproveStep::Failed));
    // Worktree d'un échec gardé, même après le nettoyage.
    assert_eq!(p.cleanup_merged().await, 0);
    assert!(p.worktree_path(&req.branch).join("src/app.txt").is_file());
    assert!(cfg.dir.join("node_modules/marker.txt").is_file());
}

/// Avance `main` du clone d'un commit (changement concurrent simulé).
fn move_main(clone: &Path) {
    write(clone, "docs/concurrent.md", "x\n");
    sh(clone, &["add", "--", "docs/concurrent.md"]);
    sh(
        clone,
        &[
            "commit",
            "-q",
            "-m",
            "concurrent",
            "--",
            "docs/concurrent.md",
        ],
    );
}

#[tokio::test]
async fn main_moved_before_switch_prevents_activation() {
    let root = TempRoot::new("premerge");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/a.txt", "a\n")],
        "a",
    )]));
    let mut tc = FakeToolchain::new();
    let clone = cfg.dir.clone();
    tc.on_build = Some(Box::new(move |_| move_main(&clone)));
    let p = pipeline(&root, cfg.clone(), host.clone(), Arc::new(tc));
    let (out, _) = run(&p, "concurrent").await;
    match out {
        ImproveOutcome::Failed {
            step: ImproveStep::Switching,
            reason,
        } => assert!(reason.contains("fusion impossible"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(host.switched.lock().unwrap().is_empty(), "jamais activée");
}

#[tokio::test]
async fn merge_failing_after_switch_rolls_ui_back() {
    let root = TempRoot::new("postmerge");
    let (cfg, _, _) = ready_clone(&root).await;
    let clone = cfg.dir.clone();
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        switch_ok: true,
        on_switch: Some(Box::new(move |_| move_main(&clone))),
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let (out, _) = run(&p, "fusion ratée").await;
    match out {
        ImproveOutcome::Failed {
            step: ImproveStep::Merged,
            reason,
        } => assert!(reason.contains("interface précédente rétablie"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(host.switched.lock().unwrap().len(), 1);
    assert_eq!(host.rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(
        sh(&cfg.dir, &["log", "-1", "--format=%s", "main"]),
        "concurrent"
    );
}

#[tokio::test]
async fn rust_change_is_merged_but_requires_restart() {
    let root = TempRoot::new("rust");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src-tauri/core.rs", "fn main() { println!(); }\n")],
        "cœur",
    )]));
    let tc = Arc::new(FakeToolchain::new());
    let p = pipeline(&root, cfg.clone(), host.clone(), tc.clone());
    let (out, log) = run(&p, "cœur").await;
    let ImproveOutcome::RestartRequired { commit } = out else {
        panic!("{out:?} / {:?}", log.last())
    };
    assert_eq!(main_of(&cfg), commit, "fusionnée après les tests");
    assert!(host.switched.lock().unwrap().is_empty(), "jamais activée");
    assert_eq!(tc.rust_flags(), [true], "cargo test demandé");
    assert!(!log.iter().any(|s| s.step == ImproveStep::Building));
    let last = log.last().unwrap().clone();
    assert_eq!(last.step, ImproveStep::RestartRequired);
    assert!(last.detail.unwrap().contains("src-tauri"));
}

#[tokio::test]
async fn contract_change_requires_restart() {
    let root = TempRoot::new("contract");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/a.txt", "a\n")],
        "a",
    )]));
    let mut tc = FakeToolchain::new();
    tc.contract = "ffffffffffffffff".into();
    let p = pipeline(&root, cfg.clone(), host.clone(), Arc::new(tc));
    let (out, _) = run(&p, "contrat").await;
    assert!(
        matches!(out, ImproveOutcome::RestartRequired { .. }),
        "{out:?}"
    );
    assert!(host.switched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revert_adds_revert_commit_on_top() {
    let root = TempRoot::new("revert");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(success_host(vec![commit_src(
        vec![("src/app.txt", "v1 modifiée\n")],
        "Changer le titre",
    )]));
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let (out, _) = run(&p, "titre").await;
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };

    let (sink, log) = collector();
    let out = p.revert(&commit, sink).await.unwrap();
    let ImproveOutcome::Merged { commit: reverted } = out else {
        panic!("{out:?} / {:?}", log.lock().unwrap().last())
    };
    assert_eq!(main_of(&cfg), reverted);
    assert_eq!(
        sh(&cfg.dir, &["log", "-1", "--format=%s", "main"]),
        "Revert \"Changer le titre\""
    );
    assert_eq!(sh(&cfg.dir, &["rev-parse", "main~1"]), commit);
    assert_eq!(read_text(&cfg.dir.join("src/app.txt")), "v1\n");
    let log = log.lock().unwrap().clone();
    assert!(log[0].branch.contains("-revert-"), "{}", log[0].branch);
    assert!(
        !log.iter().any(|s| s.step == ImproveStep::Working),
        "pas de session pour un revert"
    );
    assert_eq!(log.last().unwrap().step, ImproveStep::RolledBack);
    assert_eq!(host.switched.lock().unwrap().len(), 2);
    assert_eq!(host.prompts.lock().unwrap().len(), 1);
    // Commit inconnu.
    let (sink, _) = collector();
    assert!(p.revert("deadbeefdeadbeef", sink).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_stops_cycle_and_keeps_main() {
    let root = TempRoot::new("cancel");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        hang: true,
        switch_ok: true,
        ..Default::default()
    });
    let p = Arc::new(pipeline(
        &root,
        cfg.clone(),
        host,
        Arc::new(FakeToolchain::new()),
    ));
    let req = p.request("longue").unwrap();
    let (sink, log) = collector();
    let task = {
        let (p, req) = (p.clone(), req.clone());
        tokio::spawn(async move { p.run(req, sink).await })
    };
    for _ in 0..400 {
        if steps(&log).contains(&ImproveStep::Working) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(steps(&log).contains(&ImproveStep::Working));
    // Un seul cycle à la fois ; pas de nettoyage pendant un cycle.
    let (sink2, _) = collector();
    assert!(p.run(p.request("autre").unwrap(), sink2).await.is_err());
    assert_eq!(p.cleanup_merged().await, 0);
    assert!(
        p.worktree_path(&req.branch).exists(),
        "worktree actif gardé"
    );
    assert!(p.cancel("inconnu").await.is_err());
    p.cancel(&req.id).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("annulation en moins de 10 s")
        .unwrap()
        .unwrap();
    assert_eq!(out, ImproveOutcome::Cancelled);
    assert_eq!(steps(&log).last(), Some(&ImproveStep::Cancelled));
    assert_eq!(main_of(&cfg), c1);
    // Emplacement libéré ; worktree sans commit gardé par le nettoyage.
    assert!(p.cancel(&req.id).await.is_err());
    assert_eq!(p.cleanup_merged().await, 0);
    assert!(p.worktree_path(&req.branch).exists());
}

#[tokio::test]
async fn busy_e2e_port_fails_before_anything() {
    let root = TempRoot::new("port");
    let (cfg, _, _) = ready_clone(&root).await;
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let tc = NpmToolchain {
        e2e_port: port,
        ..Default::default()
    };
    let p = Pipeline::new(cfg, Arc::new(FakeHost::default()))
        .with_toolchain(Arc::new(tc))
        .with_dirs(root.join("work"), root.join("ui"));
    let req = p.request("port").unwrap();
    let (sink, _) = collector();
    match p.run(req.clone(), sink).await.unwrap() {
        ImproveOutcome::Failed {
            step: ImproveStep::Preparing,
            reason,
        } => assert!(reason.contains(&format!("port {port}")), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!p.worktree_path(&req.branch).exists());
}

// --- Processus ------------------------------------------------------------------

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

/// node qui lance un petit-enfant détaché (pid écrit dans `pidfile`), puis
/// attend `wait_ms`. `inherit` : le petit-enfant garde les tubes de sortie.
#[cfg(windows)]
fn spawner(pidfile: &Path, wait_ms: u32, inherit: bool) -> tokio::process::Command {
    use orchestrator_lib::selfupdate::git;
    let stdio = if inherit { "'inherit'" } else { "'ignore'" };
    let script = format!(
        "const c=require('child_process').spawn(process.execPath,['-e','setTimeout(()=>{{}},120000)'],\
         {{detached:true,stdio:['ignore',{stdio},{stdio}]}});\
         c.unref();require('fs').writeFileSync(process.argv[1],String(c.pid));\
         setTimeout(()=>{{}},{wait_ms});"
    );
    let mut c = git::command("node");
    c.arg("-e").arg(script).arg(pidfile);
    c
}

#[cfg(windows)]
async fn grandchild_pid(pidfile: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(pidfile) {
            if let Ok(p) = s.trim().parse() {
                return p;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pid du petit-enfant non écrit")
}

#[cfg(windows)]
#[tokio::test]
async fn no_orphan_after_timeout_or_exit() {
    use orchestrator_lib::selfupdate::git;
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("node absent : test sauté");
        return;
    }
    let root = TempRoot::new("orphan");

    // Délai dépassé : tout l'arbre est tué.
    let pidfile = root.join("timeout.pid");
    let res = git::run(
        spawner(&pidfile, 60_000, false),
        Duration::from_secs(4),
        "attente",
    )
    .await;
    assert!(res.unwrap_err().contains("délai dépassé"));
    let pid = grandchild_pid(&pidfile).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!alive(pid), "petit-enfant {pid} survivant après le délai");

    // Sortie normale avec un petit-enfant qui garde stdout : pas d'attente
    // jusqu'au délai, et le petit-enfant est tué.
    let pidfile = root.join("exit.pid");
    let t0 = std::time::Instant::now();
    let out = git::run(
        spawner(&pidfile, 500, true),
        Duration::from_secs(60),
        "sortie",
    )
    .await
    .unwrap();
    assert!(out.ok);
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
    let pid = grandchild_pid(&pidfile).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!alive(pid), "petit-enfant {pid} survivant après la sortie");
}

/// Relecture finale B1 : `.git/config` est en lecture seule pendant la
/// session, et une modification annulée avant la fin (même contenu à la
/// fin) est vue par sa date de modification : cycle refusé. L'attribut est
/// rétabli après la session.
#[tokio::test]
async fn git_config_changed_then_restored_is_refused() {
    let root = TempRoot::new("gitconf-restored");
    let (cfg, c1, _) = ready_clone(&root).await;
    let config = cfg.dir.join(".git").join("config");
    let seen_readonly = Arc::new(Mutex::new(None));
    let seen = seen_readonly.clone();
    let path = config.clone();
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        extra: Some(Box::new(move |_wt| {
            let mut perm = std::fs::metadata(&path).unwrap().permissions();
            *seen.lock().unwrap() = Some(perm.readonly());
            // Contourner la lecture seule, modifier, puis remettre le contenu.
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&path, perm).unwrap();
            let orig = std::fs::read(&path).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            std::fs::write(&path, b"[url \"x\"]\n").unwrap();
            std::fs::write(&path, &orig).unwrap();
        })),
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let (out, _) = run(&p, "config restaurée").await;
    assert_eq!(
        *seen_readonly.lock().unwrap(),
        Some(true),
        "lecture seule pendant la session"
    );
    assert_refused(&out, ".git/config");
    assert_eq!(main_of(&cfg), c1);
    assert!(
        !std::fs::metadata(&config).unwrap().permissions().readonly(),
        "attribut rétabli après la session"
    );
}

/// Relecture finale : verrou `improve-active` pendant la session, retiré
/// après ; `session_stopped` appelé avant de rendre `.git/config`
/// inscriptible ; au démarrage, un verrou resté (arrêt brutal) fait retirer
/// la lecture seule.
#[tokio::test]
async fn improve_lock_during_session_and_recovery_after_crash() {
    let root = TempRoot::new("improve-lock");
    let (cfg, _, _) = ready_clone(&root).await;
    let git_dir = cfg.dir.join(".git");
    let seen_lock = Arc::new(Mutex::new(None));
    let seen = seen_lock.clone();
    let lock_path = git_dir.join(IMPROVE_LOCK);
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        extra: Some(Box::new(move |_wt| {
            *seen.lock().unwrap() = Some(lock_path.is_file());
        })),
        switch_ok: true,
        watch_git: Some(git_dir.clone()),
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let (out, _) = run(&p, "verrou").await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert_eq!(
        *seen_lock.lock().unwrap(),
        Some(true),
        "verrou pendant la session"
    );
    assert_eq!(
        *host.stopped_seen.lock().unwrap(),
        Some((true, true)),
        "session_stopped avant la levée de la lecture seule et du verrou"
    );
    assert!(!git_dir.join(IMPROVE_LOCK).exists(), "verrou retiré");
    let config = git_dir.join("config");
    assert!(!std::fs::metadata(&config).unwrap().permissions().readonly());

    // Arrêt brutal simulé : verrou resté, config en lecture seule.
    std::fs::write(git_dir.join(IMPROVE_LOCK), "1\n").unwrap();
    let mut perm = std::fs::metadata(&config).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&config, perm).unwrap();
    assert!(recover_interrupted_session(&cfg.dir));
    assert!(!git_dir.join(IMPROVE_LOCK).exists());
    assert!(!std::fs::metadata(&config).unwrap().permissions().readonly());
    assert!(
        !recover_interrupted_session(&cfg.dir),
        "rien à faire ensuite"
    );
}

// --- Envoi des branches `auto/…` (propriétaire seulement) ------------------------
//
// Le dépôt distant est l'« origin » nu local, joint par un faux ssh
// (`GIT_SSH_COMMAND`, pour tout ce binaire de test : aucun autre test n'utilise
// ssh). Il imite GitHub : hôte `allow-…` = utilisateur avec droit d'écriture
// (`git receive-pack` local), hôte `deny-…` = refus à la connexion au service
// `receive-pack`, comme GitHub pour un utilisateur sans droit d'écriture
// (message « Permission to … denied to … »), et un fichier `<hôte>.down`
// simule une panne de réseau. Un `--dry-run` n'exécute pas le hook
// `pre-receive` : un dépôt nu au hook qui refuse ne simule donc pas l'absence
// de droit pour la vérification, seulement un refus à l'envoi lui-même.
// Chaque connexion est consignée dans `<hôte>.log`. Aucun accès à GitHub.

use orchestrator_lib::api::types::{PushResult, PushStatus};
use orchestrator_lib::selfupdate::push;

const FAKE_SSH: &str = r#"#!/bin/sh
# Faux ssh des tests d'envoi : `[options…] <hôte> <commande>`.
[ "$1" = "-G" ] && exit 1
all="$*"
while [ $# -gt 2 ]; do shift; done
host="${1#*@}"
cmd="$2"
dir="${0%/*}"
printf '%s\n' "$cmd" >> "$dir/$host.log"
printf '%s\n' "$all" >> "$dir/$host.args"
if [ -e "$dir/$host.down" ]; then
  echo "ssh: connect to host $host port 22: Connection timed out" >&2
  exit 255
fi
# Le « serveur » ne voit pas la configuration passée au client (`git -c`).
unset GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_DIR
case "$host" in
  allow-*) eval "git ${cmd#git-}" ;;
  deny-*)
    echo "ERROR: Permission to R3C-0N/Orchestrator-App.git denied to autre-utilisateur." >&2
    exit 1 ;;
  *) echo "ssh: Could not resolve hostname $host" >&2; exit 255 ;;
esac
"#;

/// Dossier du faux ssh (dans `target`, pas dans `%TEMP%`), installé une fois
/// et branché par `GIT_SSH_COMMAND` pour les commandes git ordinaires. Une
/// configuration git globale de test (`GIT_CONFIG_GLOBAL`, qui inclut celle
/// de l'utilisateur) y ajoute un `pushInsteadOf` « pirate » pour le seul
/// dépôt du test `global_git_config_cannot_redirect_the_push` : elle imite
/// un `~/.gitconfig` modifié par une session.
fn fake_ssh_dir() -> PathBuf {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-ssh");
    ONCE.call_once(|| {
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-ssh.sh");
        std::fs::write(&script, FAKE_SSH).unwrap();
        std::env::set_var("GIT_SSH_COMMAND", fake_ssh_program(&dir));
        let pid = std::process::id();
        let b1 = std::env::temp_dir().join(format!("orch-c-push-b1-{pid}"));
        let slash = |p: PathBuf| p.to_string_lossy().replace('\\', "/");
        let user = std::env::var_os("USERPROFILE")
            .map(|h| slash(PathBuf::from(h).join(".gitconfig")))
            .unwrap_or_default();
        let global = format!(
            "[include]\n\tpath = {user}\n[url \"git@allow-pirate-{pid}:{}\"]\n\tpushInsteadOf = git@allow-b1-{pid}:{}\n",
            slash(b1.join("pirate.git")),
            slash(b1.join("origin.git")),
        );
        let gpath = dir.join(format!("global-{pid}.gitconfig"));
        std::fs::write(&gpath, global).unwrap();
        std::env::set_var("GIT_CONFIG_GLOBAL", &gpath);
    });
    dir
}

/// Chemin du faux ssh, pour `GIT_SSH_COMMAND` ou [`Pipeline::with_ssh`].
fn fake_ssh_program(dir: &Path) -> String {
    dir.join("fake-ssh.sh").to_string_lossy().replace('\\', "/")
}

/// Arguments reçus par le faux ssh pour `host` (une ligne par connexion).
fn ssh_args(host: &str) -> Vec<String> {
    std::fs::read_to_string(fake_ssh_dir().join(format!("{host}.args")))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// Hôte `<mode>-<nom>-<pid>` du faux ssh, journal remis à zéro.
fn fake_host(mode: &str, name: &str) -> String {
    let dir = fake_ssh_dir();
    let host = format!("{mode}-{name}-{}", std::process::id());
    let _ = std::fs::remove_file(dir.join(format!("{host}.log")));
    let _ = std::fs::remove_file(dir.join(format!("{host}.down")));
    let _ = std::fs::remove_file(dir.join(format!("{host}.args")));
    host
}

/// Connexions reçues par le faux ssh pour `host`.
fn ssh_log(host: &str) -> Vec<String> {
    std::fs::read_to_string(fake_ssh_dir().join(format!("{host}.log")))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn set_down(host: &str, down: bool) {
    let f = fake_ssh_dir().join(format!("{host}.down"));
    if down {
        std::fs::write(f, "").unwrap();
    } else {
        let _ = std::fs::remove_file(f);
    }
}

/// URL ssh (forme `git@hôte:chemin`) de l'origin nu local.
fn ssh_url(host: &str, bare: &str) -> String {
    format!("git@{host}:{}", bare.replace('\\', "/"))
}

/// Références de l'origin nu : `(nom, commit)` triées.
fn remote_refs(bare: &str) -> Vec<(String, String)> {
    let out = sh(
        Path::new(bare),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let mut v: Vec<(String, String)> = out
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    v.sort();
    v
}

/// Clone prêt dont l'URL (celle de l'envoi, et `remote.origin.url` du
/// clone) passe par le faux ssh `host`.
async fn push_clone(root: &Path, host: &str) -> (RepoConfig, String, String, String) {
    let (mut cfg, c1, c2) = ready_clone(root).await;
    let bare = cfg.url.clone();
    cfg.url = ssh_url(host, &bare);
    sh(&cfg.dir, &["remote", "set-url", "origin", &cfg.url]);
    (cfg, bare, c1, c2)
}

/// Pipeline dont le dépôt canonique (seul destinataire d'un envoi) est
/// l'URL ssh du clone de test.
fn push_pipeline(
    root: &Path,
    cfg: RepoConfig,
    host: Arc<FakeHost>,
    tc: Arc<FakeToolchain>,
) -> Pipeline {
    let url = cfg.url.clone();
    pipeline(root, cfg, host, tc)
        .with_push_origin(url)
        .with_ssh(fake_ssh_program(&fake_ssh_dir()), None)
}

fn pushes(log: &[ImproveStatus]) -> Vec<Option<PushResult>> {
    log.iter().map(|s| s.push.clone()).collect()
}

#[test]
fn push_setting_and_branch_rules() {
    let root = TempRoot::new("push-setting");
    let conf = root.join("config.json");
    assert!(!push::push_requested(None));
    assert!(!push::push_requested(Some(&conf)), "fichier absent");
    for (text, want) in [
        (r#"{ "repo_url": "x" }"#, false),
        (r#"{ "push": false }"#, false),
        (r#"{ "push": "true" }"#, false),
        (r#"{ "push": 1 }"#, false),
        ("pas du json", false),
        (r#"{ "push": true }"#, true),
    ] {
        std::fs::write(&conf, text).unwrap();
        assert_eq!(push::push_requested(Some(&conf)), want, "{text}");
    }
    assert!(push::valid_branch("auto/20261117-1200-titre"));
    assert!(push::valid_branch("auto/20261117-1200-revert-abc1234-2"));
    for bad in [
        "main",
        "auto/",
        "+auto/x",
        "auto/x:refs/heads/main",
        "auto/x:main",
        "auto/../main",
        "auto/x.lock",
        "auto/.x",
        "auto//x",
        "auto/x y",
        "refs/heads/auto/x",
    ] {
        assert!(!push::valid_branch(bad), "{bad}");
    }
    assert!(push::is_denied(
        "ERROR: Permission to a/b.git denied to c.\nfatal: Could not read from remote repository."
    ));
    assert!(push::is_denied(
        "remote: Write access to repository not granted.\nfatal: unable to access 'https://github.com/a/b.git/': The requested URL returned error: 403"
    ));
    assert!(!push::is_denied(
        "ssh: connect to host github.com port 22: Connection timed out"
    ));
    assert!(push::is_denied(
        "fatal: unable to access 'https://github.com/a/b.git/': The requested URL returned error: 401"
    ));
    // Un SHA ou un identifiant qui contient 401 ou 403 n'est pas un refus.
    let sha = "40340140340140340140340140340140340140ab";
    assert!(!push::is_denied(&format!(
        "To git@github.com:a/b.git\n!\t{sha}:refs/heads/orchestrator-sonde-ecriture-401403\t[rejected] (fetch first)\nerror: failed to push some refs"
    )));
    assert!(!push::is_denied(&format!(
        "error: object {sha} is corrupt\nssh: Could not resolve hostname h403: Name or service not known"
    )));
    assert!(!push::is_denied("error: 4031 erreurs"));
    // Dépôt canonique : formes SSH strictement équivalentes seulement.
    let canon = orchestrator_lib::selfupdate::DEFAULT_ORIGIN;
    assert!(push::same_repo(canon, canon));
    assert!(push::same_repo(
        canon,
        "ssh://git@github.com/R3C-0N/Orchestrator-App.git"
    ));
    for other in [
        "https://github.com/R3C-0N/Orchestrator-App.git",
        "git@github.com:R3C-0N/Orchestrator-App",
        "git@github.com:r3c-0n/orchestrator-app.git",
        "git@github.com:autre/Orchestrator-App.git",
        "git@github.com:/R3C-0N/Orchestrator-App.git",
        "ssh://git@github.com:22/R3C-0N/Orchestrator-App.git",
        "ssh://autre@github.com/R3C-0N/Orchestrator-App.git",
        "alice@github.com:R3C-0N/Orchestrator-App.git",
        "git@gitlab.com:R3C-0N/Orchestrator-App.git",
        "git@github.com:R3C-0N/../R3C-0N/Orchestrator-App.git",
        "C:/depot/Orchestrator-App.git",
    ] {
        assert!(!push::same_repo(canon, other), "{other}");
    }
    // Réglages locaux refusés au moment de la sonde et de l'envoi.
    for bad in [
        "http.proxy",
        "http.https://github.com/.proxy",
        "http.extraheader",
        "http.https://github.com/.extraHeader",
        "http.sslverify",
        "http.sslcainfo",
        "core.askpass",
        "core.gitproxy",
        "core.sshcommand",
        "protocol.allow",
        "protocol.file.allow",
        "remote.origin.pushurl2",
        "remote.origin.receivepack",
        "remote.origin.proxy",
        "remote.evil.url",
        "remote.pushdefault",
        "url.x.insteadof",
        "includeif.gitdir:x.path",
        "credential.helper",
    ] {
        assert!(push::is_sneaky_key(bad), "{bad}");
    }
    for ok in [
        "remote.origin.url",
        "remote.origin.fetch",
        "remote.origin.pushurl",
        "core.autocrlf",
        "user.email",
        "http.postbuffer",
    ] {
        assert!(!push::is_sneaky_key(ok), "{ok}");
    }
}

/// Propriétaire avec droit d'écriture : vérification sans rien écrire, puis
/// seule la branche `auto/…` est envoyée ; `main` distant inchangé ; le
/// clone ne pousse toujours pas de lui-même.
#[tokio::test]
async fn owner_with_write_access_pushes_auto_branch_only() {
    let root = TempRoot::new("push-owner");
    let host = fake_host("allow", "owner");
    let (cfg, bare, c1, c2) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    let ship = Arc::new(success_host(vec![commit_src(
        vec![("src/a.txt", "a\n")],
        "ajoute a",
    )]));
    let p = push_pipeline(&root, cfg.clone(), ship, Arc::new(FakeToolchain::new()));
    assert_eq!(p.push_status(), PushStatus::Off);

    let st = p.enable_push(true).await;
    assert_eq!(
        st,
        PushStatus::Enabled {
            url: cfg.url.clone()
        }
    );
    assert_eq!(p.push_status(), st);
    // La sonde (`--dry-run`) a joint `receive-pack` sans rien écrire.
    let log = ssh_log(&host);
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("git-receive-pack"), "{log:?}");
    assert_eq!(remote_refs(&bare), before, "la sonde n'écrit rien");

    let req = p.request("ajouter a").unwrap();
    let (sink, statuses) = collector();
    let out = p.run(req.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    let statuses = statuses.lock().unwrap().clone();
    let last = statuses.last().unwrap();
    assert_eq!(last.step, ImproveStep::Merged);
    assert_eq!(
        last.push,
        Some(PushResult::Pushed {
            branch: req.branch.clone()
        })
    );
    let push_states = pushes(&statuses);
    assert_eq!(
        push_states[push_states.len() - 2],
        Some(PushResult::Pushing {
            branch: req.branch.clone()
        })
    );
    // Distant : la branche auto/… en plus, sur la tête fusionnée ; main
    // distant inchangé (c2, jamais le main du clone).
    let mut want = before.clone();
    want.push((format!("refs/heads/{}", req.branch), commit.clone()));
    want.sort();
    assert_eq!(remote_refs(&bare), want);
    assert_eq!(sh(Path::new(&bare), &["rev-parse", "refs/heads/main"]), c2);
    assert_eq!(sh(&cfg.dir, &["rev-parse", "main~1"]), c1);
    // Deux connexions de plus : nouvelle vérification puis envoi.
    let log = ssh_log(&host);
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.iter().all(|l| l.starts_with("git-receive-pack")));
    // Le clone garde son URL d'envoi `no-push://` (protection de la session).
    assert_eq!(
        sh(&cfg.dir, &["remote", "get-url", "--push", "origin"]),
        NO_PUSH_URL
    );
    assert!(p.pending_pushes().is_empty());
}

/// Sans droit d'écriture : envoi désactivé avec une raison, aucune tentative
/// d'envoi pendant le cycle.
#[tokio::test]
async fn without_write_access_push_is_disabled_and_never_attempted() {
    let root = TempRoot::new("push-denied");
    let host = fake_host("deny", "denied");
    let (cfg, bare, _, _) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    match p.enable_push(true).await {
        PushStatus::Disabled { reason } => {
            assert!(reason.contains("pas de droit d'écriture"), "{reason}");
            assert!(reason.contains("Permission to"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(ssh_log(&host).len(), 1, "la seule vérification");
    let (out, statuses) = run(&p, "a").await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert!(pushes(&statuses).iter().all(Option::is_none));
    assert_eq!(ssh_log(&host).len(), 1, "aucune tentative d'envoi");
    assert_eq!(remote_refs(&bare), before);
    assert!(p.pending_pushes().is_empty());
}

/// Réglage absent : aucune connexion, aucun envoi (cas des autres
/// utilisateurs), même avec un dépôt distant inscriptible.
#[tokio::test]
async fn without_setting_nothing_is_pushed() {
    let root = TempRoot::new("push-off");
    let host = fake_host("allow", "off");
    let (cfg, bare, _, _) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert_eq!(p.enable_push(false).await, PushStatus::Off);
    let (out, statuses) = run(&p, "a").await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert!(pushes(&statuses).iter().all(Option::is_none));
    assert!(ssh_log(&host).is_empty(), "aucune connexion");
    assert_eq!(remote_refs(&bare), before);
}

/// Panne de réseau à l'envoi : fusion locale acquise, statut « envoi
/// échoué », branche en attente ; envoyée au démarrage suivant.
#[tokio::test]
async fn network_failure_keeps_local_merge_and_retries_later() {
    let root = TempRoot::new("push-down");
    let host = fake_host("allow", "down");
    let (cfg, bare, _, c2) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    set_down(&host, true);
    let req = p.request("a").unwrap();
    let (sink, statuses) = collector();
    let out = p.run(req.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    assert_eq!(main_of(&cfg), commit, "fusion locale acquise");
    let last = statuses.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.step, ImproveStep::Merged);
    match last.push {
        Some(PushResult::Failed { branch, reason }) => {
            assert_eq!(branch, req.branch);
            assert!(reason.contains("timed out"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(remote_refs(&bare), before);
    // Panne : sonde « non vérifiable », pas un refus ; refaite au cycle
    // suivant ou au démarrage.
    assert!(
        matches!(p.push_status(), PushStatus::Disabled { ref reason } if reason.contains("non vérifiable")),
        "{:?}",
        p.push_status()
    );
    assert_eq!(
        p.pending_pushes(),
        vec![push::Pending {
            branch: req.branch.clone(),
            commit: commit.clone()
        }]
    );

    // Réseau revenu, démarrage suivant : l'envoi en attente est fait.
    set_down(&host, false);
    let p2 = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    );
    assert_eq!(p2.pending_pushes().len(), 1, "liste gardée sur disque");
    assert!(matches!(
        p2.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    assert_eq!(
        sh(
            Path::new(&bare),
            &["rev-parse", &format!("refs/heads/{}", req.branch)]
        ),
        commit
    );
    assert!(p2.pending_pushes().is_empty());
    assert_eq!(sh(Path::new(&bare), &["rev-parse", "refs/heads/main"]), c2);
}

/// Jamais forcé : une branche distante du même nom, d'un autre historique,
/// n'est pas écrasée (envoi échoué, branche distante intacte) ; un refus du
/// dépôt distant à l'envoi (hook `pre-receive`) laisse aussi la fusion.
#[tokio::test]
async fn push_is_never_forced_and_remote_refusal_keeps_merge() {
    let root = TempRoot::new("push-noforce");
    let host = fake_host("allow", "noforce");
    let (cfg, bare, _, c2) = push_clone(&root, &host).await;
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let req = p.request("a").unwrap();
    let theirs = format!("refs/heads/{}", req.branch);
    sh(Path::new(&bare), &["update-ref", &theirs, &c2]);
    let (sink, statuses) = collector();
    let out = p.run(req.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    assert_eq!(main_of(&cfg), commit);
    let last = statuses.lock().unwrap().last().unwrap().clone();
    assert!(
        matches!(&last.push, Some(PushResult::Failed { .. })),
        "{:?}",
        last.push
    );
    assert_eq!(
        sh(Path::new(&bare), &["rev-parse", &theirs]),
        c2,
        "pas écrasée"
    );

    // Hook `pre-receive` qui refuse tout : la vérification (`--dry-run`)
    // passe, l'envoi est refusé, la fusion reste.
    sh(Path::new(&bare), &["update-ref", "-d", &theirs]);
    write(
        Path::new(&bare),
        "hooks/pre-receive",
        "#!/bin/sh\necho \"refusé par le dépôt distant\" >&2\nexit 1\n",
    );
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/b.txt", "b\n")],
            "b",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let (out, statuses) = run(&p, "b").await;
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    assert_eq!(main_of(&cfg), commit);
    let last = statuses.last().unwrap();
    assert!(
        matches!(&last.push, Some(PushResult::Failed { reason, .. }) if reason.contains("pre-receive")),
        "{:?}",
        last.push
    );
    assert_eq!(
        remote_refs(&bare)
            .iter()
            .filter(|(r, _)| r.starts_with("refs/heads/auto/"))
            .count(),
        0
    );
    assert_eq!(p.pending_pushes().len(), 2, "les deux branches en attente");
}

/// Retour arrière et « relance requise » : leur branche `auto/…` est
/// envoyée aussi, une fois fusionnée.
#[tokio::test]
async fn revert_and_restart_required_are_pushed() {
    let root = TempRoot::new("push-revert");
    let host = fake_host("allow", "revert");
    let (cfg, bare, _, c2) = push_clone(&root, &host).await;
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/app.txt", "v1 modifiée\n")],
            "Changer le titre",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let (out, _) = run(&p, "titre").await;
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    let (sink, statuses) = collector();
    let out = p.revert(&commit, sink).await.unwrap();
    let ImproveOutcome::Merged { commit: reverted } = out else {
        panic!("{out:?}")
    };
    let last = statuses.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.step, ImproveStep::RolledBack);
    let Some(PushResult::Pushed { branch }) = last.push else {
        panic!("{:?}", last.push)
    };
    assert!(branch.contains("-revert-"), "{branch}");
    assert_eq!(
        sh(
            Path::new(&bare),
            &["rev-parse", &format!("refs/heads/{branch}")]
        ),
        reverted
    );

    // Cœur Rust modifié : fusionnée sans activation, branche envoyée.
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src-tauri/core.rs", "fn main() { println!(); }\n")],
            "cœur",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let (out, statuses) = run(&p, "cœur").await;
    let ImproveOutcome::RestartRequired { commit } = out else {
        panic!("{out:?}")
    };
    let last = statuses.last().unwrap();
    assert_eq!(last.step, ImproveStep::RestartRequired);
    let Some(PushResult::Pushed { branch }) = &last.push else {
        panic!("{:?}", last.push)
    };
    assert_eq!(
        sh(
            Path::new(&bare),
            &["rev-parse", &format!("refs/heads/{branch}")]
        ),
        commit
    );
    assert_eq!(sh(Path::new(&bare), &["rev-parse", "refs/heads/main"]), c2);
}

/// `push_branch` refuse ce qui n'est pas une branche `auto/…` fusionnée,
/// sans rien envoyer.
#[tokio::test]
async fn push_branch_refuses_main_unmerged_and_bad_refspecs() {
    let root = TempRoot::new("push-refuse");
    let host = fake_host("allow", "refuse");
    let (cfg, bare, c1, c2) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    let t = Duration::from_secs(60);
    let env = push::PushEnv {
        dir: root.join("push-env"),
        git_program: "git".into(),
        ssh_program: fake_ssh_program(&fake_ssh_dir()),
        host: host.clone(),
        identities: Vec::new(),
    };
    for (branch, commit) in [
        ("main", c1.as_str()),
        ("auto/x:refs/heads/main", c1.as_str()),
        ("+auto/x", c1.as_str()),
        ("auto/x", "HEAD"),
        // c2 n'est pas dans le main du clone (placé sur c1).
        ("auto/x", c2.as_str()),
    ] {
        let r = push::push_branch(&cfg.dir, &cfg.url, &cfg.url, &env, branch, commit, t).await;
        assert!(r.is_err(), "{branch} {commit}");
    }
    assert!(ssh_log(&host).is_empty(), "rien n'a joint le distant");
    assert_eq!(remote_refs(&bare), before);
}

/// Envoi réservé au dépôt canonique : un `repo_url` d'un autre dépôt (fork,
/// miroir) ou une URL d'origine du clone différente désactive l'envoi, sans
/// connexion ni mise en attente ; le cycle fusionne quand même.
#[tokio::test]
async fn push_only_to_canonical_repository() {
    let root = TempRoot::new("push-fork");
    let host = fake_host("allow", "fork");
    let (cfg, bare, _, _) = push_clone(&root, &host).await;
    let before = remote_refs(&bare);
    // Dépôt canonique par défaut (GitHub) : l'URL de test est un « fork ».
    let p = pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    match p.enable_push(true).await {
        PushStatus::Disabled { reason } => {
            assert!(reason.contains("envoi réservé au dépôt"), "{reason}");
            assert!(
                reason.contains(orchestrator_lib::selfupdate::DEFAULT_ORIGIN),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    let (out, statuses) = run(&p, "a").await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert!(pushes(&statuses).iter().all(Option::is_none));
    assert!(ssh_log(&host).is_empty(), "aucune connexion");
    assert!(p.pending_pushes().is_empty());

    // `repo_url` canonique, mais l'origine du clone pointe ailleurs.
    sh(
        &cfg.dir,
        &[
            "remote",
            "set-url",
            "origin",
            &ssh_url("allow-ailleurs", &bare),
        ],
    );
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    );
    match p.enable_push(true).await {
        PushStatus::Disabled { reason } => {
            assert!(reason.contains("URL d'origine du clone"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert!(ssh_log(&host).is_empty());
    assert_eq!(remote_refs(&bare), before);
}

/// Réglage git douteux dans le clone (`http.proxy`, autre dépôt distant) :
/// la sonde est refusée avant toute connexion.
#[tokio::test]
async fn sneaky_clone_config_blocks_the_probe() {
    let root = TempRoot::new("push-sneaky");
    let host = fake_host("allow", "sneaky");
    let (cfg, _, _, _) = push_clone(&root, &host).await;
    for (key, value) in [
        ("http.proxy", "http://127.0.0.1:9"),
        ("remote.evil.url", "git@allow-evil:x.git"),
        ("core.askPass", "calc.exe"),
    ] {
        sh(&cfg.dir, &["config", "--local", key, value]);
        let p = push_pipeline(
            &root,
            cfg.clone(),
            Arc::new(FakeHost::default()),
            Arc::new(FakeToolchain::new()),
        );
        match p.enable_push(true).await {
            PushStatus::Disabled { reason } => {
                assert!(reason.contains("réglage git refusé"), "{key} : {reason}")
            }
            other => panic!("{key} : {other:?}"),
        }
        sh(&cfg.dir, &["config", "--local", "--unset", key]);
    }
    assert!(ssh_log(&host).is_empty(), "aucune connexion");
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
}

/// Sonde impossible au démarrage (réseau) : la fusion du cycle est mise en
/// attente, et la sonde est refaite au cycle suivant, qui envoie les deux
/// branches.
#[tokio::test]
async fn unavailable_probe_queues_merge_and_is_retried_next_cycle() {
    let root = TempRoot::new("push-unavail");
    let host = fake_host("allow", "unavail");
    let (cfg, bare, _, c2) = push_clone(&root, &host).await;
    set_down(&host, true);
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    match p.enable_push(true).await {
        PushStatus::Disabled { reason } => assert!(reason.contains("non vérifiable"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let req1 = p.request("a").unwrap();
    let (sink, statuses) = collector();
    let out = p.run(req1.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit: c_a } = out else {
        panic!("{out:?}")
    };
    let last = statuses.lock().unwrap().last().unwrap().clone();
    assert!(
        matches!(&last.push, Some(PushResult::Failed { branch, .. }) if *branch == req1.branch),
        "{:?}",
        last.push
    );
    assert_eq!(p.pending_pushes().len(), 1, "fusion mise en attente");

    // Le faux hôte de session ne rejoue qu'une liste fixe de commits : un
    // second cycle demande un nouveau pipeline, remis dans le même état
    // (liste en attente relue, sonde impossible) avant le cycle.
    let p = {
        drop(p);
        push_pipeline(
            &root,
            cfg.clone(),
            Arc::new(success_host(vec![commit_src(
                vec![("src/b.txt", "b\n")],
                "b",
            )])),
            Arc::new(FakeToolchain::new()),
        )
    };
    set_down(&host, true);
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Disabled { .. }
    ));
    // Réseau revenu : le cycle suivant refait la sonde et envoie tout.
    set_down(&host, false);
    let (out, statuses) = run(&p, "b").await;
    let ImproveOutcome::Merged { commit: c_b } = out else {
        panic!("{out:?}")
    };
    let last = statuses.last().unwrap();
    assert!(
        matches!(&last.push, Some(PushResult::Pushed { .. })),
        "{:?}",
        last.push
    );
    assert!(matches!(p.push_status(), PushStatus::Enabled { .. }));
    assert!(p.pending_pushes().is_empty());
    let refs = remote_refs(&bare);
    assert!(refs.contains(&(format!("refs/heads/{}", req1.branch), c_a)));
    assert!(refs
        .iter()
        .any(|(r, c)| r.starts_with("refs/heads/auto/") && *c == c_b));
    assert_eq!(sh(Path::new(&bare), &["rev-parse", "refs/heads/main"]), c2);
}

/// `config.json` figé au lancement : une session qui le modifie fait
/// refuser le cycle ; `push-pending.json` : une entrée ajoutée par la
/// session fait refuser le cycle, n'est jamais envoyée et le fichier est
/// rétabli depuis la mémoire.
#[tokio::test]
async fn session_cannot_touch_config_json_or_pending_list() {
    let root = TempRoot::new("push-files");
    let (cfg, c1, _) = ready_clone(&root).await;
    let conf = root.join("config.json");
    std::fs::write(&conf, r#"{ "improve_model": "haiku" }"#).unwrap();
    let path = conf.clone();
    let p = pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost {
            commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
            extra: Some(Box::new(move |_wt| {
                std::fs::write(&path, r#"{ "improve_model": "haiku", "push": true }"#).unwrap();
            })),
            switch_ok: true,
            ..Default::default()
        }),
        Arc::new(FakeToolchain::new()),
    );
    let (out, _) = run(&p, "config").await;
    assert_refused(&out, "config.json");
    assert_eq!(main_of(&cfg), c1);

    std::fs::write(&conf, r#"{ "improve_model": "haiku" }"#).unwrap();
    let pending = root.join("work").join(push::PENDING_FILE);
    let forged = pending.clone();
    let p = pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost {
            commits: vec![commit_src(vec![("src/b.txt", "b\n")], "b")],
            extra: Some(Box::new(move |_wt| {
                std::fs::create_dir_all(forged.parent().unwrap()).unwrap();
                std::fs::write(
                    &forged,
                    r#"[{ "branch": "auto/pirate", "commit": "0123456789012345678901234567890123456789" }]"#,
                )
                .unwrap();
            })),
            switch_ok: true,
            ..Default::default()
        }),
        Arc::new(FakeToolchain::new()),
    );
    let (out, _) = run(&p, "liste").await;
    assert_refused(&out, "push-pending.json");
    assert_eq!(main_of(&cfg), c1);
    assert!(p.pending_pushes().is_empty(), "entrée jamais retenue");
    assert!(
        !pending.exists(),
        "fichier rétabli depuis la mémoire (vide)"
    );
}

/// B1 : une configuration git globale modifiée (`pushInsteadOf` vers un
/// dépôt « pirate ») détourne une commande git ordinaire (contrôle), mais
/// ni la sonde ni l'envoi : ils ne lisent ni `~/.gitconfig` ni la
/// configuration système. Le dépôt pirate ne reçoit que le contrôle.
#[tokio::test]
async fn global_git_config_cannot_redirect_the_push() {
    let root = TempRoot::new("push-b1");
    let host = fake_host("allow", "b1");
    let pirate_host = fake_host("allow", "pirate");
    let (cfg, bare, c1, _) = push_clone(&root, &host).await;
    let pirate = root.join("pirate.git");
    sh(&root, &["init", "-q", "--bare", &pirate.to_string_lossy()]);
    let pirate_s = pirate.to_string_lossy().into_owned();

    // Contrôle : `git push` ordinaire (environnement hérité) part au pirate.
    let control = format!("{c1}:refs/heads/controle");
    let out = orchestrator_lib::selfupdate::git::run(
        orchestrator_lib::selfupdate::git::git_command(
            &cfg.dir,
            [
                "push",
                "--porcelain",
                "--",
                cfg.url.as_str(),
                control.as_str(),
            ],
        ),
        Duration::from_secs(60),
        "contrôle",
    )
    .await
    .unwrap();
    assert!(out.ok, "{}", out.tail(6));
    assert_eq!(
        remote_refs(&pirate_s),
        vec![("refs/heads/controle".to_string(), c1.clone())],
        "le contrôle doit être détourné (sinon le test ne prouve rien)"
    );
    assert_eq!(ssh_log(&pirate_host).len(), 1);
    let before = remote_refs(&bare);

    // Sonde et envoi du pipeline : vers le bon dépôt seulement.
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![commit_src(
            vec![("src/a.txt", "a\n")],
            "a",
        )])),
        Arc::new(FakeToolchain::new()),
    );
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let req = p.request("a").unwrap();
    let (sink, statuses) = collector();
    let out = p.run(req.clone(), sink).await.unwrap();
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    let last = statuses.lock().unwrap().last().unwrap().clone();
    assert!(
        matches!(last.push, Some(PushResult::Pushed { .. })),
        "{:?}",
        last.push
    );
    let mut want = before.clone();
    want.push((format!("refs/heads/{}", req.branch), commit));
    want.sort();
    assert_eq!(remote_refs(&bare), want, "envoi au bon dépôt");
    assert_eq!(
        remote_refs(&pirate_s),
        vec![("refs/heads/controle".to_string(), c1)],
        "rien d'autre chez le pirate"
    );
    assert_eq!(
        ssh_log(&pirate_host).len(),
        1,
        "aucune connexion de l'envoi"
    );
}

/// B2 : ssh de l'envoi lancé sans `~/.ssh/config` (`-F` fichier vide), avec
/// serveur, utilisateur, port imposés, sans mandataire, clé d'hôte
/// vérifiée ; les `IdentityFile` relevés au lancement sont passés.
#[tokio::test]
async fn push_ssh_ignores_user_ssh_config_but_keeps_identity() {
    let root = TempRoot::new("push-b2");
    let host = fake_host("allow", "b2");
    let (cfg, _, _, _) = push_clone(&root, &host).await;
    // Chemin avec une espace (profil « Jean Dupont »).
    let key = root.join("Jean Dupont").join("id_test");
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, "clé de test").unwrap();
    let key_s = key.to_string_lossy().replace('\\', "/");
    let ssh_config = format!(
        "Host autre\n  IdentityFile {}/autre\nHost allow-*\n  HostName pirate.example\n  ProxyCommand calc.exe\n  IdentityFile \"{key_s}\"\n",
        root.to_string_lossy().replace('\\', "/")
    );
    let p = push_pipeline(
        &root,
        cfg.clone(),
        Arc::new(FakeHost::default()),
        Arc::new(FakeToolchain::new()),
    )
    .with_ssh(fake_ssh_program(&fake_ssh_dir()), Some(ssh_config));
    assert!(matches!(
        p.enable_push(true).await,
        PushStatus::Enabled { .. }
    ));
    let args = ssh_args(&host);
    assert_eq!(args.len(), 1, "{args:?}");
    let a = &args[0];
    let empty = root
        .join("work")
        .join(".push-env")
        .join("empty.ssh_config")
        .to_string_lossy()
        .replace('\\', "/");
    assert!(a.contains(&format!("-F {empty}")), "{a}");
    for o in [
        format!("HostName={host}"),
        "User=git".into(),
        "Port=22".into(),
        "ProxyCommand=none".into(),
        "ProxyJump=none".into(),
        "BatchMode=yes".into(),
        "StrictHostKeyChecking=yes".into(),
        "IdentitiesOnly=yes".into(),
        format!("IdentityFile=\"{key_s}\""),
    ] {
        assert!(a.contains(&format!("-o {o}")), "{o} absent de : {a}");
    }
    assert!(
        !a.contains("pirate.example") && !a.contains("calc.exe"),
        "{a}"
    );
    assert!(!a.contains("/autre"), "{a}");
    assert_eq!(
        std::fs::read(root.join("work/.push-env/empty.ssh_config")).unwrap(),
        b""
    );
}

#[test]
fn ssh_identities_from_user_config() {
    let root = TempRoot::new("ssh-ids");
    let home = root.to_path_buf();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    for f in ["global", "gh", "star", "tilde", "pct"] {
        std::fs::write(home.join(".ssh").join(f), "").unwrap();
    }
    let h = home.to_string_lossy().replace('\\', "/");
    let config = format!(
        "IdentityFile {h}/.ssh/global\n\
         Host github.com\n  IdentityFile={h}/.ssh/gh\n  IdentityFile {h}/.ssh/absent\n\
         Host *.com !gitlab.com\n  IdentityFile \"{h}/.ssh/star\"\n\
         Host gitlab.com\n  IdentityFile {h}/.ssh/autre\n\
         Match exec \"calc.exe\"\n  IdentityFile {h}/.ssh/pct\n\
         Host GITHUB.COM\n  IdentityFile ~/.ssh/tilde\n  IdentityFile %d/.ssh/pct\n  IdentityFile ~/.ssh/%h\n"
    );
    let ids = push::ssh_identities(&config, "github.com", &home);
    let names: Vec<String> = ids
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["global", "gh", "star", "tilde", "pct"]);
    let ids = push::ssh_identities(&config, "gitlab.com", &home);
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(push::ssh_identities("", "github.com", &home).is_empty());
}

/// C1 et C3 : les fichiers de l'app sont rétablis même si la session échoue
/// ; un `push-pending.json` illisible au démarrage est réécrit tout de
/// suite (pas de refus à tort). C2 : `config.json` déjà changé avant la
/// session : échec immédiat, sans session.
#[tokio::test]
async fn app_files_restored_on_failed_session_and_checked_before() {
    let root = TempRoot::new("push-files2");
    let (cfg, c1, _) = ready_clone(&root).await;
    let pending = root.join("work").join(push::PENDING_FILE);
    std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
    std::fs::write(&pending, "pas du json").unwrap();
    let forged = pending.clone();
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        extra: Some(Box::new(move |_wt| {
            std::fs::write(
                &forged,
                r#"[{ "branch": "auto/pirate", "commit": "0123456789012345678901234567890123456789" }]"#,
            )
            .unwrap();
        })),
        fail: true,
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    assert!(!pending.exists(), "fichier illisible réécrit au démarrage");
    let (out, _) = run(&p, "liste").await;
    assert_refused(&out, "push-pending.json");
    assert_refused(&out, "session simulée en échec");
    assert!(!pending.exists(), "rétabli");
    assert!(p.pending_pushes().is_empty());
    assert_eq!(main_of(&cfg), c1);

    // config.json changé avant le cycle : refus avant la session.
    std::fs::write(root.join("config.json"), "{}").unwrap();
    let prompts = host.prompts.lock().unwrap().len();
    let (out, _) = run(&p, "config").await;
    match out {
        ImproveOutcome::Failed {
            step: ImproveStep::Preparing,
            reason,
        } => assert!(reason.contains("config.json"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        host.prompts.lock().unwrap().len(),
        prompts,
        "pas de session"
    );
}

// --- Jalon J16 : conversation avec la session, tours successifs ---------------
//
// `TurnHost` simule une session à plusieurs tours (`run_turn`) : chaque tour
// joue le `Turn` suivant de son script (un commit, une action libre, puis une
// fin) ; script épuisé : tour sans commit. Les lignes d'état (`note`) et les
// messages reçus sont consignés.

/// Fin d'un tour simulé.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TurnScript {
    Done,
    /// La session s'arrête avant la fin de son tour (erreur de l'hôte).
    Fail,
    /// Le tour dure jusqu'à `interrupt_turn` (fin `Interrupted`).
    UntilInterrupt,
}

/// Un tour de la session simulée.
struct Turn {
    files: Vec<(&'static str, &'static str)>,
    extra: Hook,
    end: TurnScript,
}

fn turn(files: Vec<(&'static str, &'static str)>) -> Turn {
    Turn {
        files,
        extra: None,
        end: TurnScript::Done,
    }
}

fn turn_end(end: TurnScript) -> Turn {
    Turn {
        files: Vec::new(),
        extra: None,
        end,
    }
}

#[derive(Default)]
struct TurnHost {
    turns: Mutex<VecDeque<Turn>>,
    /// (message, resume) de chaque `run_turn`.
    calls: Mutex<Vec<(String, bool)>>,
    cwds: Mutex<Vec<PathBuf>>,
    notes: Mutex<Vec<(NoticeLevel, String, String)>>,
    /// Appels de `session_stopped` (un par tour).
    stopped: AtomicUsize,
    /// Instant de chaque début de tour.
    started: Mutex<Vec<Instant>>,
    interrupt: tokio::sync::Notify,
    switch_ok: bool,
    /// Porte armée dans `session_stopped` (après le tour, avant la garde) et
    /// dans `wait_idle` (entre les tests et la bascule) : signale `reached`
    /// puis attend `release`, une fois chacune.
    gate_stopped: std::sync::atomic::AtomicBool,
    gate_idle: std::sync::atomic::AtomicBool,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl TurnHost {
    fn new(turns: Vec<Turn>) -> Self {
        TurnHost {
            turns: Mutex::new(turns.into()),
            switch_ok: true,
            ..Default::default()
        }
    }

    fn calls(&self) -> Vec<(String, bool)> {
        self.calls.lock().unwrap().clone()
    }

    fn notes_with(&self, code: &str) -> Vec<String> {
        self.notes
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.1 == code)
            .map(|n| n.2.clone())
            .collect()
    }
}

impl PipelineHost for TurnHost {
    fn run_session<'a>(
        &'a self,
        _request: &'a ImproveRequest,
        _cwd: &'a Path,
        _prompt: &'a str,
    ) -> BoxFut<'a, Result<(), CoreError>> {
        Box::pin(async { panic!("le pipeline J16 passe par run_turn") })
    }

    fn run_turn<'a>(
        &'a self,
        _request: &'a ImproveRequest,
        cwd: &'a Path,
        message: &'a str,
        resume: bool,
    ) -> BoxFut<'a, Result<TurnEnd, CoreError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((message.to_string(), resume));
            self.cwds.lock().unwrap().push(cwd.to_path_buf());
            self.started.lock().unwrap().push(Instant::now());
            let t = self.turns.lock().unwrap().pop_front();
            let Some(t) = t else {
                return Ok(TurnEnd::Done);
            };
            if !t.files.is_empty() {
                let names: Vec<&str> = t.files.iter().map(|f| f.0).collect();
                for (rel, content) in &t.files {
                    write(cwd, rel, content);
                }
                let mut args = vec!["add", "--"];
                args.extend(&names);
                sh(cwd, &args);
                let mut args = vec!["commit", "-q", "-m", "tour", "--"];
                args.extend(&names);
                sh(cwd, &args);
            }
            if let Some(f) = &t.extra {
                f(cwd);
            }
            match t.end {
                TurnScript::Done => Ok(TurnEnd::Done),
                TurnScript::Fail => Err(CoreError::Io(
                    "la session dédiée s'est arrêtée avant la fin de son tour".into(),
                )),
                TurnScript::UntilInterrupt => {
                    self.interrupt.notified().await;
                    Ok(TurnEnd::Interrupted)
                }
            }
        })
    }

    fn interrupt_turn(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move {
            self.interrupt.notify_one();
            Ok(())
        })
    }

    fn note(&self, level: NoticeLevel, code: &str, message: &str) {
        self.notes
            .lock()
            .unwrap()
            .push((level, code.to_string(), message.to_string()));
    }

    fn switch_ui<'a>(
        &'a self,
        _hash: &'a str,
        _dir: &'a Path,
    ) -> BoxFut<'a, Result<bool, CoreError>> {
        Box::pin(async move { Ok(self.switch_ok) })
    }

    fn rollback_ui(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn session_stopped(&self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            if self.gate_stopped.swap(false, Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
        })
    }

    fn wait_idle(&self) -> BoxFut<'_, Result<(), CoreError>> {
        Box::pin(async move {
            if self.gate_idle.swap(false, Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

fn last_step(v: &[ImproveStatus]) -> Option<ImproveStep> {
    v.last().map(|s| s.step)
}

/// Attend (au plus `secs` s) que les statuts émis vérifient `pred`.
async fn wait_until(
    log: &Arc<Mutex<Vec<ImproveStatus>>>,
    secs: u64,
    pred: impl Fn(&[ImproveStatus]) -> bool,
) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if pred(&log.lock().unwrap()) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    pred(&log.lock().unwrap())
}

/// Attend (au plus `secs` s) que `cond` soit vrai.
async fn wait_cond(secs: u64, cond: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

fn j16_pipeline(
    root: &Path,
    cfg: &RepoConfig,
    host: Arc<TurnHost>,
    tc: Arc<FakeToolchain>,
) -> Arc<Pipeline> {
    Arc::new(
        Pipeline::new(cfg.clone(), host)
            .with_toolchain(tc)
            .with_dirs(root.join("work"), root.join("ui"))
            .with_contract(CONTRACT)
            .with_config_file(root.join("config.json")),
    )
}

type CycleTask = tokio::task::JoinHandle<Result<ImproveOutcome, CoreError>>;

/// Lance un cycle en tâche de fond.
fn spawn_cycle(
    p: &Arc<Pipeline>,
    req: &ImproveRequest,
) -> (CycleTask, Arc<Mutex<Vec<ImproveStatus>>>) {
    let (sink, log) = collector();
    let (p, req) = (p.clone(), req.clone());
    (tokio::spawn(async move { p.run(req, sink).await }), log)
}

/// Issue du cycle, au plus 600 s (git est lent sous Windows chargé).
async fn finished(task: CycleTask) -> ImproveOutcome {
    tokio::time::timeout(Duration::from_secs(600), task)
        .await
        .expect("cycle fini en moins de 600 s")
        .unwrap()
        .unwrap()
}

fn count(log: &[ImproveStatus], step: ImproveStep) -> usize {
    log.iter().filter(|s| s.step == step).count()
}

/// Le chemin `p` (toutes formes : `\`, `/`, casse) n'apparaît pas dans `text`.
fn assert_no_path(text: &str, p: &Path) {
    let s = p.to_string_lossy().to_lowercase();
    let t = text.to_lowercase();
    for v in [s.replace('/', "\\"), s.replace('\\', "/")] {
        assert!(!t.contains(&v), "chemin {v} présent dans :\n{text}");
    }
}

/// Critère 2 : des tests en échec renvoient à la session un retour borné,
/// sans chemin local ni séquence ANSI ; le tour 2 (reprise) corrige et le
/// cycle fusionne.
#[tokio::test]
async fn failing_tests_are_fed_back_then_fixed_and_merged() {
    let root = TempRoot::new("j16-feedback");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "a\n")]),
        turn(vec![("src/a.txt", "a corrigé\n")]),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("ajouter a").unwrap();
    let wt = p.worktree_path(&req.branch);
    let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users".into()));
    // Sortie de test : ANSI, contrôle, chemins du worktree (formes `\` et
    // `/`), du worktree de tests et du dossier personnel, 300 lignes.
    let mut out = String::from(
        "npm test a échoué :\n\u{1b}[31mFAIL\u{1b}[0m {TREE}\\e2e\\unit\\a.test.ts\r\n",
    );
    out.push_str(&format!(
        "at {} (src/a.ts:1)\n",
        wt.to_string_lossy().replace('\\', "/")
    ));
    for i in 0..300 {
        out.push_str(&format!(
            "  ligne {i} {}\\src\\a.ts {}\\.npmrc\u{7}\n",
            wt.display(),
            home.display()
        ));
    }
    out.push_str("Tests: 1 failed\n");
    tc.script
        .lock()
        .unwrap()
        .extend([TestStep::Fail(out), TestStep::Ok]);

    let (task, log) = spawn_cycle(&p, &req);
    let outcome = finished(task).await;
    let log = log.lock().unwrap().clone();
    let ImproveOutcome::Merged { commit } = outcome else {
        panic!("{outcome:?} / {:?}", log.last())
    };
    assert_eq!(main_of(&cfg), commit);
    assert_eq!(
        sh(&cfg.dir, &["rev-list", "--count", &format!("{c1}..main")]),
        "2"
    );
    use ImproveStep as S;
    assert_eq!(
        log.iter().map(|s| s.step).collect::<Vec<_>>(),
        [
            S::Preparing,
            S::Working,
            S::Checking,
            S::Testing,
            S::Fixing,
            S::Checking,
            S::Testing,
            S::Building,
            S::WaitingIdle,
            S::Switching,
            S::Merged
        ]
    );
    let fixing = log.iter().find(|s| s.step == S::Fixing).unwrap();
    assert_eq!(
        (fixing.attempt, fixing.max_attempts, fixing.turn),
        (1, 3, 1)
    );
    assert_eq!(log.last().unwrap().turn, 2);
    // Deux lancements : le premier avec le prompt, le second en reprise,
    // dans le même worktree.
    let calls = host.calls();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].1 && calls[1].1, "{calls:?}");
    assert!(calls[0].0.contains("ajouter a"));
    let cwds = host.cwds.lock().unwrap().clone();
    assert_eq!(cwds, [wt.clone(), wt.clone()]);
    // Retour d'erreur : en-tête, borné, nettoyé.
    let fb = &calls[1].0;
    assert!(
        fb.starts_with("[Contrôle automatique — tentative 1/3]\nÉtape : tests\n"),
        "{fb}"
    );
    assert!(fb.contains("Raison : npm test a échoué"), "{fb}");
    assert!(fb.len() <= MAX_FEEDBACK_BYTES, "{} octets", fb.len());
    assert!(
        fb.contains("Tests: 1 failed") && fb.contains("tronquée"),
        "{fb}"
    );
    assert!(fb.contains(".\\src\\a.ts"), "{fb}");
    assert!(fb.contains("~\\.npmrc"), "{fb}");
    assert_no_path(fb, &wt);
    assert_no_path(fb, &root.join("work").join(".tests"));
    assert_no_path(fb, &home);
    assert!(
        !fb.chars().any(|c| c.is_control() && c != '\n' && c != '\t'),
        "caractère de contrôle dans le retour"
    );
    // Garde après chaque tour, lignes d'état lisibles.
    assert_eq!(host.stopped.load(Ordering::SeqCst), 2);
    assert_eq!(host.notes_with("improve.guard").len(), 2);
    assert_eq!(
        host.notes_with("improve.fault"),
        ["✗ Tests en échec (tentative 1/3) : erreur renvoyée à la session"]
    );
    assert_eq!(
        host.notes_with("improve.merged"),
        [format!(
            "✓ Fusionné {}",
            log.last().unwrap().commit.clone().unwrap()
        )]
    );
    assert!(!host.notes_with("improve.checking").is_empty());
}

/// Critère 3 : 4 échecs consécutifs donnent exactement 3 tours `fixing`,
/// puis l'attente (`attempts_exhausted`) ; un message de l'utilisateur
/// remet `attempt` à 0 et relance un tour.
#[tokio::test]
async fn four_failures_wait_for_user_and_message_resets_attempts() {
    let root = TempRoot::new("j16-attempts");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "1\n")]),
        turn(vec![("src/a.txt", "2\n")]),
        turn(vec![("src/a.txt", "3\n")]),
        turn(vec![("src/a.txt", "4\n")]),
        turn(vec![("src/a.txt", "5\n")]),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    tc.script
        .lock()
        .unwrap()
        .extend((0..4).map(|i| TestStep::Fail(format!("npm test a échoué :\néchec {i}"))));
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("quatre échecs").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    assert!(
        wait_until(&log, 600, |v| last_step(v)
            == Some(ImproveStep::AwaitingUser))
        .await,
        "{:?}",
        steps(&log)
    );
    {
        let v = log.lock().unwrap();
        assert_eq!(count(&v, ImproveStep::Fixing), MAX_AUTO_ATTEMPTS as usize);
        let last = v.last().unwrap();
        assert_eq!(last.waiting, Some(ImproveWaiting::AttemptsExhausted));
        assert_eq!((last.attempt, last.turn), (3, 4));
        assert!(last.detail.as_deref().unwrap_or("").contains("échec 3"));
    }
    assert_eq!(tc.tests.lock().unwrap().len(), 4);
    assert_eq!(host.calls().len(), 4);
    assert_eq!(main_of(&cfg), c1);
    assert!(p.interrupt(&req.id).await.is_err(), "pas de tour en cours");
    // Le message relance un tour avec `attempt` = 0.
    p.send(&req.id, "  essaie autrement  ").await.unwrap();
    let out = finished(task).await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    let calls = host.calls();
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[4], ("essaie autrement".to_string(), true));
    let v = log.lock().unwrap().clone();
    let after = v
        .iter()
        .rposition(|s| s.step == ImproveStep::AwaitingUser)
        .unwrap();
    assert_eq!(v[after + 1].step, ImproveStep::Working);
    assert_eq!(v[after + 1].attempt, 0);
    assert_eq!(v[after + 1].waiting, None);
    assert_eq!(v.last().unwrap().attempt, 0);
    // Corrections numérotées 1, 2, 3.
    let fixes: Vec<u32> = v
        .iter()
        .filter(|s| s.step == ImproveStep::Fixing)
        .map(|s| s.attempt)
        .collect();
    assert_eq!(fixes, [1, 2, 3]);
    assert!(calls[3]
        .0
        .starts_with("[Contrôle automatique — tentative 3/3]"));
}

/// Critère 4 (1) : la garde tourne après chaque tour, y compris un tour
/// sans commit, qui ne lance pas les tests et attend l'utilisateur.
#[tokio::test]
async fn guard_after_every_turn_and_no_commit_turn_skips_tests() {
    let root = TempRoot::new("j16-guard");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "a\n")]),
        // Correction sans commit (question posée en texte).
        turn(Vec::new()),
        turn(vec![("src/a.txt", "a2\n")]),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    tc.script
        .lock()
        .unwrap()
        .push_back(TestStep::Fail("npm test a échoué :\nx".into()));
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("trois tours").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    assert!(
        wait_until(&log, 600, |v| last_step(v)
            == Some(ImproveStep::AwaitingUser))
        .await
    );
    assert_eq!(
        log.lock().unwrap().last().unwrap().waiting,
        Some(ImproveWaiting::NoNewCommit)
    );
    assert_eq!(tc.tests.lock().unwrap().len(), 1, "pas de tests au tour 2");
    p.send(&req.id, "oui, vas-y").await.unwrap();
    let out = finished(task).await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert_eq!(host.calls().len(), 3);
    assert_eq!(tc.tests.lock().unwrap().len(), 2);
    assert_eq!(host.stopped.load(Ordering::SeqCst), 3);
    assert_eq!(
        host.notes_with("improve.guard"),
        [
            "✓ Garde de sécurité (tour 1)",
            "✓ Garde de sécurité (tour 2)",
            "✓ Garde de sécurité (tour 3)"
        ]
    );
}

/// Critère 4 (2) : `.git/config` modifié au tour 2 (tour de correction) :
/// `failed`, rien n'est renvoyé à la session, branche non fusionnée.
#[tokio::test]
async fn git_config_modified_on_turn_two_fails_without_feedback() {
    let root = TempRoot::new("j16-gitconf");
    let (cfg, c1, _) = ready_clone(&root).await;
    let path = cfg.dir.join(".git").join("config");
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "a\n")]),
        Turn {
            files: vec![("src/a.txt", "a2\n")],
            extra: Some(Box::new(move |_| {
                let mut perm = std::fs::metadata(&path).unwrap().permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                std::fs::set_permissions(&path, perm).unwrap();
                let mut c = std::fs::read(&path).unwrap();
                c.extend_from_slice(b"[url \"C:/pirate.git\"]\n\tpushInsteadOf = git@\n");
                std::fs::write(&path, c).unwrap();
            })),
            end: TurnScript::Done,
        },
    ]));
    let tc = Arc::new(FakeToolchain::new());
    tc.script
        .lock()
        .unwrap()
        .push_back(TestStep::Fail("npm test a échoué :\nx".into()));
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("config au tour 2").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    let out = finished(task).await;
    assert_refused(&out, ".git/config");
    assert_eq!(steps(&log).last(), Some(&ImproveStep::Failed));
    let calls = host.calls();
    assert_eq!(calls.len(), 2, "aucun tour après la violation");
    // Seul retour : celui des tests du tour 1 (le prompt du tour 1 cite la
    // configuration git dans ses interdits).
    assert!(calls[1]
        .0
        .starts_with("[Contrôle automatique — tentative 1/3]\nÉtape : tests"));
    assert!(!calls[1].0.contains(".git/config") && !calls[1].0.contains("configuration git"));
    assert_eq!(
        tc.tests.lock().unwrap().len(),
        1,
        "pas de tests après le tour 2"
    );
    assert_eq!(main_of(&cfg), c1);
    let unmerged = sh(&cfg.dir, &["branch", "--no-merged", "main"]);
    assert!(
        unmerged
            .lines()
            .any(|l| l.trim_start_matches(['*', '+', ' ']) == req.branch),
        "{unmerged}"
    );
    assert_eq!(host.notes_with("improve.fault").len(), 1);
    assert_eq!(host.notes_with("improve.failed").len(), 1);
}

/// Critère 5 : un message pendant `testing` abandonne les tests en moins de
/// 1 s et lance un nouveau tour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn message_during_testing_aborts_tests_and_starts_a_turn() {
    // Mesure de latence : seul test en cours.
    let root = TempRoot::exclusive("j16-abort");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "a\n")]),
        turn(vec![("src/b.txt", "b\n")]),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    // Vrai processus sous Job, qui tient le dossier des tests.
    tc.script.lock().unwrap().push_back(TestStep::HangProcess);
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("abandon").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    assert!(wait_until(&log, 600, |v| last_step(v) == Some(ImproveStep::Testing)).await);
    assert!(
        wait_cond(600, || !tc.tests.lock().unwrap().is_empty()).await,
        "tests en cours"
    );
    assert!(
        p.validate(&req.id).await.is_err(),
        "« Valider » hors attente"
    );
    let t0 = Instant::now();
    p.send(&req.id, "change plutôt b").await.unwrap();
    assert!(wait_cond(30, || host.calls().len() == 2).await);
    let started = host.started.lock().unwrap()[1];
    let elapsed = started.duration_since(t0);
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(host.calls()[1], ("change plutôt b".to_string(), true));
    let out = finished(task).await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    let trees: Vec<PathBuf> = tc
        .tests
        .lock()
        .unwrap()
        .iter()
        .map(|t| t.0.clone())
        .collect();
    assert_eq!(trees.len(), 2);
    assert_ne!(trees[0], trees[1], "un worktree de tests par passage");
    assert_eq!(host.notes_with("improve.aborted").len(), 1);
    // Worktrees des tests supprimés (celui des tests abandonnés au passage
    // suivant, sinon par le nettoyage).
    let tests_root = root.join("work").join(".tests");
    let mut empty = false;
    for _ in 0..50 {
        p.cleanup_merged().await;
        if !tests_root.read_dir().unwrap().any(|_| true) {
            empty = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(empty, "worktrees de tests restés");
    // Plus de message une fois le cycle fini.
    assert!(p.send(&req.id, "trop tard").await.is_err());
}

/// Message pendant un tour : mis en file et livré comme tour suivant, sans
/// tests entre les deux ; interruption sans message : attente
/// (`interrupted`) ; « Valider » relance contrôles et tests sans tour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_message_interrupt_and_validate() {
    let root = TempRoot::new("j16-queue");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        Turn {
            files: vec![("src/a.txt", "a\n")],
            extra: None,
            end: TurnScript::UntilInterrupt,
        },
        turn_end(TurnScript::UntilInterrupt),
    ]));
    let tc = Arc::new(FakeToolchain::new());
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("file").unwrap();
    assert!(p.send("inconnu", "x").await.is_err());
    let (task, log) = spawn_cycle(&p, &req);
    assert!(wait_cond(600, || host.calls().len() == 1).await);
    assert!(
        p.validate(&req.id).await.is_err(),
        "« Valider » pendant un tour"
    );
    assert!(p.send(&req.id, "   ").await.is_err(), "message vide");
    p.send(&req.id, "précision").await.unwrap();
    p.interrupt(&req.id).await.unwrap();
    // Tour 2 = le message en file, sans tests entre les deux.
    assert!(wait_cond(600, || host.calls().len() == 2).await);
    assert_eq!(host.calls()[1], ("précision".to_string(), true));
    assert!(tc.tests.lock().unwrap().is_empty());
    // Tour 2 interrompu sans message : attente.
    p.interrupt(&req.id).await.unwrap();
    assert!(
        wait_until(&log, 600, |v| last_step(v)
            == Some(ImproveStep::AwaitingUser))
        .await
    );
    assert_eq!(
        log.lock().unwrap().last().unwrap().waiting,
        Some(ImproveWaiting::Interrupted)
    );
    assert!(tc.tests.lock().unwrap().is_empty());
    assert!(p.interrupt(&req.id).await.is_err(), "pas de tour en cours");
    // « Valider » : contrôles et tests sur le commit du tour 1.
    p.validate(&req.id).await.unwrap();
    let out = finished(task).await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert_eq!(tc.tests.lock().unwrap().len(), 1);
    assert_eq!(host.calls().len(), 2, "pas de tour pour « Valider »");
    assert_eq!(host.stopped.load(Ordering::SeqCst), 2);
}

/// Session arrêtée avant la fin de son tour : attente (`session_stopped`)
/// sans tests ni tentative ; annulation : branche gardée.
#[tokio::test]
async fn stopped_session_waits_without_attempt() {
    let root = TempRoot::new("j16-stopped");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![Turn {
        files: vec![("src/a.txt", "a\n")],
        extra: None,
        end: TurnScript::Fail,
    }]));
    let tc = Arc::new(FakeToolchain::new());
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("arrêt").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    assert!(
        wait_until(&log, 600, |v| last_step(v)
            == Some(ImproveStep::AwaitingUser))
        .await
    );
    let last = log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.waiting, Some(ImproveWaiting::SessionStopped));
    assert_eq!((last.attempt, last.turn), (0, 1));
    assert!(tc.tests.lock().unwrap().is_empty());
    p.cancel(&req.id).await.unwrap();
    let out = finished(task).await;
    assert_eq!(out, ImproveOutcome::Cancelled);
    assert_eq!(main_of(&cfg), c1);
    assert!(p.worktree_path(&req.branch).join("src/a.txt").is_file());
    assert_eq!(host.notes_with("improve.cancelled").len(), 1);
}

/// Faute de périmètre corrigible : renvoyée à la session, corrigée au tour
/// suivant par un commit qui rétablit le fichier.
#[tokio::test]
async fn out_of_scope_file_is_fed_back_and_fixed() {
    let root = TempRoot::new("j16-scope");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![
        turn(vec![("src/a.txt", "a\n"), ("README.md", "piraté\n")]),
        Turn {
            files: Vec::new(),
            extra: Some(Box::new(|wt| {
                write(wt, "README.md", "# app\n");
                sh(wt, &["commit", "-q", "-m", "rétablit", "--", "README.md"]);
            })),
            end: TurnScript::Done,
        },
    ]));
    let tc = Arc::new(FakeToolchain::new());
    let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
    let req = p.request("périmètre").unwrap();
    let (task, _log) = spawn_cycle(&p, &req);
    let out = finished(task).await;
    // README.md rétabli à l'identique : plus de différence hors périmètre.
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    let calls = host.calls();
    assert!(
        calls[1].0.contains("Étape : contrôles")
            && calls[1].0.contains("README.md (hors périmètre)"),
        "{}",
        calls[1].0
    );
    assert_eq!(tc.tests.lock().unwrap().len(), 1, "pas de tests au tour 1");
}

/// Relecture 1 : la garde est rejouée après les tests et après le build
/// (`node_modules` du clone partagé, fichiers de l'app) : violation fatale,
/// sans retour à la session ni fusion.
#[tokio::test]
async fn guard_replayed_after_tests_and_build() {
    let root = TempRoot::new("j16-guard-tests");
    let (cfg, c1, _) = ready_clone(&root).await;
    // Les tests modifient le node_modules du clone.
    let lock = cfg.dir.join("node_modules").join(".package-lock.json");
    let target = lock.clone();
    let mut tc = FakeToolchain::new();
    tc.on_test = Some(Box::new(move |_| {
        std::fs::write(&target, "{\"par les tests\":1}").unwrap()
    }));
    let host = Arc::new(TurnHost::new(vec![turn(vec![("src/a.txt", "a\n")])]));
    let p = j16_pipeline(&root, &cfg, host.clone(), Arc::new(tc));
    let req = p.request("tests").unwrap();
    let (task, _) = spawn_cycle(&p, &req);
    let out = finished(task).await;
    assert_refused(&out, "node_modules");
    assert_eq!(host.calls().len(), 1, "rien de renvoyé à la session");
    assert_eq!(main_of(&cfg), c1);
    std::fs::remove_file(&lock).unwrap();

    // Le build modifie config.json.
    let conf = root.join("config.json");
    let target = conf.clone();
    let mut tc = FakeToolchain::new();
    tc.on_build = Some(Box::new(move |_| std::fs::write(&target, "{}").unwrap()));
    let host = Arc::new(TurnHost::new(vec![turn(vec![("src/b.txt", "b\n")])]));
    let p = j16_pipeline(&root, &cfg, host.clone(), Arc::new(tc));
    let req = p.request("build").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    let out = finished(task).await;
    assert_refused(&out, "config.json");
    assert_eq!(host.calls().len(), 1);
    assert_eq!(main_of(&cfg), c1);
    assert!(
        !steps(&log).contains(&ImproveStep::Switching),
        "jamais activée"
    );
}

/// Relecture 5 : un commit arrivé après les tests (pendant le build) n'est
/// ni activé ni fusionné.
#[tokio::test]
async fn commit_after_tests_is_never_merged() {
    let root = TempRoot::new("j16-moved");
    let (cfg, c1, _) = ready_clone(&root).await;
    let mut tc = FakeToolchain::new();
    tc.on_build = Some(Box::new(|wt| {
        write(wt, "src/tard.txt", "x\n");
        sh(wt, &["add", "--", "src/tard.txt"]);
        sh(wt, &["commit", "-q", "-m", "tard", "--", "src/tard.txt"]);
    }));
    let host = Arc::new(TurnHost::new(vec![turn(vec![("src/a.txt", "a\n")])]));
    let p = j16_pipeline(&root, &cfg, host.clone(), Arc::new(tc));
    let req = p.request("commit tardif").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    let out = finished(task).await;
    assert_refused(&out, "a bougé après les tests");
    assert_eq!(main_of(&cfg), c1);
    assert!(!steps(&log).contains(&ImproveStep::Switching));
    assert_eq!(host.calls().len(), 1);
}

/// Relecture 5 : entre les tests et la bascule, ni message, ni « Valider »,
/// ni interruption ; le cycle fusionne la tête testée.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_accepted_between_tests_and_switch() {
    let root = TempRoot::new("j16-closed");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![turn(vec![("src/a.txt", "a\n")])]));
    host.gate_idle.store(true, Ordering::SeqCst);
    let p = j16_pipeline(&root, &cfg, host.clone(), Arc::new(FakeToolchain::new()));
    let req = p.request("fermé").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    tokio::time::timeout(Duration::from_secs(600), host.reached.notified())
        .await
        .expect("attente du repos atteinte");
    assert_eq!(
        last_step(&log.lock().unwrap()),
        Some(ImproveStep::WaitingIdle)
    );
    assert!(p.send(&req.id, "encore une chose").await.is_err());
    assert!(p.validate(&req.id).await.is_err());
    assert!(p.interrupt(&req.id).await.is_err());
    host.release.notify_one();
    let out = finished(task).await;
    let ImproveOutcome::Merged { commit } = out else {
        panic!("{out:?}")
    };
    assert_eq!(main_of(&cfg), commit);
    assert_eq!(host.calls().len(), 1);
}

/// Relecture 4 : une interruption après la fin réelle du tour (pendant
/// l'arrêt de la session et la garde) est refusée et ne marque pas le tour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_after_turn_end_is_refused() {
    let root = TempRoot::new("j16-late-int");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(TurnHost::new(vec![turn(vec![("src/a.txt", "a\n")])]));
    host.gate_stopped.store(true, Ordering::SeqCst);
    let p = j16_pipeline(&root, &cfg, host.clone(), Arc::new(FakeToolchain::new()));
    let req = p.request("tard").unwrap();
    let (task, log) = spawn_cycle(&p, &req);
    tokio::time::timeout(Duration::from_secs(600), host.reached.notified())
        .await
        .expect("fin du tour atteinte");
    assert!(p.interrupt(&req.id).await.is_err(), "tour déjà fini");
    host.release.notify_one();
    let out = finished(task).await;
    assert!(matches!(out, ImproveOutcome::Merged { .. }), "{out:?}");
    assert!(!steps(&log).contains(&ImproveStep::AwaitingUser));
}

/// Relecture 5 (critère 4) : la garde arrête le cycle aussi après un tour
/// sans commit, un tour interrompu, ou un tour suivi d'un message en file
/// (jamais livré).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guard_violation_on_no_commit_interrupted_and_queued_turns() {
    let root = TempRoot::new("j16-guard-cases");
    let (cfg, c1, _) = ready_clone(&root).await;
    let hook = cfg.dir.join(".git").join("hooks").join("post-commit");
    for case in ["sans commit", "interrompu", "message en file"] {
        let h = hook.clone();
        let extra: Hook = Some(Box::new(move |_| {
            std::fs::write(&h, "#!/bin/sh\n").unwrap();
        }));
        let t = match case {
            "sans commit" => Turn {
                files: Vec::new(),
                extra,
                end: TurnScript::Done,
            },
            _ => Turn {
                files: vec![("src/a.txt", "a\n")],
                extra,
                end: TurnScript::UntilInterrupt,
            },
        };
        let host = Arc::new(TurnHost::new(vec![t]));
        let tc = Arc::new(FakeToolchain::new());
        let p = j16_pipeline(&root, &cfg, host.clone(), tc.clone());
        let req = p.request(case).unwrap();
        let (task, _) = spawn_cycle(&p, &req);
        if case != "sans commit" {
            assert!(wait_cond(600, || host.calls().len() == 1).await);
            if case == "message en file" {
                p.send(&req.id, "jamais livré").await.unwrap();
            }
            p.interrupt(&req.id).await.unwrap();
        }
        let out = finished(task).await;
        match &out {
            ImproveOutcome::Failed {
                step: ImproveStep::Checking,
                reason,
            } => assert!(reason.contains("hooks"), "{case} : {reason}"),
            other => panic!("{case} : {other:?}"),
        }
        assert_eq!(host.calls().len(), 1, "{case}");
        assert!(tc.tests.lock().unwrap().is_empty(), "{case}");
        assert_eq!(main_of(&cfg), c1);
        std::fs::remove_file(&hook).unwrap();
    }
}
