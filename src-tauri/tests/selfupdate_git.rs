//! J14, étape 3 : clone et cycle git de l'auto-amélioration, sur des dépôts
//! git temporaires (un « origin » nu local comme source du clone), avec un
//! `PipelineHost` et une `Toolchain` simulés. Aucun accès réseau. Chaque test
//! nettoie son dossier, même en cas d'échec (garde `Drop`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orchestrator_lib::api::types::{CoreError, ImproveStatus, ImproveStep, RepoStatus};
use orchestrator_lib::selfupdate::pipeline::{NpmToolchain, Toolchain};
use orchestrator_lib::selfupdate::repo::{
    check_url, ensure_clone_with, load_from, CloneOptions, ToolCheck, NO_PUSH_URL,
};
use orchestrator_lib::selfupdate::{
    BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, RepoConfig, StatusSink,
};

// --- Outils de test -----------------------------------------------------------

/// Dossier temporaire d'un test, supprimé au drop (même après un échec).
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("orch-c-{name}-{}", std::process::id()));
        if d.exists() {
            let _ = std::fs::remove_dir_all(&d);
        }
        std::fs::create_dir_all(&d).unwrap();
        TempRoot(d)
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
    switch_ok: bool,
    /// Action pendant la bascule (reçoit le dossier du build).
    on_switch: Hook,
    switched: Mutex<Vec<(String, PathBuf)>>,
    rollbacks: AtomicUsize,
    prompts: Mutex<Vec<String>>,
    /// `node_modules` du worktree était une jonction pendant la session.
    junction_seen: Mutex<Option<bool>>,
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
}

/// Appel de `Toolchain::test` : dossier, Rust, fichiers de `e2e/` à ce
/// moment, contenu de `e2e/existing.spec.ts`.
type TestCall = (PathBuf, bool, Vec<String>, String);

struct FakeToolchain {
    contract: String,
    tests_ok: bool,
    tests: Mutex<Vec<TestCall>>,
    installs: Mutex<Vec<PathBuf>>,
    on_build: Hook,
}

impl FakeToolchain {
    fn new() -> Self {
        FakeToolchain {
            contract: CONTRACT.into(),
            tests_ok: true,
            tests: Mutex::new(Vec::new()),
            installs: Mutex::new(Vec::new()),
            on_build: None,
        }
    }

    fn rust_flags(&self) -> Vec<bool> {
        self.tests.lock().unwrap().iter().map(|t| t.1).collect()
    }
}

const CONTRACT: &str = "00c0ffee00c0ffee";

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
            if self.tests_ok {
                Ok(())
            } else {
                Err("npm test a échoué :\n1 test en échec".into())
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
            assert!(reason.starts_with("Node introuvable"), "{reason}")
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
            "« #[ignore] »",
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

    let p = pipeline(
        &root,
        cfg.clone(),
        Arc::new(success_host(vec![])),
        Arc::new(FakeToolchain::new()),
    );
    let empty_req = p.request("rien").unwrap();
    let (sink, _) = collector();
    let out = p.run(empty_req.clone(), sink).await.unwrap();
    assert_refused(&out, "aucun commit");
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
