//! Lancement de git et des autres outils du cycle (J14, étape 3) : sans
//! console, sans invite (`GIT_TERMINAL_PROMPT=0`, ssh en `BatchMode`), avec un
//! délai maximal ; le processus est tué si le délai expire ou si le cycle est
//! annulé (`kill_on_drop`).

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// Délai d'une commande git locale (worktree, merge, revert…).
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `CREATE_NO_WINDOW` : pas de fenêtre de console sous Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Sortie d'une commande terminée dans le délai.
#[derive(Debug, Clone)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// Dernières lignes non vides de stderr puis stdout, pour un message
    /// d'échec lisible.
    pub fn tail(&self, lines: usize) -> String {
        let mut all: Vec<&str> = self
            .stdout
            .lines()
            .chain(self.stderr.lines())
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .collect();
        if all.len() > lines {
            all.drain(..all.len() - lines);
        }
        all.join("\n")
    }
}

/// Commande sans entrée, sorties capturées, tuée si elle est abandonnée.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Programme npm (`npm.cmd` sous Windows : `Command` ne résout que les `.exe`).
pub fn npm_program() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Lance `cmd` et attend sa fin au plus `timeout`. `Err` : lancement
/// impossible ou délai dépassé (message lisible préfixé par `what`).
pub async fn run(mut cmd: Command, timeout: Duration, what: &str) -> Result<Output, String> {
    match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => Err(format!("{what} : délai dépassé ({} s)", timeout.as_secs())),
        Ok(Err(e)) => Err(format!("{what} : {e}")),
        Ok(Ok(o)) => Ok(Output {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).trim_end().to_string(),
        }),
    }
}

/// Commande git non interactive dans `dir` (`git -C dir …`).
pub fn git_command<I, S>(dir: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command("git");
    c.arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    // Clé SSH de l'utilisateur, sans invite (phrase de passe, hôte inconnu) :
    // échec immédiat plutôt qu'une attente invisible. Un `GIT_SSH_COMMAND`
    // déjà défini (plink, autre clé…) est respecté.
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        c.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=20",
        );
    }
    c
}

/// `git -C dir args` ; `Ok(stdout)` si le code de sortie est 0, sinon
/// `Err("git <args> : <fin de stderr>")`.
pub async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_timeout(dir, args, GIT_TIMEOUT).await
}

/// Comme [`git`], avec un délai choisi.
pub async fn git_timeout(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let what = format!("git {}", args.join(" "));
    let out = run(git_command(dir, args), timeout, &what).await?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(format!("{what} : {}", out.tail(6)))
    }
}

/// `git -C dir args` réussit-il ? (`Err` : git introuvable ou délai.)
pub async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool, String> {
    let what = format!("git {}", args.join(" "));
    run(git_command(dir, args), GIT_TIMEOUT, &what)
        .await
        .map(|o| o.ok)
}

/// Commit complet de `rev` dans `dir`, s'il existe.
pub async fn resolve(dir: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    git(dir, &["rev-parse", "--verify", "--quiet", &spec])
        .await
        .ok()
        .filter(|s| !s.is_empty())
}

/// Hash court (`git rev-parse --short`) de `rev`.
pub async fn short(dir: &Path, rev: &str) -> Result<String, String> {
    git(dir, &["rev-parse", "--short", rev]).await
}
