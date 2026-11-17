//! Envoi des améliorations fusionnées sur le dépôt distant (décision 16,
//! app multi-utilisateurs) : réservé au propriétaire de l'app.
//!
//! - Réglage d'activation : `"push": true` dans
//!   `%LOCALAPPDATA%\orchestrator\config.json` (absent ou faux par défaut,
//!   jamais exposé ni activable dans l'interface). Sans lui, rien ne change.
//! - Destination unique : le dépôt canonique compilé
//!   ([`super::DEFAULT_ORIGIN`]). Si `repo_url` ou l'URL d'origine du clone
//!   (`remote.origin.url`) désigne un autre dépôt (fork, miroir, dépôt
//!   local), l'envoi est désactivé ; le clone depuis un autre `repo_url`
//!   reste possible. Formes reconnues comme équivalentes : `git@hôte:chemin`
//!   et `ssh://git@hôte/chemin` (même hôte, même chemin, casse comprise).
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
//!   suppression (source toujours un commit). La configuration locale du
//!   clone est contrôlée au moment même de la sonde et de l'envoi
//!   ([`sneaky_config`]).
//! - Échec : la fusion locale reste acquise ; la branche est gardée en
//!   attente (mémoire du pipeline, recopiée dans `push-pending.json`) et
//!   renvoyée au cycle suivant et au démarrage.

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

/// Clé SSH d'un dépôt : `(hôte, chemin)` pour les seules formes
/// `git@hôte:chemin` et `ssh://git@hôte/chemin` (utilisateur `git`, pas de
/// port, chemin non vide sans `..` ni espace). Toute autre forme (HTTPS,
/// autre utilisateur, port, chemin local) : `None`.
pub fn ssh_repo_key(url: &str) -> Option<(String, String)> {
    let (host, path) = if let Some(rest) = url.strip_prefix("ssh://git@") {
        let (host, path) = rest.split_once('/')?;
        (host, path)
    } else {
        let rest = url.strip_prefix("git@")?;
        rest.split_once(':')?
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let path_ok = !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.chars().any(|c| c.is_whitespace() || c.is_control())
        && !path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..");
    (host_ok && path_ok).then(|| (host.to_string(), path.to_string()))
}

/// `a` et `b` désignent le même dépôt SSH (voir [`ssh_repo_key`]).
pub fn same_repo(a: &str, b: &str) -> bool {
    match (ssh_repo_key(a), ssh_repo_key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Réglage local du clone refusé avant un envoi et après la session :
/// réécriture d'URL, commande ssh, hooks, inclusions, identifiants,
/// mandataire, en-têtes ou certificats HTTP, aide à la saisie, transports,
/// dépôts distants autres que `remote.origin.{url,fetch,pushurl}`.
pub fn is_sneaky_key(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let section = n.split('.').next().unwrap_or("");
    let key = n.rsplit('.').next().unwrap_or("");
    match section {
        "url" | "include" | "includeif" | "credential" | "protocol" => true,
        "core" => matches!(
            key,
            "sshcommand" | "hookspath" | "fsmonitor" | "askpass" | "gitproxy"
        ),
        "http" => matches!(key, "proxy" | "extraheader" | "sslverify" | "sslcainfo"),
        "remote" => !matches!(
            n.as_str(),
            "remote.origin.url" | "remote.origin.fetch" | "remote.origin.pushurl"
        ),
        _ => false,
    }
}

/// Réglages refusés ([`is_sneaky_key`]) dans la configuration locale du
/// clone, s'il y en a (une ligne `nom=valeur` par réglage).
pub async fn sneaky_config(clone: &Path) -> Option<String> {
    let listed = match git(clone, &["config", "--local", "--list", "-z"]).await {
        Ok(s) => s,
        Err(e) => return Some(format!("configuration illisible : {e}")),
    };
    let found: Vec<String> = listed
        .split('\0')
        .filter(|e| !e.is_empty())
        .filter_map(|e| {
            let (name, value) = e.split_once('\n').unwrap_or((e, ""));
            is_sneaky_key(name).then(|| format!("{name}={value}"))
        })
        .collect();
    (!found.is_empty()).then(|| found.join(" ; "))
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

/// `line` contient `code` (`error: 403`…) non suivi d'un chiffre.
fn has_code(line: &str, code: &str) -> bool {
    line.match_indices(code).any(|(i, _)| {
        !line[i + code.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

/// Le texte d'erreur de git dit que l'utilisateur n'a pas le droit
/// d'écrire (et non une panne de réseau). Formes précises seulement ; les
/// lignes qui citent une référence (`refs/heads/…`, refspec de
/// `--porcelain`) ou la destination (`To …`) sont ignorées : un SHA ou un
/// identifiant qui contient « 403 » ne compte pas.
pub fn is_denied(err: &str) -> bool {
    err.lines().map(str::trim).any(|l| {
        let low = l.to_ascii_lowercase();
        if low.contains("refs/heads/") || low.starts_with("to ") {
            return false;
        }
        (low.contains("permission to ") && low.contains(" denied to "))
            || low.contains("permission denied (publickey")
            || has_code(&low, "error: 403")
            || has_code(&low, "error: 401")
            || low.contains("write access to repository not granted")
            || low.contains("authentication failed for")
    })
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
    /// Envoi interdit par l'app (autre dépôt que le canonique, réglage git
    /// douteux dans le clone) : rien n'est tenté ni mis en attente.
    Refused(String),
    /// Vérification impossible (réseau, délai, git…) : à refaire.
    Unavailable(String),
}

impl CheckError {
    pub fn reason(&self) -> &str {
        match self {
            CheckError::Denied(r) | CheckError::Refused(r) | CheckError::Unavailable(r) => r,
        }
    }

    /// Refus définitif pour cette exécution (pas de nouvel essai, pas
    /// d'attente).
    pub fn is_final(&self) -> bool {
        !matches!(self, CheckError::Unavailable(_))
    }
}

/// Destination et clone acceptables pour un envoi : `url` est le dépôt
/// `canonical`, l'URL d'origine du clone aussi (une seule valeur), et la
/// configuration locale du clone n'a aucun réglage refusé.
pub async fn check_target(clone: &Path, url: &str, canonical: &str) -> Result<(), String> {
    if !same_repo(url, canonical) {
        return Err(format!(
            "envoi réservé au dépôt {canonical} ; repo_url = {url} : envoi désactivé \
             (un fork ou un miroir ne reçoit jamais d'envoi)"
        ));
    }
    let origin = git(
        clone,
        &["config", "--local", "--get-all", "remote.origin.url"],
    )
    .await
    .unwrap_or_default();
    let origins: Vec<&str> = origin.lines().map(str::trim).collect();
    if origins.len() != 1 || !same_repo(origins[0], canonical) {
        return Err(format!(
            "URL d'origine du clone ({}) différente de {canonical} : envoi désactivé",
            origins.join(", ")
        ));
    }
    if let Some(bad) = sneaky_config(clone).await {
        return Err(format!("réglage git refusé dans le clone : {bad}"));
    }
    Ok(())
}

/// Vérifie le droit d'écriture sur `url` sans rien écrire :
/// `git push --dry-run --porcelain <url> <commit>:refs/heads/<sonde>`,
/// après [`check_target`].
pub async fn check_write_access(
    clone: &Path,
    url: &str,
    canonical: &str,
    timeout: Duration,
) -> Result<(), CheckError> {
    super::repo::check_url(url).map_err(CheckError::Refused)?;
    check_target(clone, url, canonical)
        .await
        .map_err(CheckError::Refused)?;
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
/// `auto/…` dont `commit` est dans le `main` du clone, vers le dépôt
/// `canonical` ; le clone est contrôlé ([`check_target`]) juste avant.
pub async fn push_branch(
    clone: &Path,
    url: &str,
    canonical: &str,
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
    check_target(clone, url, canonical).await?;
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

/// Fichier des envois en attente (`push-pending.json`) : recopie de la
/// liste tenue en mémoire par le pipeline, relue seulement à sa création.
pub struct PendingFile(pub PathBuf);

impl PendingFile {
    /// Contenu du fichier : liste vide s'il est absent, `None` s'il est
    /// illisible.
    pub fn read(&self) -> Option<Vec<Pending>> {
        match std::fs::read(&self.0) {
            Ok(b) => serde_json::from_slice(&b).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
            Err(_) => None,
        }
    }

    /// Remplace le fichier par `v` (supprimé si `v` est vide).
    pub fn write(&self, v: &[Pending]) {
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
}
