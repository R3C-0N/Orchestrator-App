//! J14, étape 3 : clone et cycle git de l'auto-amélioration, sur des dépôts
//! git temporaires (un « origin » nu local comme source du clone), avec un
//! `PipelineHost` et une `Toolchain` simulés. Aucun accès réseau.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orchestrator_lib::api::types::{CoreError, ImproveStatus, ImproveStep, RepoStatus};
use orchestrator_lib::selfupdate::pipeline::Toolchain;
use orchestrator_lib::selfupdate::repo::{
    ensure_clone_with, load_from, CloneOptions, ToolCheck, NO_PUSH_URL,
};
use orchestrator_lib::selfupdate::{
    BoxFut, ImproveOutcome, ImproveRequest, Pipeline, PipelineHost, RepoConfig, StatusSink,
};

// --- Outils de test -----------------------------------------------------------

fn temp_root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("orch-c-{name}-{}", std::process::id()));
    if d.exists() {
        let _ = std::fs::remove_dir_all(&d);
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `git -C dir args` synchrone, identité fixée ; panique si échec.
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args)
        .output()
        .expect("git introuvable");
    assert!(
        out.status.success(),
        "git {args:?} dans {} : {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
    write(&seed, "README.md", "# app\n");
    write(&seed, ".gitignore", "node_modules/\n");
    sh(
        &seed,
        &["add", "--", "src", "src-tauri", "README.md", ".gitignore"],
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

/// Un commit fait par la session simulée.
#[derive(Clone)]
struct FakeCommit {
    files: Vec<(&'static str, &'static str)>,
    msg: &'static str,
}

#[derive(Default)]
struct FakeHost {
    commits: Vec<FakeCommit>,
    /// Fichier laissé non commité.
    dirty: Option<(&'static str, &'static str)>,
    /// La session ne finit jamais (annulation).
    hang: bool,
    switch_ok: bool,
    switched: Mutex<Vec<(String, PathBuf)>>,
    prompts: Mutex<Vec<String>>,
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
            Ok(self.switch_ok)
        })
    }
}

struct FakeToolchain {
    contract: String,
    tests_ok: bool,
    tests: Mutex<Vec<bool>>,
    installs: Mutex<Vec<PathBuf>>,
}

impl FakeToolchain {
    fn new() -> Self {
        FakeToolchain {
            contract: CONTRACT.into(),
            tests_ok: true,
            tests: Mutex::new(Vec::new()),
            installs: Mutex::new(Vec::new()),
        }
    }
}

const CONTRACT: &str = "00c0ffee00c0ffee";

impl Toolchain for FakeToolchain {
    fn install_deps<'a>(&'a self, dir: &'a Path) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            self.installs.lock().unwrap().push(dir.to_path_buf());
            write(dir, "node_modules/marker.txt", "deps");
            Ok(())
        })
    }

    fn test<'a>(&'a self, _worktree: &'a Path, rust: bool) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            self.tests.lock().unwrap().push(rust);
            if self.tests_ok {
                Ok(())
            } else {
                Err("npm test a échoué :\n1 test en échec".into())
            }
        })
    }

    fn build_ui<'a>(
        &'a self,
        _worktree: &'a Path,
        hash: &'a str,
        out: &'a Path,
    ) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            write(out, "index.html", &format!("<!-- {hash} -->"));
            Ok(())
        })
    }

    fn contract<'a>(&'a self, _worktree: &'a Path) -> BoxFut<'a, Result<String, String>> {
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

fn commit_src(files: Vec<(&'static str, &'static str)>, msg: &'static str) -> FakeCommit {
    FakeCommit { files, msg }
}

// --- Clone ---------------------------------------------------------------------

#[tokio::test]
async fn clone_is_placed_on_build_commit_and_cannot_push() {
    let root = temp_root("clone");
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

    // Deuxième lancement : clone existant gardé tel quel.
    match ensure_clone_with(&cfg, &git_only()).await {
        RepoStatus::Ready { head, path, .. } => {
            assert_eq!(head, c1);
            assert_eq!(PathBuf::from(path), cfg.dir);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn clone_failures_disable_with_reason() {
    let root = temp_root("clonefail");
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
    let root = temp_root("config");
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
    let root = temp_root("request");
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
    let root = temp_root("success");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![
            commit_src(vec![("src/a.txt", "a\n")], "ajoute a"),
            commit_src(vec![("src/b.txt", "b\n"), ("docs/n.md", "n\n")], "ajoute b"),
        ],
        switch_ok: true,
        ..Default::default()
    });
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
    // Pas de test Rust ; deps : npm ci une fois dans le clone, jonction ensuite.
    assert_eq!(*tc.tests.lock().unwrap(), [false]);
    assert_eq!(*tc.installs.lock().unwrap(), std::slice::from_ref(&cfg.dir));
    // Worktree fusionné supprimé, sans toucher au node_modules du clone.
    assert!(!wt.exists(), "worktree fusionné supprimé");
    assert!(cfg.dir.join("node_modules/marker.txt").is_file());
    assert!(!sh(&cfg.dir, &["worktree", "list"]).contains(&req.branch));
    // Clone propre.
    assert_eq!(sh(&cfg.dir, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn file_out_of_scope_is_refused() {
    let root = temp_root("scope");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(
            vec![("src/a.txt", "a\n"), ("README.md", "piraté\n")],
            "hors périmètre",
        )],
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let req = p.request("toucher le readme").unwrap();
    let (sink, log) = collector();
    match p.run(req.clone(), sink).await.unwrap() {
        ImproveOutcome::Failed {
            step: ImproveStep::Checking,
            reason,
        } => {
            assert!(
                reason.contains("hors périmètre") && reason.contains("README.md"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(main_of(&cfg), c1);
    assert!(host.switched.lock().unwrap().is_empty());
    assert_eq!(steps(&log).last(), Some(&ImproveStep::Failed));
    assert!(
        p.worktree_path(&req.branch).exists(),
        "worktree d'un échec gardé"
    );
}

#[tokio::test]
async fn dirty_tree_and_no_commit_are_refused() {
    let root = temp_root("dirty");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        dirty: Some(("src/oubli.txt", "x")),
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let (sink, _) = collector();
    match p.run(p.request("sale").unwrap(), sink).await.unwrap() {
        ImproveOutcome::Failed {
            step: ImproveStep::Checking,
            reason,
        } => {
            assert!(
                reason.contains("arbre non propre") && reason.contains("src/oubli.txt"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }

    let host = Arc::new(FakeHost {
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(&root, cfg.clone(), host, Arc::new(FakeToolchain::new()));
    let (sink, _) = collector();
    match p.run(p.request("rien").unwrap(), sink).await.unwrap() {
        ImproveOutcome::Failed {
            step: ImproveStep::Checking,
            reason,
        } => {
            assert!(reason.contains("aucun commit"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(main_of(&cfg), c1);
}

#[tokio::test]
async fn failing_tests_keep_main() {
    let root = temp_root("tests");
    let (cfg, c1, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        switch_ok: true,
        ..Default::default()
    });
    let mut tc = FakeToolchain::new();
    tc.tests_ok = false;
    let p = pipeline(&root, cfg.clone(), host.clone(), Arc::new(tc));
    let (sink, _) = collector();
    match p
        .run(p.request("tests rouges").unwrap(), sink)
        .await
        .unwrap()
    {
        ImproveOutcome::Failed {
            step: ImproveStep::Testing,
            reason,
        } => {
            assert!(reason.contains("1 test en échec"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(main_of(&cfg), c1);
    assert!(host.switched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_switch_leaves_main_and_branch_unmerged() {
    let root = temp_root("switch");
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

#[tokio::test]
async fn rust_change_is_merged_but_requires_restart() {
    let root = temp_root("rust");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(
            vec![("src-tauri/core.rs", "fn main() { println!(); }\n")],
            "cœur",
        )],
        switch_ok: true,
        ..Default::default()
    });
    let tc = Arc::new(FakeToolchain::new());
    let p = pipeline(&root, cfg.clone(), host.clone(), tc.clone());
    let (sink, log) = collector();
    let ImproveOutcome::RestartRequired { commit } =
        p.run(p.request("cœur").unwrap(), sink).await.unwrap()
    else {
        panic!("{:?}", log.lock().unwrap().last())
    };
    assert_eq!(main_of(&cfg), commit, "fusionnée après les tests");
    assert!(host.switched.lock().unwrap().is_empty(), "jamais activée");
    assert_eq!(*tc.tests.lock().unwrap(), [true], "cargo test demandé");
    assert!(!steps(&log).contains(&ImproveStep::Building));
    let last = log.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.step, ImproveStep::RestartRequired);
    assert!(last.detail.unwrap().contains("src-tauri"));
}

#[tokio::test]
async fn contract_change_requires_restart() {
    let root = temp_root("contract");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(vec![("src/a.txt", "a\n")], "a")],
        switch_ok: true,
        ..Default::default()
    });
    let mut tc = FakeToolchain::new();
    tc.contract = "ffffffffffffffff".into();
    let p = pipeline(&root, cfg.clone(), host.clone(), Arc::new(tc));
    let (sink, _) = collector();
    let out = p.run(p.request("contrat").unwrap(), sink).await.unwrap();
    assert!(
        matches!(out, ImproveOutcome::RestartRequired { .. }),
        "{out:?}"
    );
    assert!(host.switched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revert_adds_revert_commit_on_top() {
    let root = temp_root("revert");
    let (cfg, _, _) = ready_clone(&root).await;
    let host = Arc::new(FakeHost {
        commits: vec![commit_src(
            vec![("src/app.txt", "v1 modifiée\n")],
            "Changer le titre",
        )],
        switch_ok: true,
        ..Default::default()
    });
    let p = pipeline(
        &root,
        cfg.clone(),
        host.clone(),
        Arc::new(FakeToolchain::new()),
    );
    let (sink, _) = collector();
    let ImproveOutcome::Merged { commit } = p.run(p.request("titre").unwrap(), sink).await.unwrap()
    else {
        panic!()
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
    let root = temp_root("cancel");
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
    for _ in 0..200 {
        if steps(&log).contains(&ImproveStep::Working) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(steps(&log).contains(&ImproveStep::Working));
    // Un seul cycle à la fois.
    let (sink2, _) = collector();
    assert!(p.run(p.request("autre").unwrap(), sink2).await.is_err());
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
    // Le cycle suivant est de nouveau permis (emplacement libéré).
    assert!(p.cancel(&req.id).await.is_err());
}
