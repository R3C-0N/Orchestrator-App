//! Cycle de vie des processus : Job Object, tuyau réel avec claude.exe.
//!
//! - `job_close_kills_process_tree` : sans claude.exe, toujours exécuté.
//! - `live_*` : lancent le vrai claude.exe (réseau, coût de quelques centimes),
//!   ignorés par défaut ; `cargo test -- --ignored` pour les exécuter. Ils
//!   tournent dans `.claude-work/j0-sandbox/` avec `--model haiku` et
//!   `--no-session-persistence` (aucun transcript écrit).

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::events::{CoreEvent, EventBody};
use orchestrator_lib::api::types::SessionKey;
use orchestrator_lib::process::locate::locate_claude;
use orchestrator_lib::process::spawn::{spawn_claude, LaunchOptions};
use orchestrator_lib::process::supervisor::Session;
use orchestrator_lib::state::batcher::Sink;

/// Vrai si un processus de ce pid existe encore (tasklist).
fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

/// Pids des enfants directs (CIM).
fn children(pid: u32) -> Vec<u32> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}').ProcessId"),
        ])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn wait_dead(pid: u32, within: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < within {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    !alive(pid)
}

fn sandbox() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.claude-work/j0-sandbox");
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

#[tokio::test]
async fn job_close_kills_process_tree() {
    // cmd.exe lance ping.exe : on vérifie que fermer le Job tue les deux.
    let cmd = PathBuf::from(
        std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into()),
    );
    let spawned = spawn_claude(
        &cmd,
        &std::env::temp_dir(),
        &["/c".into(), "ping -n 60 127.0.0.1 >NUL".into()],
    )
    .expect("lancement");
    let pid = spawned.pid;
    let mut kids = Vec::new();
    for _ in 0..50 {
        kids = children(pid);
        if !kids.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(alive(pid), "cmd.exe vivant");
    assert!(!kids.is_empty(), "ping.exe lancé par cmd.exe");
    println!("cmd.exe pid={pid}, enfants={kids:?}");

    // Fermer le handle du Job (drop), comme le noyau le fait quand l'app meurt.
    let orchestrator_lib::process::spawn::Spawned { child, job, .. } = spawned;
    std::mem::forget(child); // pas de kill_on_drop : seul le Job doit tuer
    drop(job);

    assert!(
        wait_dead(pid, Duration::from_secs(5)),
        "cmd.exe tué par le Job"
    );
    for k in kids {
        assert!(
            wait_dead(k, Duration::from_secs(5)),
            "enfant {k} tué par le Job"
        );
    }
}

#[tokio::test]
#[ignore = "lance le vrai claude.exe (réseau, coût)"]
async fn live_pipe_end_to_end() {
    let exe = locate_claude().expect("claude.exe");
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let batches = Arc::new(Mutex::new(0usize));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let sink: Sink = {
        let events = events.clone();
        let batches = batches.clone();
        Arc::new(move |batch| {
            *batches.lock().unwrap() += 1;
            for ev in batch {
                if matches!(
                    ev.body,
                    EventBody::TurnResult { .. } | EventBody::Exited { .. }
                ) {
                    let _ = tx.send(());
                }
                events.lock().unwrap().push(ev);
            }
            true
        })
    };
    let t0 = Instant::now();
    let session = Session::start(
        SessionKey("live".into()),
        &exe,
        &sandbox(),
        LaunchOptions {
            resume: None,
            model: Some("haiku".into()),
            extra_args: vec!["--no-session-persistence".into()],
        },
        sink,
    )
    .await
    .expect("start + initialize");
    println!(
        "initialize : réponse en {} ms, pid {}",
        t0.elapsed().as_millis(),
        session.pid
    );

    session.send_message("Dis bonjour en un mot.").unwrap();
    tokio::time::timeout(Duration::from_secs(120), rx.recv())
        .await
        .expect("turn_result dans les 120 s");
    let info = session.stop().await;
    println!("arrêt : {info:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!alive(session.pid), "claude.exe arrêté");

    let evs = events.lock().unwrap();
    let kinds: Vec<String> = evs
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    println!(
        "{} événements en {} lots",
        evs.len(),
        batches.lock().unwrap()
    );
    let mut uniq = kinds.clone();
    uniq.dedup();
    println!("suite des kinds (dédoublonnés consécutifs) : {uniq:?}");
    // seq strictement croissant, sans trou.
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
    let text: String = evs
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::TextDelta { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    println!("texte reçu en streaming : {text:?}");
    for k in [
        "starting",
        "ready",
        "init",
        "user_message",
        "block_start",
        "text_delta",
        "block_done",
        "turn_result",
        "exited",
    ] {
        assert!(kinds.iter().any(|x| x == k), "événement {k} manquant");
    }
    assert!(!text.trim().is_empty());
    let res = evs.iter().find_map(|e| match &e.body {
        EventBody::TurnResult {
            is_error, subtype, ..
        } => Some((*is_error, subtype.clone())),
        _ => None,
    });
    assert_eq!(res, Some((false, "success".into())));
    assert!(
        info.code == Some(0) && !info.killed,
        "arrêt propre par fermeture de stdin"
    );
}

#[test]
#[ignore = "lance le vrai claude.exe (réseau)"]
fn live_killing_host_kills_claude() {
    // Le binaire `probe` joue l'app : il lance claude.exe dans son Job, puis on
    // le tue brutalement (TerminateProcess, comme le Gestionnaire des tâches).
    let mut probe = Command::new(env!("CARGO_BIN_EXE_probe"))
        .args([
            "--cwd",
            sandbox().to_str().unwrap(),
            "--model",
            "haiku",
            "--no-persist",
            "--hold",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("probe");
    use std::io::BufRead;
    let stdout = probe.stdout.take().unwrap();
    let mut claude_pid = None;
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.unwrap();
        if let Some(p) = line.strip_prefix("CLAUDE_PID ") {
            claude_pid = Some(p.trim().parse::<u32>().unwrap());
            break;
        }
    }
    let claude_pid = claude_pid.expect("pid de claude.exe");
    let kids = children(claude_pid);
    println!(
        "probe pid={}, claude.exe pid={claude_pid}, enfants de claude={kids:?}",
        probe.id()
    );
    assert!(alive(claude_pid));

    probe.kill().expect("TerminateProcess sur probe");
    probe.wait().unwrap();

    assert!(
        wait_dead(claude_pid, Duration::from_secs(5)),
        "claude.exe doit mourir avec l'hôte"
    );
    for k in kids {
        assert!(
            wait_dead(k, Duration::from_secs(5)),
            "enfant {k} de claude.exe doit mourir"
        );
    }
}
