//! Lancement de git et des autres outils du cycle (J14, étape 3) : sans
//! console, sans invite (`GIT_TERMINAL_PROMPT=0`, ssh en `BatchMode`), sans
//! hooks, avec un délai maximal. Sous Windows, chaque commande vit dans un Job
//! Object `KILL_ON_JOB_CLOSE` (même procédé que `process::spawn` pour
//! `claude.exe`) : à la fin de la commande, au délai dépassé ou à l'annulation,
//! tout l'arbre est tué (ssh, node lancé par npm, Playwright, Vite, Chromium,
//! exécutables de test de cargo).

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::process::spawn::Job;

/// Délai d'une commande git locale (worktree, merge, revert…).
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `CREATE_NO_WINDOW` : pas de fenêtre de console sous Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_SUSPENDED` : rattaché au Job avant d'avoir pu lancer un enfant.
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// Hooks désactivés pour toutes les commandes git de l'app.
#[cfg(windows)]
const NO_HOOKS: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const NO_HOOKS: &str = "core.hooksPath=/dev/null";

/// ssh sans invite, avec délai de connexion et détection de coupure.
pub const SSH_COMMAND: &str =
    "ssh -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=15 -o ServerAliveCountMax=3";

/// Sortie d'une commande terminée dans le délai.
#[derive(Debug, Clone)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// Dernières lignes non vides de stdout puis stderr, pour un message
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
/// À lancer par [`run`] (Job Object).
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

/// Lance `cmd` dans un Job Object et attend sa fin au plus `timeout`. À la
/// sortie du processus direct, les descendants restants sont tués (ils ne
/// retiennent donc pas les tubes de sortie). `Err` : lancement impossible ou
/// délai dépassé (message lisible préfixé par `what`) ; dans les deux cas
/// tout l'arbre est tué.
pub async fn run(mut cmd: Command, timeout: Duration, what: &str) -> Result<Output, String> {
    let job = Job::new().map_err(|e| format!("{what} : Job Object : {e}"))?;
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut child = cmd.spawn().map_err(|e| format!("{what} : {e}"))?;
    #[cfg(windows)]
    {
        let attached = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("handle de processus indisponible"))
            .and_then(|h| job.assign(h))
            .and_then(|()| {
                let pid = child
                    .id()
                    .ok_or_else(|| std::io::Error::other("pid indisponible"))?;
                win::resume_process_threads(pid)
            });
        if let Err(e) = attached {
            job.terminate();
            return Err(format!("{what} : rattachement au Job Object : {e}"));
        }
    }
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let work = async {
        let read_out = async {
            let mut b = Vec::new();
            if let Some(s) = so.as_mut() {
                let _ = s.read_to_end(&mut b).await;
            }
            b
        };
        let read_err = async {
            let mut b = Vec::new();
            if let Some(s) = se.as_mut() {
                let _ = s.read_to_end(&mut b).await;
            }
            b
        };
        let wait = async {
            let st = child.wait().await;
            // Descendants restants (serveur, ssh…) : tués, tubes fermés.
            job.terminate();
            st
        };
        tokio::join!(wait, read_out, read_err)
    };
    let res = tokio::time::timeout(timeout, work).await;
    // Délai dépassé : le futur `work` est abandonné ; tuer tout l'arbre.
    job.terminate();
    match res {
        Err(_) => Err(format!("{what} : délai dépassé ({} s)", timeout.as_secs())),
        Ok((Err(e), _, _)) => Err(format!("{what} : {e}")),
        Ok((Ok(status), out, err)) => Ok(Output {
            ok: status.success(),
            stdout: String::from_utf8_lossy(&out).trim_end().to_string(),
            stderr: String::from_utf8_lossy(&err).trim_end().to_string(),
        }),
    }
}

/// Commande git non interactive dans `dir` (`git -C dir …`), sans hooks ni
/// fsmonitor, transport `ext::` interdit.
pub fn git_command<I, S>(dir: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // Chemin résolu au démarrage (`tools::program`), sinon le nom nu.
    git_command_with(super::tools::program("git"), dir, args)
}

/// [`git_command`] avec un programme git choisi (chemin absolu de l'envoi).
pub fn git_command_with<P, I, S>(program: P, dir: &Path, args: I) -> Command
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command(program);
    c.arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off"])
        .args(["-c", NO_HOOKS])
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "protocol.ext.allow=never"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    // Clé SSH de l'utilisateur, sans invite (phrase de passe, hôte inconnu) :
    // échec immédiat plutôt qu'une attente invisible. Un `GIT_SSH_COMMAND`
    // déjà défini (plink, autre clé…) est respecté.
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        c.env("GIT_SSH_COMMAND", SSH_COMMAND);
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

#[cfg(windows)]
mod win {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// Relance les threads d'un processus créé avec `CREATE_SUSPENDED`
    /// (même procédé que `process::spawn`, dont la fonction est privée).
    pub fn resume_process_threads(pid: u32) -> std::io::Result<()> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let snap = Owned(snap);
            let mut entry: THREADENTRY32 = zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut more = Thread32First(snap.0, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let th = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !th.is_null() {
                        let th = Owned(th);
                        if ResumeThread(th.0) != u32::MAX {
                            resumed += 1;
                        }
                    }
                }
                more = Thread32Next(snap.0, &mut entry) != 0;
            }
            if resumed == 0 {
                return Err(std::io::Error::other(format!(
                    "aucun thread relancé pour le pid {pid}"
                )));
            }
            Ok(())
        }
    }
}
