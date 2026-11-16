//! Journal de l'app (plan 1.9) : `%LOCALAPPDATA%\orchestrator\logs\`.
//!
//! - `orchestrator.AAAA-MM-JJ.log` : journal `tracing`, rotation quotidienne,
//!   7 fichiers gardés ;
//! - `raw\<horodatage>-<session>.ndjson` : copie du flux brut de chaque session,
//!   au format des fixtures (`fixtures/stream-json/README`) : lignes du CLI
//!   telles quelles, `>>> ` pour ce que l'app écrit sur stdin, `STDERR …`,
//!   `EXIT n`. Sert à fabriquer une fixture à partir d'un bogue réel. Ces
//!   fichiers contiennent la conversation : ils restent locaux (7 jours).
//!
//! Sans [`init`] (tests, binaire `probe`), rien n'est écrit : les macros
//! `tracing` sont sans effet et [`raw_dir`] vaut `None`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// Jours de journal gardés.
pub const KEEP_DAYS: u64 = 7;

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Dossier du journal, si [`init`] a réussi.
pub fn log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

/// Dossier des flux bruts par session, si [`init`] a réussi.
pub fn raw_dir() -> Option<PathBuf> {
    log_dir().map(|d| d.join("raw"))
}

/// `%LOCALAPPDATA%\orchestrator\logs` ; `ORCH_LOG_DIR` le remplace.
pub fn default_log_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ORCH_LOG_DIR") {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("orchestrator").join("logs"))
}

/// Installe le journal. Renvoie le dossier, ou `None` si le journal est
/// indisponible (l'app continue sans journal). Appelée une fois, au démarrage.
pub fn init() -> Option<PathBuf> {
    let dir = default_log_dir()?;
    if let Err(e) = init_in(&dir) {
        eprintln!("journal indisponible dans {} : {e}", dir.display());
        return None;
    }
    Some(dir)
}

fn init_in(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir.join("raw"))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("orchestrator")
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS as usize)
        .build(dir)
        .map_err(io::Error::other)?;
    // Écriture synchrone (pas de tâche de fond à vider à la sortie : l'app se
    // termine par `std::process::exit` dans `RunEvent::Exit`).
    let level = match std::env::var("ORCH_LOG").as_deref() {
        Ok("debug") => tracing::Level::DEBUG,
        Ok("trace") => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(appender))
        .with_ansi(false)
        .with_max_level(level)
        .with_target(true)
        .try_init()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let _ = LOG_DIR.set(dir.to_path_buf());

    // Les paniques vont aussi au journal.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(target: "panic", "{info}");
        prev(info);
    }));

    let removed = purge_old_raw(&dir.join("raw"), Duration::from_secs(KEEP_DAYS * 86_400));
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        dir = %dir.display(),
        raw_purged = removed,
        "démarrage de l'app"
    );
    Ok(())
}

/// Supprime les flux bruts plus vieux que `max_age`. Renvoie leur nombre.
pub fn purge_old_raw(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("ndjson") {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= max_age);
        if old && fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

/// Copie du flux brut d'une session, au format des fixtures.
pub struct RawLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl RawLog {
    /// Crée `dir/<epoch_ms>-<session>.ndjson`.
    pub fn create(dir: &Path, session: &str, epoch_ms: u64) -> io::Result<RawLog> {
        fs::create_dir_all(dir)?;
        let safe: String = session
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(40)
            .collect();
        let path = dir.join(format!("{epoch_ms}-{safe}.ndjson"));
        let file = File::create(&path)?;
        Ok(RawLog {
            path,
            file: Mutex::new(Some(file)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ligne reçue du CLI, telle quelle.
    pub fn cli(&self, line: &str) {
        self.put("", line);
    }

    /// Ligne écrite sur stdin par l'app.
    pub fn sent(&self, line: &str) {
        self.put(">>> ", line);
    }

    pub fn stderr(&self, line: &str) {
        self.put("STDERR ", line);
    }

    pub fn exit(&self, code: Option<i32>) {
        let c = code.map_or_else(|| "?".into(), |c| c.to_string());
        self.put("EXIT ", &c);
        // Plus rien après la fin du processus.
        self.file.lock().unwrap().take();
    }

    /// Une ligne = un seul `write_all` (pas de tampon : rien de perdu si l'app
    /// meurt). Une erreur d'écriture coupe la copie, une seule trace au journal.
    fn put(&self, prefix: &str, line: &str) {
        let mut guard = self.file.lock().unwrap();
        let Some(f) = guard.as_mut() else { return };
        let mut buf = Vec::with_capacity(prefix.len() + line.len() + 1);
        buf.extend_from_slice(prefix.as_bytes());
        // Une ligne du flux ne contient pas de `\n` ; stderr non plus (découpé
        // par le lecteur). Par sécurité, on n'écrit qu'une ligne.
        buf.extend_from_slice(line.replace('\n', "\\n").as_bytes());
        buf.push(b'\n');
        if let Err(e) = f.write_all(&buf) {
            tracing::warn!(path = %self.path.display(), "copie du flux brut arrêtée : {e}");
            guard.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fixture::{classify, FixtureLine};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "orch-logging-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn raw_log_reads_back_as_a_fixture() {
        let d = tmp("raw");
        let log = RawLog::create(&d, "abc/../x", 42).unwrap();
        assert_eq!(log.path().file_name().unwrap(), "42-abcx.ndjson");
        log.sent(r#"{"type":"user"}"#);
        log.cli(r#"{"type":"result"}"#);
        log.stderr("attention");
        log.exit(Some(0));
        log.cli("après la fin : ignorée");
        let text = fs::read_to_string(log.path()).unwrap();
        let kinds: Vec<_> = text.lines().map(classify).collect();
        assert_eq!(
            kinds,
            vec![
                FixtureLine::Sent(r#"{"type":"user"}"#),
                FixtureLine::Cli(r#"{"type":"result"}"#),
                FixtureLine::Stderr("attention"),
                FixtureLine::Exit(Some(0)),
            ]
        );
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn purge_keeps_recent_files() {
        let d = tmp("purge");
        fs::write(d.join("a.ndjson"), "x").unwrap();
        fs::write(d.join("b.log"), "x").unwrap();
        assert_eq!(purge_old_raw(&d, Duration::from_secs(3600)), 0);
        assert_eq!(purge_old_raw(&d, Duration::ZERO), 1);
        assert!(!d.join("a.ndjson").exists());
        assert!(d.join("b.log").exists(), "seuls les .ndjson sont visés");
        let _ = fs::remove_dir_all(d);
    }
}
