//! Supervision avec le vrai `claude.exe` (J6). Ignoré par défaut :
//! `cargo test --test supervisor_live -- --ignored`. Ne lance que
//! `initialize` (aucun message, donc aucun tour facturé), en haiku, sans
//! transcript, dans `.claude-work/j6-sandbox/`.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use orchestrator_lib::api::commands::AppState;
use orchestrator_lib::api::types::SessionKey;
use orchestrator_lib::process::locate::locate_claude;
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::Session;
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
async fn live_reload_leaves_no_claude_process() {
    let exe = locate_claude().expect("claude.exe");
    let sink: Sink = Arc::new(|_| true);
    let s = Session::start(
        SessionKey("live-j6".into()),
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
    let pids = tree(s.pid);
    println!("arbre de claude.exe : {pids:?}, version {:?}", s.version());
    assert!(pids.contains(&s.pid));

    let state = AppState::new(sandbox());
    state
        .sessions
        .lock()
        .unwrap()
        .insert(s.key.clone(), s.clone());
    // Ce que fait `on_page_load` au rechargement de la webview.
    assert_eq!(state.close_orphans().await, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let left: Vec<u32> = pids.into_iter().filter(|p| alive(*p)).collect();
    println!("processus restants après rechargement : {left:?}");
    assert!(left.is_empty(), "aucun processus de l'arbre ne survit");
    assert!(state.sessions.lock().unwrap().is_empty());
}
