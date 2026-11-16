//! Journal de l'app (plan 1.9) : `%LOCALAPPDATA%\orchestrator\logs\`.
//!
//! - `orchestrator.AAAA-MM-JJ.log` : journal `tracing`, rotation quotidienne,
//!   7 fichiers gardés ;
//! - `raw\<horodatage>-<session>.ndjson` : copie du flux brut de chaque session,
//!   au format des fixtures (`fixtures/stream-json/README`) : lignes du CLI
//!   telles quelles, `>>> ` pour ce que l'app écrit sur stdin, `STDERR …`,
//!   `EXIT n`. Sert à fabriquer une fixture à partir d'un bogue réel. Ces
//!   fichiers contiennent la conversation (contenus de fichiers, sorties de
//!   commandes) : ils restent locaux et sont bornés : 7 jours,
//!   [`RAW_FILE_MAX`] par session (la copie s'arrête ensuite, avec une ligne
//!   `STDERR [orchestrator] …` qui le signale), [`RAW_DIR_MAX`] pour le
//!   dossier (les plus anciens sont supprimés). `ORCH_RAW_LOG=0` désactive
//!   la copie.
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
/// Taille maximale du flux brut d'une session.
pub const RAW_FILE_MAX: u64 = 50 * 1000 * 1000;
/// Taille maximale du dossier des flux bruts.
pub const RAW_DIR_MAX: u64 = 500 * 1000 * 1000;

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Dossier du journal, si [`init`] a réussi.
pub fn log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

/// Dossier des flux bruts par session, si [`init`] a réussi et si la copie
/// n'est pas désactivée (`ORCH_RAW_LOG=0`).
pub fn raw_dir() -> Option<PathBuf> {
    if !raw_enabled(std::env::var("ORCH_RAW_LOG").ok().as_deref()) {
        return None;
    }
    log_dir().map(|d| d.join("raw"))
}

/// Interrupteur de la copie du flux brut : activée sauf `0`, `false`, `off`, `no`.
pub fn raw_enabled(var: Option<&str>) -> bool {
    !matches!(
        var.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
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

    let removed = purge_raw(
        &dir.join("raw"),
        Duration::from_secs(KEEP_DAYS * 86_400),
        RAW_DIR_MAX,
    );
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        dir = %dir.display(),
        raw_purged = removed,
        "démarrage de l'app"
    );
    Ok(())
}

/// Supprime les flux bruts plus vieux que `max_age`, puis les plus anciens
/// tant que le dossier dépasse `max_total` octets. Renvoie leur nombre.
pub fn purge_raw(dir: &Path, max_age: Duration, max_total: u64) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().unwrap_or(now), m.len(), e.path()))
        })
        .collect();
    // Les plus anciens d'abord.
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    let mut n = 0;
    for (modified, len, p) in files {
        let too_old = now.duration_since(modified).is_ok_and(|age| age >= max_age);
        if (too_old || total > max_total) && fs::remove_file(&p).is_ok() {
            total -= len;
            n += 1;
        }
    }
    n
}

/// Copie du flux brut d'une session, au format des fixtures.
pub struct RawLog {
    path: PathBuf,
    max_bytes: u64,
    file: Mutex<Option<(File, u64)>>,
}

impl RawLog {
    /// Crée `dir/<epoch_ms>-<session>.ndjson`, plafonné à [`RAW_FILE_MAX`],
    /// après avoir ramené le dossier sous [`RAW_DIR_MAX`].
    pub fn create(dir: &Path, session: &str, epoch_ms: u64) -> io::Result<RawLog> {
        fs::create_dir_all(dir)?;
        purge_raw(
            dir,
            Duration::from_secs(KEEP_DAYS * 86_400),
            RAW_DIR_MAX.saturating_sub(RAW_FILE_MAX),
        );
        Self::create_capped(dir, session, epoch_ms, RAW_FILE_MAX)
    }

    /// Comme [`RawLog::create`], avec un plafond choisi et sans purge.
    pub fn create_capped(
        dir: &Path,
        session: &str,
        epoch_ms: u64,
        max_bytes: u64,
    ) -> io::Result<RawLog> {
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
            max_bytes,
            file: Mutex::new(Some((file, 0))),
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
        let Some((f, written)) = guard.as_mut() else {
            return;
        };
        let mut buf = Vec::with_capacity(prefix.len() + line.len() + 1);
        buf.extend_from_slice(prefix.as_bytes());
        // Une ligne du flux ne contient pas de `\n` ; stderr non plus (découpé
        // par le lecteur). Par sécurité, on n'écrit qu'une ligne.
        buf.extend_from_slice(line.replace('\n', "\\n").as_bytes());
        buf.push(b'\n');
        if *written + buf.len() as u64 > self.max_bytes {
            // Plafond atteint : une ligne le signale (classée `STDERR` par le
            // rejeu des fixtures), puis plus rien pour cette session.
            let note = format!(
                "STDERR [orchestrator] copie du flux brut arrêtée : plafond de {} octets atteint\n",
                self.max_bytes
            );
            let _ = f.write_all(note.as_bytes());
            tracing::warn!(
                path = %self.path.display(),
                max = self.max_bytes,
                "copie du flux brut arrêtée : plafond atteint"
            );
            guard.take();
            return;
        }
        match f.write_all(&buf) {
            Ok(()) => *written += buf.len() as u64,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), "copie du flux brut arrêtée : {e}");
                guard.take();
            }
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
    fn raw_log_stops_at_its_cap_with_a_marker() {
        let d = tmp("cap");
        let log = RawLog::create_capped(&d, "s", 1, 30).unwrap();
        log.cli("0123456789"); // 11 octets
        log.cli("0123456789"); // 22
        log.cli("0123456789"); // 33 > 30 : refusée, copie arrêtée
        log.cli("après");
        log.exit(Some(0));
        let text = fs::read_to_string(log.path()).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[..2], ["0123456789", "0123456789"]);
        assert!(matches!(classify(lines[2]), FixtureLine::Stderr(m) if m.contains("plafond")));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn purge_trims_the_folder_oldest_first() {
        let d = tmp("total");
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            fs::write(d.join(format!("{name}.ndjson")), "x".repeat(100)).unwrap();
            // Dates de modification distinctes et croissantes.
            let f = File::options()
                .write(true)
                .open(d.join(format!("{name}.ndjson")))
                .unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(300 - 100 * i as u64))
                .unwrap();
        }
        assert_eq!(purge_raw(&d, Duration::from_secs(3600), 250), 1);
        assert!(!d.join("a.ndjson").exists(), "le plus ancien part");
        assert!(d.join("b.ndjson").exists() && d.join("c.ndjson").exists());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn raw_switch() {
        assert!(raw_enabled(None));
        assert!(raw_enabled(Some("1")));
        for off in ["0", "false", "OFF", " no "] {
            assert!(!raw_enabled(Some(off)), "{off}");
        }
    }

    #[test]
    fn purge_keeps_recent_files() {
        let d = tmp("purge");
        fs::write(d.join("a.ndjson"), "x").unwrap();
        fs::write(d.join("b.log"), "x").unwrap();
        assert_eq!(purge_raw(&d, Duration::from_secs(3600), u64::MAX), 0);
        assert_eq!(purge_raw(&d, Duration::ZERO, u64::MAX), 1);
        assert!(!d.join("a.ndjson").exists());
        assert!(d.join("b.log").exists(), "seuls les .ndjson sont visés");
        let _ = fs::remove_dir_all(d);
    }
}
