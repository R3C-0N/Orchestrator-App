//! Supervision avec le vrai `claude.exe` (J6, puis J14 : le rechargement
//! détache la session au lieu de la fermer). Ignoré par défaut :
//! `cargo test --test supervisor_live -- --ignored`. Ne lance que
//! `initialize` (aucun message, donc aucun tour facturé), en haiku, sans
//! transcript, dans `.claude-work/j6-sandbox/`.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use orchestrator_lib::api::commands::AppState;
use orchestrator_lib::api::types::SessionRole;
use orchestrator_lib::process::locate::locate_claude;
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::SessionConfig;
use orchestrator_lib::state::batcher::Sink;

fn sandbox() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.claude-work/j6-sandbox");
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

/// Pids de l'arbre (le processus et ses descendants), par CIM.
fn tree(pid: u32) -> Vec<u32> {
    let script = format!(
        "$all = Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId; \
         $todo = @({pid}); $seen = @(); \
         while ($todo.Count -gt 0) {{ $p = $todo[0]; $todo = @($todo | Select-Object -Skip 1); \
           if ($seen -contains $p) {{ continue }}; $seen += $p; \
           $todo += @($all | Where-Object {{ $_.ParentProcessId -eq $p }} | ForEach-Object {{ $_.ProcessId }}) }}; \
         $seen -join ' '"
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[tokio::test]
#[ignore = "lance le vrai claude.exe (initialize seulement)"]
async fn live_reload_keeps_the_process_then_closes_it_without_page() {
    let exe = locate_claude().expect("claude.exe");
    let mut state = AppState::new(sandbox());
    state.detach_grace = Duration::from_secs(2);
    let sink: Sink = Arc::new(|_| true);
    let s = state
        .spawn_session(
            SessionRole::Main,
            &exe,
            &sandbox(),
            LaunchOptions {
                resume: None,
                model: Some("haiku".into()),
                extra_args: vec!["--no-session-persistence".into()],
            },
            Some(state.attachment(sink)),
            SessionConfig::default(),
        )
        .await
        .expect("start + initialize");
    let pids = tree(s.pid);
    println!("arbre de claude.exe : {pids:?}, version {:?}", s.version());
    assert!(pids.contains(&s.pid));

    // Ce que fait `on_page_load` au rechargement de la webview (J14) : la
    // session est détachée, pas fermée.
    assert_eq!(state.close_orphans().await, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(alive(s.pid), "claude.exe survit au rechargement");

    // La nouvelle page se rattache : même session, même pid, journal sans trou.
    let resync = state.attach_all(state.attachment(Arc::new(|_| true)));
    assert_eq!(resync.len(), 1);
    assert_eq!(resync[0].pid, s.pid);
    assert!(resync[0].running);
    let seqs: Vec<u64> = resync[0].events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
    println!("rattachée : {} événements, pid {}", seqs.len(), s.pid);

    // Rechargement sans page qui se rattache : fermée après le délai, sans
    // processus laissé (esprit de J6).
    assert_eq!(state.close_orphans().await, 1);
    tokio::time::sleep(state.detach_grace + Duration::from_secs(4)).await;
    let left: Vec<u32> = pids.into_iter().filter(|p| alive(*p)).collect();
    println!("processus restants après le délai : {left:?}");
    assert!(left.is_empty(), "aucun processus de l'arbre ne survit");
    assert!(state.sessions.lock().unwrap().is_empty());
}
