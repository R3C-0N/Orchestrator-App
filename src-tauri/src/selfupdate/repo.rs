//! Clone du dépôt source de l'app (J14, décision 16) : emplacement, URL
//! remplaçable par `config.json`, premier clone non interactif placé sur le
//! commit de compilation, contrôle d'un clone existant. Toute impossibilité
//! devient un `RepoStatus::Disabled { reason }` lisible : rien ne panique et
//! rien ne bloque le démarrage de l'app.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::git::{self, git, git_ok, git_timeout, resolve};
use super::{RepoConfig, DEFAULT_ORIGIN};
use crate::api::types::RepoStatus;

/// Nom du dossier du clone (sous le dossier d'installation ou de données).
pub const REPO_DIR_NAME: &str = "repo";

/// URL de push du clone : le clone ne pousse jamais (décision 16).
pub const NO_PUSH_URL: &str = "no-push://orchestrator-ne-pousse-jamais";

/// Outil dont l'auto-amélioration a besoin (décision 19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCheck {
    /// Nom affiché dans la raison de désactivation.
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl ToolCheck {
    pub fn new(label: &str, program: &str, args: &[&str]) -> Self {
        ToolCheck {
            label: label.into(),
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }
}

/// Réglages de [`ensure_clone_with`].
#[derive(Debug, Clone)]
pub struct CloneOptions {
    /// Outils vérifiés avant tout (`--version`).
    pub required_tools: Vec<ToolCheck>,
    /// Délai maximal du clonage (et du `fetch` d'un commit manquant).
    pub clone_timeout: Duration,
}

impl Default for CloneOptions {
    fn default() -> Self {
        CloneOptions {
            required_tools: vec![
                ToolCheck::new("git", "git", &["--version"]),
                ToolCheck::new("Node", "node", &["--version"]),
                ToolCheck::new("npm", git::npm_program(), &["--version"]),
                ToolCheck::new("cargo", "cargo", &["--version"]),
            ],
            clone_timeout: Duration::from_secs(600),
        }
    }
}

/// [`RepoConfig`] calculée sans variable d'environnement (testable).
/// `config_file` : `config.json` (clé `repo_url`) ; `install_dir` : dossier
/// de l'exe, retenu s'il est inscriptible ; `data_dir` :
/// `%LOCALAPPDATA%\orchestrator`.
pub fn load_from(
    config_file: Option<&Path>,
    install_dir: Option<&Path>,
    data_dir: Option<&Path>,
    commit: &str,
) -> RepoConfig {
    let url = config_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("repo_url")?.as_str().map(str::trim).map(String::from))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_ORIGIN.to_string());
    let dir = match install_dir.filter(|d| is_writable(d)) {
        Some(d) => d.join(REPO_DIR_NAME),
        None => data_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir)
            .join(REPO_DIR_NAME),
    };
    RepoConfig {
        url,
        dir,
        commit: commit.to_string(),
    }
}

/// Le dossier existe et on peut y créer un fichier (installation NSIS par
/// utilisateur : oui ; MSI dans Program Files : non).
pub fn is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".orch-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// `dir` est la racine d'un clone git (dossier ou fichier `.git`).
pub fn is_clone(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// URL de dépôt acceptée : `git@…`, `ssh://`, `https://`, `file://` ou
/// chemin local absolu ; jamais d'option (`-…`) ni de transport `ext::`.
pub fn check_url(url: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("URL du dépôt refusée ({why}) : {url}"));
    if url.is_empty() {
        return bad("vide");
    }
    if url.starts_with('-') {
        return bad("commence par « - »");
    }
    if url.chars().any(|c| c.is_control()) {
        return bad("caractère de contrôle");
    }
    let lower = url.to_ascii_lowercase();
    let remote = ["git@", "ssh://", "https://", "file://"]
        .iter()
        .any(|p| lower.starts_with(p));
    if remote || Path::new(url).is_absolute() {
        Ok(())
    } else {
        bad("formes permises : git@, ssh://, https://, file:// ou chemin local absolu")
    }
}

fn disabled(reason: impl Into<String>) -> RepoStatus {
    RepoStatus::Disabled {
        reason: reason.into(),
    }
}

/// Raison lisible d'un échec de clonage, d'après stderr de git/ssh.
pub fn clone_failure_reason(url: &str, err: &str) -> String {
    let e = err.to_ascii_lowercase();
    let why = if e.contains("permission denied (publickey") || e.contains("publickey") {
        "clé SSH absente ou refusée par le dépôt distant".to_string()
    } else if e.contains("host key verification failed") {
        "clé d'hôte inconnue : se connecter une fois en SSH au serveur pour l'accepter".to_string()
    } else if e.contains("could not resolve hostname")
        || e.contains("could not resolve host")
        || e.contains("network is unreachable")
        || e.contains("connection timed out")
        || e.contains("operation timed out")
    {
        "pas d'accès réseau au dépôt distant".to_string()
    } else if e.contains("délai dépassé") {
        "délai dépassé".to_string()
    } else if e.contains("repository not found")
        || e.contains("does not appear to be a git repository")
        || e.contains("does not exist")
    {
        "dépôt distant introuvable".to_string()
    } else {
        err.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("erreur inconnue")
            .trim()
            .to_string()
    };
    format!("clonage de {url} impossible : {why}")
}

/// Vérifie les outils puis clone ou contrôle le clone (voir
/// [`super::ensure_clone`]).
pub async fn ensure_clone_with(config: &RepoConfig, opts: &CloneOptions) -> RepoStatus {
    for tool in &opts.required_tools {
        let mut cmd = git::command(&tool.program);
        cmd.args(&tool.args);
        let ok = matches!(
            git::run(cmd, Duration::from_secs(30), &tool.label).await,
            Ok(o) if o.ok
        );
        if !ok {
            return disabled(format!(
                "{} introuvable : /ameliorer demande git, Node, npm et cargo",
                tool.label
            ));
        }
    }
    if let Err(reason) = check_url(&config.url) {
        return disabled(reason);
    }
    let commit = config.commit.trim();
    if commit.is_empty() || commit == "unknown" {
        return disabled("commit de compilation inconnu (exe compilé hors d'un dépôt git)");
    }
    if is_clone(&config.dir) {
        return check_existing(config, opts).await;
    }
    let non_empty = std::fs::read_dir(&config.dir)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if non_empty {
        return disabled(format!(
            "le dossier {} existe et n'est pas un clone git",
            config.dir.display()
        ));
    }
    clone_fresh(config, opts).await
}

/// Premier clone : dans un dossier temporaire voisin, placé sur le commit de
/// compilation, configuré (pas de push, `node_modules` ignoré), puis renommé.
async fn clone_fresh(config: &RepoConfig, opts: &CloneOptions) -> RepoStatus {
    let dir = &config.dir;
    let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return disabled(format!("dossier du clone invalide : {}", dir.display()));
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        return disabled(format!(
            "dossier {} non inscriptible : {e}",
            parent.display()
        ));
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| REPO_DIR_NAME.into());
    let tmp = parent.join(format!("{name}.clone-tmp"));
    if tmp.exists() {
        if let Err(e) = std::fs::remove_dir_all(&tmp) {
            return disabled(format!("{} non supprimable : {e}", tmp.display()));
        }
    }
    let tmp_s = tmp.to_string_lossy().into_owned();
    if let Err(e) = git_timeout(
        parent,
        &[
            "clone",
            "--no-checkout",
            "--origin",
            "origin",
            "--",
            &config.url,
            &tmp_s,
        ],
        opts.clone_timeout,
    )
    .await
    {
        let _ = std::fs::remove_dir_all(&tmp);
        return disabled(clone_failure_reason(&config.url, &e));
    }
    let status = place_new_clone(config, &tmp, opts).await;
    if let Err(reason) = status {
        let _ = std::fs::remove_dir_all(&tmp);
        return disabled(reason);
    }
    // Dossier de destination déjà là mais vide (contrôlé plus haut) : retiré
    // pour que le renommage aboutisse.
    if dir.is_dir() {
        if let Err(e) = std::fs::remove_dir(dir) {
            let _ = std::fs::remove_dir_all(&tmp);
            return disabled(format!("{} non supprimable : {e}", dir.display()));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, dir) {
        let _ = std::fs::remove_dir_all(&tmp);
        return disabled(format!("renommage du clone vers {} : {e}", dir.display()));
    }
    ready(config).await
}

async fn place_new_clone(
    config: &RepoConfig,
    tmp: &Path,
    opts: &CloneOptions,
) -> Result<(), String> {
    let commit = find_commit(tmp, &config.commit, opts)
        .await
        .ok_or_else(|| {
            format!(
                "commit de compilation {} introuvable sur {}",
                config.commit, config.url
            )
        })?;
    git(tmp, &["checkout", "-q", "-B", "main", &commit]).await?;
    configure(tmp).await
}

/// Le commit existe localement, sinon `git fetch origin <commit>`.
async fn find_commit(dir: &Path, commit: &str, opts: &CloneOptions) -> Option<String> {
    if let Some(c) = resolve(dir, commit).await {
        return Some(c);
    }
    let _ = git_timeout(dir, &["fetch", "-q", "origin", commit], opts.clone_timeout).await;
    resolve(dir, commit).await
}

/// Réglages locaux du clone : push impossible, `node_modules` ignoré (même
/// en jonction), identité de commit si l'utilisateur n'en a pas.
async fn configure(dir: &Path) -> Result<(), String> {
    git(dir, &["remote", "set-url", "--push", "origin", NO_PUSH_URL]).await?;
    let common = git(dir, &["rev-parse", "--git-common-dir"]).await?;
    let common = {
        let p = PathBuf::from(common.trim());
        if p.is_absolute() {
            p
        } else {
            dir.join(p)
        }
    };
    let exclude = common.join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == "/node_modules") {
        let _ = std::fs::create_dir_all(common.join("info"));
        let mut next = current;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str("/node_modules\n");
        std::fs::write(&exclude, next).map_err(|e| format!("{} : {e}", exclude.display()))?;
    }
    if !git_ok(dir, &["config", "user.email"]).await? {
        git(dir, &["config", "user.email", "orchestrator@localhost"]).await?;
    }
    if !git_ok(dir, &["config", "user.name"]).await? {
        git(dir, &["config", "user.name", "Orchestrator"]).await?;
    }
    Ok(())
}

/// Clone déjà là : sur `main`, et `main` contient le commit de l'exe (sinon
/// avance en fast-forward si possible, ou désactivation : clone divergé).
async fn check_existing(config: &RepoConfig, opts: &CloneOptions) -> RepoStatus {
    let dir = &config.dir;
    if resolve(dir, "refs/heads/main").await.is_none() {
        return disabled(format!("branche main absente du clone {}", dir.display()));
    }
    match git(dir, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(b) if b.trim() == "main" => {}
        _ => {
            return disabled(format!(
                "le clone {} n'est pas sur la branche main",
                dir.display()
            ))
        }
    }
    let Some(commit) = find_commit(dir, &config.commit, opts).await else {
        return disabled(format!(
            "commit de compilation {} introuvable dans le clone ni sur {}",
            config.commit, config.url
        ));
    };
    let contained = git_ok(dir, &["merge-base", "--is-ancestor", &commit, "main"])
        .await
        .unwrap_or(false);
    if !contained {
        // Exe plus récent que le clone : avance en fast-forward si `main` est
        // un ancêtre du commit de l'exe, jamais de réécriture.
        let behind = git_ok(dir, &["merge-base", "--is-ancestor", "main", &commit])
            .await
            .unwrap_or(false);
        if !behind
            || git(dir, &["merge", "--ff-only", "-q", &commit])
                .await
                .is_err()
        {
            return disabled(format!(
                "le main du clone a divergé du commit de l'exe ({})",
                short_hash(&commit)
            ));
        }
    }
    let _ = git(dir, &["worktree", "prune"]).await;
    if let Err(e) = configure(dir).await {
        return disabled(format!("configuration du clone : {e}"));
    }
    ready(config).await
}

async fn ready(config: &RepoConfig) -> RepoStatus {
    match resolve(&config.dir, "refs/heads/main").await {
        Some(head) => RepoStatus::Ready {
            path: config.dir.to_string_lossy().into_owned(),
            url: config.url.clone(),
            head,
        },
        None => disabled("tête de main illisible dans le clone"),
    }
}

fn short_hash(h: &str) -> &str {
    &h[..h.len().min(10)]
}
