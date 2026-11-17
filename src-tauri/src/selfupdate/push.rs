//! Envoi des améliorations fusionnées sur le dépôt distant (décision 16,
//! app multi-utilisateurs) : réservé au propriétaire de l'app.
//!
//! - Réglage d'activation : `"push": true` dans
//!   `%LOCALAPPDATA%\orchestrator\config.json` (absent ou faux par défaut,
//!   jamais exposé ni activable dans l'interface). Sans lui, rien ne change.
//! - Autorité : le dépôt distant (GitHub). Avant d'activer l'envoi (au
//!   démarrage, clone prêt) et avant chaque envoi, le droit d'écriture est
//!   vérifié par `git push --dry-run <url> <commit>:refs/heads/<sonde>` :
//!   la connexion au service `receive-pack` est authentifiée (GitHub refuse
//!   en SSH comme en HTTPS un utilisateur sans droit d'écriture) et rien
//!   n'est écrit (`--dry-run` n'envoie ni objet ni mise à jour de
//!   référence). Sans droit : envoi désactivé, raison lisible.
//! - Qui pousse : le pipeline, jamais la session dédiée (elle reste privée
//!   de tout moyen de pousser, `pushurl` du clone = `no-push://`). Après une
//!   fusion réussie, seule la branche `auto/…` est envoyée, sous le même
//!   nom, vers l'URL réelle passée en argument, avec l'environnement de
//!   l'app : jamais `main`, jamais `--force` (pas de `+`), jamais de
//!   suppression (source toujours un commit).
//! - Échec : la fusion locale reste acquise ; la branche est gardée dans
//!   `push-pending.json` et renvoyée au cycle suivant et au démarrage.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::git::{self, git, git_ok, resolve};
use super::BRANCH_PREFIX;

/// Délai d'une vérification de droit ou d'un envoi.
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

/// Fichier des envois en attente (sous le dossier des worktrees).
pub const PENDING_FILE: &str = "push-pending.json";

/// Préfixe de la référence sonde de la vérification (jamais écrite).
pub const PROBE_PREFIX: &str = "refs/heads/orchestrator-sonde-ecriture-";

/// Clé `push` de `config.json` : vrai seulement pour le booléen JSON `true`.
pub fn push_requested(config_file: Option<&Path>) -> bool {
    config_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("push")?.as_bool())
        .unwrap_or(false)
}

/// Réglages git imposés à la vérification et à l'envoi, en plus de ceux de
/// [`git::git_command`] (hooks désactivés, pas d'invite, `ext::` interdit) :
/// ni étiquettes, ni sous-modules, ni signature, ni options poussées.
const PUSH_CONFIG: &[&str] = &[
    "-c",
    "push.followTags=false",
    "-c",
    "push.recurseSubmodules=no",
    "-c",
    "push.gpgSign=false",
    "-c",
    "push.pushOption=",
];

/// Réglages locaux refusés avant un envoi (même liste que les contrôles
/// d'après session) : un `credential.helper` ou un `url.*.insteadOf` du
/// clone ne doit jamais servir à l'envoi.
pub const SNEAKY_CONFIG: &str =
    r"^(url\.|core\.sshcommand|core\.hookspath|core\.fsmonitor|include|credential)";

/// Réglage git refusé dans la configuration locale du clone, s'il y en a.
pub async fn sneaky_config(clone: &Path) -> Option<String> {
    let found = git(clone, &["config", "--local", "--get-regexp", SNEAKY_CONFIG])
        .await
        .unwrap_or_default();
    let found = found.trim();
    (!found.is_empty()).then(|| found.to_string())
}

/// Nom de branche envoyable : `auto/…`, format git valide, sans `+`, `:`,
/// `*` ni espace (jamais `main`, jamais de forçage ni de suppression).
pub fn valid_branch(branch: &str) -> bool {
    branch.starts_with(BRANCH_PREFIX)
        && branch.len() > BRANCH_PREFIX.len()
        && !branch.contains(['+', ':', '*', '?', '[', '\\', '^', '~', ' '])
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.ends_with('/')
        && !branch.ends_with(".lock")
        && !branch
            .split('/')
            .any(|c| c.is_empty() || c.starts_with('.'))
}

/// Le texte d'erreur de git dit que l'utilisateur n'a pas le droit
/// d'écrire (et non une panne de réseau).
pub fn is_denied(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("permission to") && e.contains("denied")
        || e.contains("permission denied")
        || e.contains("write access")
        || e.contains("403")
        || e.contains("401")
        || e.contains("authentication failed")
        || e.contains("access denied")
        || e.contains("not allowed")
}

/// Lignes utiles d'un échec de git (refus du distant, erreur ssh, ligne
/// `!` de `--porcelain`), sans les formules génériques ; à défaut, la
/// dernière ligne non vide.
pub fn summarize(err: &str) -> String {
    const NOISE: &[&str] = &[
        "could not read from remote repository",
        "please make sure you have the correct access rights",
        "and the repository exists",
        "failed to push some refs",
        "hint:",
        "to ",
        "done",
    ];
    let useful: Vec<&str> = err
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            !NOISE
                .iter()
                .any(|n| low.starts_with(n) || low.contains(n) && n.len() > 8)
        })
        .collect();
    if useful.is_empty() {
        return err
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("erreur inconnue")
            .to_string();
    }
    let n = useful.len();
    useful[n.saturating_sub(3)..]
        .iter()
        .map(|l| l.replace(char::from(9), " "))
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// Échec d'une vérification de droit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// Le dépôt distant refuse l'écriture à cet utilisateur.
    Denied(String),
    /// Vérification impossible (réseau, délai, git…).
    Unavailable(String),
}

impl CheckError {
    pub fn reason(&self) -> &str {
        match self {
            CheckError::Denied(r) | CheckError::Unavailable(r) => r,
        }
    }
}

/// Vérifie le droit d'écriture sur `url` sans rien écrire :
/// `git push --dry-run --porcelain <url> <commit>:refs/heads/<sonde>`.
pub async fn check_write_access(
    clone: &Path,
    url: &str,
    timeout: Duration,
) -> Result<(), CheckError> {
    super::repo::check_url(url).map_err(CheckError::Denied)?;
    let commit = resolve(clone, "refs/heads/main")
        .await
        .ok_or_else(|| CheckError::Unavailable("branche main absente du clone".into()))?;
    let probe = format!("{commit}:{PROBE_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let mut args: Vec<&str> = PUSH_CONFIG.to_vec();
    args.extend(["push", "--dry-run", "--porcelain", "--", url, &probe]);
    match git_timeout_out(clone, &args, timeout).await {
        Ok(()) => Ok(()),
        Err(e) if is_denied(&e) => Err(CheckError::Denied(format!(
            "pas de droit d'écriture sur {url} : {}",
            summarize(&e)
        ))),
        Err(e) => Err(CheckError::Unavailable(format!(
            "droit d'écriture sur {url} non vérifiable : {}",
            summarize(&e)
        ))),
    }
}

/// `git -C dir args` dans le délai ; `Err` : sortie et erreur de git.
async fn git_timeout_out(dir: &Path, args: &[&str], timeout: Duration) -> Result<(), String> {
    let what = format!("git {}", args.join(" "));
    let out = git::run(git::git_command(dir, args), timeout, &what).await?;
    if out.ok {
        Ok(())
    } else {
        Err(out.tail(6))
    }
}

/// Envoie `commit` sur `refs/heads/<branch>` de `url` : jamais forcé (pas
/// de `+`, pas de `--force`), jamais une suppression, seulement une branche
/// `auto/…` dont `commit` est dans le `main` du clone.
pub async fn push_branch(
    clone: &Path,
    url: &str,
    branch: &str,
    commit: &str,
    timeout: Duration,
) -> Result<(), String> {
    if !valid_branch(branch) {
        return Err(format!("branche refusée à l'envoi : {branch}"));
    }
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("commit invalide : {commit}"));
    }
    super::repo::check_url(url)?;
    if !git_ok(clone, &["merge-base", "--is-ancestor", commit, "main"])
        .await
        .unwrap_or(false)
    {
        return Err(format!(
            "{} n'est pas dans le main du clone : envoi refusé",
            &commit[..10]
        ));
    }
    if let Some(bad) = sneaky_config(clone).await {
        return Err(format!("réglage git refusé dans le clone : {bad}"));
    }
    let refspec = format!("{commit}:refs/heads/{branch}");
    let mut args: Vec<&str> = PUSH_CONFIG.to_vec();
    args.extend(["push", "--porcelain", "--", url, &refspec]);
    git_timeout_out(clone, &args, timeout)
        .await
        .map_err(|e| summarize(&e))
}

/// Envoi en attente (échec précédent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub branch: String,
    pub commit: String,
}

/// Liste des envois en attente (`push-pending.json`).
pub struct PendingList(pub PathBuf);

impl PendingList {
    pub fn read(&self) -> Vec<Pending> {
        std::fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, v: &[Pending]) {
        if v.is_empty() {
            let _ = std::fs::remove_file(&self.0);
            return;
        }
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.0.with_extension("json.tmp");
        let ok = serde_json::to_vec_pretty(v)
            .ok()
            .and_then(|b| std::fs::write(&tmp, b).ok())
            .and_then(|()| std::fs::rename(&tmp, &self.0).ok());
        if ok.is_none() {
            tracing::warn!("{} non écrit", self.0.display());
        }
    }

    /// Ajoute (ou remplace) l'envoi de `branch`.
    pub fn add(&self, p: Pending) {
        let mut v = self.read();
        v.retain(|x| x.branch != p.branch);
        v.push(p);
        self.write(&v);
    }

    /// Retire l'envoi de `branch`.
    pub fn remove(&self, branch: &str) {
        let mut v = self.read();
        let n = v.len();
        v.retain(|x| x.branch != branch);
        if v.len() != n {
            self.write(&v);
        }
    }
}
