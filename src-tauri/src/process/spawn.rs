//! Lancement de `claude.exe` : arguments de la spec 4.1, aucune fenêtre console
//! (`CREATE_NO_WINDOW`), et rattachement à un Job Object `KILL_ON_JOB_CLOSE`
//! pour que tout l'arbre (claude.exe, bash, serveurs MCP) meure avec l'app.
//!
//! Le processus est créé suspendu (`CREATE_SUSPENDED`), rattaché au Job, puis
//! relancé : aucun processus enfant ne peut naître hors du Job.

use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

/// Commande définitive du prototype 1 (spec 4.1), testée dans `t8`.
pub const BASE_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--forward-subagent-text",
    "--permission-mode",
    "bypassPermissions",
    "--permission-prompt-tool",
    "stdio",
];

/// Options de lancement d'une session.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    pub resume: Option<String>,
    pub model: Option<String>,
    /// Arguments supplémentaires (tests : `--no-session-persistence` ;
    /// session d'amélioration : `--disallowedTools …`).
    pub extra_args: Vec<String>,
    /// Variables d'environnement ajoutées à celles de l'app (session
    /// d'amélioration : git sans réseau, sans hooks, sans invite).
    pub env: Vec<(String, String)>,
    /// Liste blanche de l'environnement hérité (session d'amélioration) :
    /// `Some` → le processus ne reçoit de l'app que les variables dont le
    /// nom (sans casse) est dans la liste, ou commence par le préfixe d'une
    /// entrée finie par `*` ; puis `env`. `None` : tout l'environnement.
    pub env_allow: Option<Vec<String>>,
}

/// Le nom `name` passe la liste blanche `allow` (sans casse ; `PREFIXE*`).
pub fn env_allowed(name: &str, allow: &[String]) -> bool {
    let n = name.to_ascii_uppercase();
    allow.iter().any(|a| {
        let a = a.to_ascii_uppercase();
        match a.strip_suffix('*') {
            Some(prefix) => n.starts_with(prefix),
            None => n == a,
        }
    })
}

/// Valide les valeurs venues de l'interface avant de les passer en arguments :
/// `resume` doit être un UUID canonique (`8-4-4-4-12`) ; `model` ne doit pas
/// commencer par `-` (pas d'injection d'option) ni contenir d'espace ou de
/// caractère de contrôle.
pub fn validate_launch_options(opts: &LaunchOptions) -> Result<(), String> {
    if let Some(r) = &opts.resume {
        if !canonical_uuid(r) {
            return Err(format!("resume doit être un UUID de session : {r:?}"));
        }
    }
    // Jalon J16 : `--session-id <uuid>` (premier tour de la session
    // d'amélioration) passe par `extra_args` ; même règle que `resume`, une
    // seule fois, et pas avec `resume` (le CLI exigerait `--fork-session`).
    let ids: Vec<usize> = opts
        .extra_args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "--session-id" || a.starts_with("--session-id="))
        .map(|(i, _)| i)
        .collect();
    if ids.len() > 1 {
        return Err("--session-id donné plusieurs fois".into());
    }
    if let Some(&i) = ids.first() {
        let a = &opts.extra_args[i];
        let v = match a.strip_prefix("--session-id=") {
            Some(v) => Some(v),
            None => opts.extra_args.get(i + 1).map(String::as_str),
        };
        match v {
            Some(v) if canonical_uuid(v) => {}
            v => return Err(format!("--session-id doit être un UUID de session : {v:?}")),
        }
        if opts.resume.is_some() {
            return Err("--session-id et resume sont exclusifs".into());
        }
    }
    if let Some(m) = &opts.model {
        if m.is_empty() || m.starts_with('-') {
            return Err(format!("modèle invalide : {m:?}"));
        }
        if m.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("modèle invalide (espace ou contrôle) : {m:?}"));
        }
    }
    Ok(())
}

/// UUID canonique `8-4-4-4-12` (identifiant de session du CLI).
fn canonical_uuid(s: &str) -> bool {
    s.len() == 36 && s.as_bytes()[8] == b'-' && uuid::Uuid::try_parse(s).is_ok()
}

pub fn build_args(opts: &LaunchOptions) -> Vec<String> {
    let mut args: Vec<String> = BASE_ARGS.iter().map(|s| s.to_string()).collect();
    if let Some(r) = &opts.resume {
        args.push("--resume".into());
        args.push(r.clone());
    }
    if let Some(m) = &opts.model {
        args.push("--model".into());
        args.push(m.clone());
    }
    args.extend(opts.extra_args.iter().cloned());
    args
}

/// Processus lancé et son Job Object. Fermer le Job (drop) tue tout l'arbre.
pub struct Spawned {
    pub child: Child,
    pub job: Job,
    pub pid: u32,
    pub args: Vec<String>,
}

pub fn spawn_claude(exe: &Path, cwd: &Path, args: &[String]) -> std::io::Result<Spawned> {
    spawn_claude_env(exe, cwd, args, &[], None)
}

/// [`spawn_claude`] avec des variables d'environnement en plus (`env`) et en
/// liste blanche de l'environnement hérité (`env_allow`, appliquée avant
/// `env`).
pub fn spawn_claude_env(
    exe: &Path,
    cwd: &Path,
    args: &[String],
    env: &[(String, String)],
    env_allow: Option<&[String]>,
) -> std::io::Result<Spawned> {
    let job = Job::new()?;
    let mut cmd = Command::new(exe);
    if let Some(allow) = env_allow {
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            if env_allowed(&k.to_string_lossy(), allow) {
                cmd.env(k, v);
            }
        }
    }
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("pid indisponible après le lancement"))?;

    #[cfg(windows)]
    {
        let handle = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("handle de processus indisponible"))?;
        if let Err(e) = job.assign(handle) {
            // Ne pas laisser un processus suspendu orphelin.
            job.terminate();
            return Err(e);
        }
        if let Err(e) = win::resume_process_threads(pid) {
            job.terminate();
            return Err(e);
        }
    }

    Ok(Spawned {
        child,
        job,
        pid,
        args: args.to_vec(),
    })
}

/// Job Object Windows `KILL_ON_JOB_CLOSE`. Ailleurs : coquille vide.
pub struct Job {
    #[cfg(windows)]
    handle: win::Handle,
}

impl Job {
    #[cfg(windows)]
    pub fn new() -> std::io::Result<Self> {
        Ok(Job {
            handle: win::create_kill_on_close_job()?,
        })
    }

    #[cfg(not(windows))]
    pub fn new() -> std::io::Result<Self> {
        Ok(Job {})
    }

    #[cfg(windows)]
    pub fn assign(&self, process: std::os::windows::io::RawHandle) -> std::io::Result<()> {
        win::assign(&self.handle, process)
    }

    /// Tue immédiatement tous les processus du Job.
    pub fn terminate(&self) {
        #[cfg(windows)]
        win::terminate(&self.handle);
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::null;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// Handle possédé, fermé au drop.
    pub struct Handle(HANDLE);
    // Un HANDLE de Job est utilisable depuis n'importe quel thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    pub fn create_kill_on_close_job() -> std::io::Result<Handle> {
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let handle = Handle(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(handle)
        }
    }

    pub fn assign(job: &Handle, process: std::os::windows::io::RawHandle) -> std::io::Result<()> {
        let ok = unsafe { AssignProcessToJobObject(job.0, process as HANDLE) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn terminate(job: &Handle) {
        unsafe { TerminateJobObject(job.0, 1) };
    }

    /// Relance les threads d'un processus créé avec `CREATE_SUSPENDED`
    /// (`std`/tokio ne rendent pas le handle du thread principal).
    pub fn resume_process_threads(pid: u32) -> std::io::Result<()> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let snap = Handle(snap);
            let mut entry: THREADENTRY32 = zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut more = Thread32First(snap.0, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let th = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !th.is_null() {
                        let th = Handle(th);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_match_spec_4_1() {
        let a = build_args(&LaunchOptions {
            resume: Some("abc".into()),
            model: Some("haiku".into()),
            extra_args: vec![],
            ..LaunchOptions::default()
        });
        let joined = a.join(" ");
        assert!(joined.starts_with(
            "-p --input-format stream-json --output-format stream-json --verbose \
             --include-partial-messages --forward-subagent-text \
             --permission-mode bypassPermissions --permission-prompt-tool stdio"
        ));
        assert!(joined.ends_with("--resume abc --model haiku"));
    }

    fn opts(resume: Option<&str>, model: Option<&str>) -> LaunchOptions {
        LaunchOptions {
            resume: resume.map(str::to_string),
            model: model.map(str::to_string),
            extra_args: vec![],
            ..LaunchOptions::default()
        }
    }

    #[test]
    fn validate_accepts_uuid_resume_and_model_aliases() {
        assert!(validate_launch_options(&opts(None, None)).is_ok());
        assert!(validate_launch_options(&opts(
            Some("77cbd82d-c401-4ae6-a84e-8439cd39aecd"),
            Some("haiku")
        ))
        .is_ok());
        assert!(validate_launch_options(&opts(None, Some("claude-sonnet-5-5"))).is_ok());
        assert!(validate_launch_options(&opts(None, Some("opus[1m]"))).is_ok());
    }

    #[test]
    fn validate_rejects_option_injection_and_bad_resume() {
        for bad in [
            "--dangerously-skip-permissions",
            "-p",
            "abc",
            "",
            "77cbd82dc4014ae6a84e8439cd39aecd", // UUID sans tirets
            "{77cbd82d-c401-4ae6-a84e-8439cd39aecd}", // accolades
            "urn:uuid:77cbd82d-c401-4ae6-a84e-8439cd39aecd",
            "C:\\x\\session.jsonl",
        ] {
            assert!(
                validate_launch_options(&opts(Some(bad), None)).is_err(),
                "resume {bad:?} doit être refusé"
            );
        }
        for bad in ["-x", "--model", "", "haiku --bare", "haiku\n"] {
            assert!(
                validate_launch_options(&opts(None, Some(bad))).is_err(),
                "model {bad:?} doit être refusé"
            );
        }
    }

    /// J16 : `--session-id <uuid>` (premier tour de la session
    /// d'amélioration) dans `extra_args`, `resume` aux tours suivants.
    #[test]
    fn validate_session_id_in_extra_args() {
        let id = "77cbd82d-c401-4ae6-a84e-8439cd39aecd";
        let with = |resume: Option<&str>, extra: &[&str]| LaunchOptions {
            resume: resume.map(str::to_string),
            extra_args: extra.iter().map(|s| s.to_string()).collect(),
            ..LaunchOptions::default()
        };
        let ok = with(None, &["--disallowedTools", "X", "--session-id", id]);
        assert!(validate_launch_options(&ok).is_ok());
        assert!(build_args(&ok).ends_with(&["--session-id".into(), id.into()]));
        assert!(validate_launch_options(&with(None, &[&format!("--session-id={id}")])).is_ok());
        assert!(validate_launch_options(&with(Some(id), &[])).is_ok());
        for bad in [
            with(None, &["--session-id"]),
            with(None, &["--session-id", "--resume"]),
            with(None, &["--session-id", "abc"]),
            with(None, &["--session-id=abc"]),
            with(None, &["--session-id", id, "--session-id", id]),
            with(Some(id), &["--session-id", id]),
        ] {
            assert!(validate_launch_options(&bad).is_err(), "{bad:?}");
        }
    }
}
