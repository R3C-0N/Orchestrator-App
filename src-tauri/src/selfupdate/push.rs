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

use super::git::{self, git};
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
    sneaky_in(git(clone, &["config", "--local", "--list", "-z"]).await)
}

/// [`sneaky_config`] lu par la commande de l'envoi ([`PushEnv::git`]).
pub async fn sneaky_config_env(env: &PushEnv, clone: &Path) -> Option<String> {
    sneaky_in(env.git(clone, &["config", "--local", "--list", "-z"]).await)
}

fn sneaky_in(listed: Result<String, String>) -> Option<String> {
    let listed = match listed {
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
/// configuration locale du clone n'a aucun réglage refusé. Lu par la
/// commande de l'envoi ([`PushEnv::git`]) : rien d'hérité ne peut faire lire
/// un autre dépôt (`GIT_DIR`…) ou une autre configuration.
pub async fn check_target(
    env: &PushEnv,
    clone: &Path,
    url: &str,
    canonical: &str,
) -> Result<(), String> {
    if !same_repo(url, canonical) {
        return Err(format!(
            "envoi réservé au dépôt {canonical} ; repo_url = {url} : envoi désactivé \
             (un fork ou un miroir ne reçoit jamais d'envoi)"
        ));
    }
    let origin = env
        .git(
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
    if let Some(bad) = sneaky_config_env(env, clone).await {
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
    env: &PushEnv,
    timeout: Duration,
) -> Result<(), CheckError> {
    super::repo::check_url(url).map_err(CheckError::Refused)?;
    check_target(env, clone, url, canonical)
        .await
        .map_err(CheckError::Refused)?;
    let commit = env
        .git(
            clone,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/main^{commit}",
            ],
        )
        .await
        .ok()
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| CheckError::Unavailable("branche main absente du clone".into()))?;
    let commit = commit.trim();
    let probe = format!("{commit}:{PROBE_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let mut args: Vec<&str> = PUSH_CONFIG.to_vec();
    args.extend(["push", "--dry-run", "--porcelain", "--", url, &probe]);
    match git_timeout_out(env, clone, &args, timeout).await {
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

/// `git -C dir args` de l'envoi ([`PushEnv::command`]) dans le délai ;
/// `Err` : sortie et erreur de git.
async fn git_timeout_out(
    env: &PushEnv,
    dir: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(), String> {
    let what = format!("git {}", args.join(" "));
    let cmd = env.command(dir, args)?;
    let out = git::run(cmd, timeout, &what).await?;
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
    env: &PushEnv,
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
    if env
        .git(clone, &["merge-base", "--is-ancestor", commit, "main"])
        .await
        .is_err()
    {
        return Err(format!(
            "{} n'est pas dans le main du clone : envoi refusé",
            &commit[..10]
        ));
    }
    check_target(env, clone, url, canonical).await?;
    let refspec = format!("{commit}:refs/heads/{branch}");
    let mut args: Vec<&str> = PUSH_CONFIG.to_vec();
    args.extend(["push", "--porcelain", "--", url, &refspec]);
    git_timeout_out(env, clone, &args, timeout)
        .await
        .map_err(|e| summarize(&e))
}

/// Préfixes de variables héritées retirées de la sonde et de l'envoi.
pub const PUSH_ENV_REMOVED_PREFIXES: &[&str] = &[
    "GIT_CONFIG_KEY_",
    "GIT_CONFIG_VALUE_",
    "GIT_SSL_",
    "GIT_TRACE",
];

/// Variables d'environnement héritées retirées de la sonde et de l'envoi
/// (en plus de [`PUSH_ENV_REMOVED_PREFIXES`]) : configuration passée par
/// l'environnement, ssh ou programmes git choisis ailleurs, dépôt, objets
/// ou index imposés.
pub const PUSH_ENV_REMOVED: &[&str] = &[
    "GIT_EXEC_PATH",
    "GIT_TEMPLATE_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_INDEX_FILE",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_SSH_VARIANT",
    "GIT_ALLOW_PROTOCOL",
    "GIT_PROXY_COMMAND",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// Environnement de la sonde et de l'envoi, coupé de toute configuration
/// que la session pourrait écrire avec le compte de l'utilisateur :
/// `~/.gitconfig` et la configuration système de git ignorés
/// (`GIT_CONFIG_GLOBAL` = fichier vide de l'app, `GIT_CONFIG_NOSYSTEM`),
/// `~/.ssh/config` ignoré (`ssh -F <fichier vide>`), serveur, utilisateur
/// et port imposés, sans mandataire, clé d'hôte vérifiée (`known_hosts` de
/// l'utilisateur). Les clés `IdentityFile` de `~/.ssh/config` qui
/// s'appliquent à l'hôte sont relevées une fois au lancement de l'app
/// ([`ssh_identities`]) et passées explicitement.
#[derive(Debug, Clone)]
pub struct PushEnv {
    /// Dossier des fichiers vides (`empty.gitconfig`, `empty.ssh_config`).
    pub dir: PathBuf,
    /// `git.exe`, chemin absolu résolu une fois au lancement de l'app
    /// ([`resolve_program`] ; `git` s'il n'a pas été trouvé).
    pub git_program: String,
    /// `ssh.exe`, chemin absolu résolu une fois au lancement de l'app (un
    /// faux ssh dans les tests ; `ssh` s'il n'a pas été trouvé).
    pub ssh_program: String,
    /// Hôte du dépôt canonique (`github.com`).
    pub host: String,
    /// Clés relevées au lancement (vide : clés par défaut et agent ssh).
    pub identities: Vec<PathBuf>,
}

/// Chemin pour la ligne de commande ssh (`/` au lieu de `\`).
fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Mot entre apostrophes pour le shell qui lance `GIT_SSH_COMMAND`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl PushEnv {
    /// Commande ssh de l'envoi (valeur de `GIT_SSH_COMMAND`).
    pub fn ssh_command(&self, ssh_config: &Path) -> String {
        let mut parts = vec![
            sh_quote(&self.ssh_program),
            "-F".to_string(),
            sh_quote(&slash(ssh_config)),
        ];
        let mut opts = vec![
            format!("HostName={}", self.host),
            "User=git".into(),
            "Port=22".into(),
            "ProxyCommand=none".into(),
            "ProxyJump=none".into(),
            "BatchMode=yes".into(),
            "StrictHostKeyChecking=yes".into(),
            "ConnectTimeout=20".into(),
            "ServerAliveInterval=15".into(),
            "ServerAliveCountMax=3".into(),
        ];
        if !self.identities.is_empty() {
            opts.push("IdentitiesOnly=yes".into());
            for id in &self.identities {
                // Valeur entre guillemets doubles : OpenSSH coupe sinon un
                // chemin qui contient une espace (vérifié par `ssh -G`).
                opts.push(format!("IdentityFile=\"{}\"", slash(id)));
            }
        }
        for o in opts {
            parts.push("-o".into());
            parts.push(sh_quote(&o));
        }
        parts.join(" ")
    }

    /// Commande git de la sonde et de l'envoi : celle de
    /// [`git::git_command`] (hooks coupés, pas d'invite, `ext::` interdit ;
    /// lancée dans un Job Object avec délai par [`git::run`]), sans
    /// configuration globale ni système, sans variables git héritées
    /// ([`PUSH_ENV_REMOVED`]), avec le ssh de [`PushEnv::ssh_command`] et
    /// `safe.directory` = le clone (la configuration globale de
    /// l'utilisateur, qui pouvait l'autoriser, n'est plus lue).
    pub fn command(&self, clone: &Path, args: &[&str]) -> Result<tokio::process::Command, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{} : {e}", self.dir.display()))?;
        let global = self.dir.join("empty.gitconfig");
        let ssh_config = self.dir.join("empty.ssh_config");
        for f in [&global, &ssh_config] {
            std::fs::write(f, b"").map_err(|e| format!("{} : {e}", f.display()))?;
        }
        let safe = format!("safe.directory={}", slash(clone));
        let mut full: Vec<&str> = vec!["-c", &safe];
        full.extend_from_slice(args);
        let mut c = git::git_command_with(&self.git_program, clone, &full);
        for k in PUSH_ENV_REMOVED {
            c.env_remove(k);
        }
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_ascii_uppercase();
            if PUSH_ENV_REMOVED_PREFIXES.iter().any(|p| k.starts_with(p)) {
                c.env_remove(&k);
            }
        }
        c.env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_SSH_VARIANT", "ssh")
            .env("GIT_SSH_COMMAND", self.ssh_command(&ssh_config));
        Ok(c)
    }
}

/// Chemin absolu de `name` (`git.exe`, `ssh.exe`) dans le `PATH` actuel ;
/// appelé une fois au lancement de l'app, pour que la sonde et l'envoi ne
/// dépendent pas d'un `PATH` modifié ensuite.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

impl PushEnv {
    /// `git -C dir args` local (lecture du clone) par la commande de
    /// l'envoi ; `Ok(stdout)` si le code de sortie est 0.
    pub async fn git(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        let what = format!("git {}", args.join(" "));
        let out = git::run(self.command(dir, args)?, git::GIT_TIMEOUT, &what).await?;
        if out.ok {
            Ok(out.stdout)
        } else {
            Err(format!("{what} : {}", out.tail(6)))
        }
    }
}

/// Motif d'hôte de `~/.ssh/config` (`*` et `?`, sans casse).
fn host_glob(pat: &str, host: &str) -> bool {
    fn m(p: &[u8], h: &[u8]) -> bool {
        match (p.first(), h.first()) {
            (None, None) => true,
            (Some(b'*'), _) => m(&p[1..], h) || (!h.is_empty() && m(p, &h[1..])),
            (Some(b'?'), Some(_)) => m(&p[1..], &h[1..]),
            (Some(a), Some(b)) if a == b => m(&p[1..], &h[1..]),
            _ => false,
        }
    }
    m(
        pat.to_ascii_lowercase().as_bytes(),
        host.to_ascii_lowercase().as_bytes(),
    )
}

/// Clés `IdentityFile` de `~/.ssh/config` (texte `config`) qui s'appliquent
/// à `host` : directives globales et blocs `Host` dont un motif correspond
/// (et aucun motif `!` exclu). Les blocs `Match` et les `Include` sont
/// ignorés (jamais évalués : `Match exec` lancerait une commande). `~` et
/// `%d` sont remplacés par `home` ; une valeur avec un autre `%` ou `${`
/// est ignorée ; seuls les fichiers existants sont gardés.
pub fn ssh_identities(config: &str, host: &str, home: &Path) -> Vec<PathBuf> {
    let mut applies = true;
    let mut out: Vec<PathBuf> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
            Some(i) => (
                &line[..i],
                line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '='),
            ),
            None => (line, ""),
        };
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                let pats: Vec<&str> = rest.split_whitespace().collect();
                let neg = pats
                    .iter()
                    .filter_map(|p| p.strip_prefix('!'))
                    .any(|p| host_glob(p, host));
                let pos = pats
                    .iter()
                    .filter(|p| !p.starts_with('!'))
                    .any(|p| host_glob(p, host));
                applies = pos && !neg;
            }
            "match" => applies = false,
            "identityfile" if applies => {
                let v = rest.trim().trim_matches('"');
                let v = if let Some(r) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
                    home.join(r).to_string_lossy().into_owned()
                } else {
                    v.replace("%d", &home.to_string_lossy())
                };
                if v.is_empty() || v.contains('%') || v.contains("${") || v.contains('"') {
                    continue;
                }
                let p = PathBuf::from(v);
                if p.is_absolute() && p.is_file() && !out.contains(&p) {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
    out
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
