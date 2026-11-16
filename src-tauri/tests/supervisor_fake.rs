//! Superviseur face au faux `claude.exe` (`src/bin/fake_claude.rs`), plan 1.10
//! point 3 et jalon J6 : chaque scénario de panne produit les bons `exited` et
//! `notice`, sans blocage et sans processus laissé derrière.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::commands::AppState;
use orchestrator_lib::api::events::{CoreEvent, EventBody, NoticeLevel, SessionState};
use orchestrator_lib::api::types::{CoreError, SessionKey};
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::{Session, SessionConfig};
use orchestrator_lib::state::batcher::Sink;

fn fake_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

/// Vrai si un processus de ce pid existe encore (tasklist).
fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

/// Événements reçus par le `Sink`, avec attente d'une condition.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<CoreEvent>>>);

impl Events {
    fn sink(&self) -> Sink {
        let inner = self.0.clone();
        Arc::new(move |batch| {
            inner.lock().unwrap().extend(batch);
            true
        })
    }

    fn all(&self) -> Vec<CoreEvent> {
        self.0.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<EventBody> {
        self.all().into_iter().map(|e| e.body).collect()
    }

    async fn wait_for(&self, what: &str, within: Duration, f: impl Fn(&EventBody) -> bool) {
        let start = Instant::now();
        loop {
            if self.0.lock().unwrap().iter().any(|e| f(&e.body)) {
                return;
            }
            if start.elapsed() > within {
                panic!(
                    "{what} non reçu en {within:?} ; reçu : {:#?}",
                    kinds(&self.bodies())
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn notices(&self, code: &str) -> Vec<(NoticeLevel, String)> {
        self.bodies()
            .into_iter()
            .filter_map(|b| match b {
                EventBody::Notice {
                    level,
                    code: c,
                    message,
                    ..
                } if c == code => Some((level, message)),
                _ => None,
            })
            .collect()
    }

    fn exited(&self) -> Vec<(Option<i32>, bool, Vec<String>)> {
        self.bodies()
            .into_iter()
            .filter_map(|b| match b {
                EventBody::Exited {
                    code,
                    during_turn,
                    stderr_tail,
                } => Some((code, during_turn, stderr_tail)),
                _ => None,
            })
            .collect()
    }

    fn statuses(&self) -> Vec<SessionState> {
        self.bodies()
            .into_iter()
            .filter_map(|b| match b {
                EventBody::Status { state } => Some(state),
                _ => None,
            })
            .collect()
    }

    fn starting_pid(&self) -> u32 {
        self.bodies()
            .into_iter()
            .find_map(|b| match b {
                EventBody::Starting { pid, .. } => Some(pid),
                _ => None,
            })
            .expect("événement starting")
    }
}

fn kinds(bodies: &[EventBody]) -> Vec<String> {
    bodies
        .iter()
        .map(|b| {
            let v = serde_json::to_value(b).unwrap();
            match v["code"].as_str() {
                Some(c) => format!("{}:{c}", v["kind"].as_str().unwrap_or("?")),
                None => v["kind"].as_str().unwrap_or("?").to_string(),
            }
        })
        .collect()
}

fn is_turn_result(b: &EventBody) -> bool {
    matches!(b, EventBody::TurnResult { .. })
}

fn is_exited(b: &EventBody) -> bool {
    matches!(b, EventBody::Exited { .. })
}

fn fast() -> SessionConfig {
    SessionConfig {
        initialize_timeout: Duration::from_secs(10),
        stop_grace: Duration::from_secs(2),
        ..SessionConfig::default()
    }
}

async fn start(
    args: &[&str],
    config: SessionConfig,
) -> (Result<Arc<Session>, CoreError>, Events, Duration) {
    let ev = Events::default();
    let t0 = Instant::now();
    let res = Session::start_with(
        SessionKey("fake".into()),
        &fake_exe(),
        &std::env::temp_dir(),
        LaunchOptions {
            resume: None,
            model: None,
            extra_args: args.iter().map(|s| s.to_string()).collect(),
        },
        ev.sink(),
        config,
    )
    .await;
    (res, ev, t0.elapsed())
}

async fn start_ok(args: &[&str], config: SessionConfig) -> (Arc<Session>, Events) {
    let (res, ev, _) = start(args, config).await;
    (res.expect("démarrage du faux CLI"), ev)
}

#[tokio::test]
async fn normal_turn_then_clean_stop_gives_no_alert() {
    let (s, ev) = start_ok(&[], fast()).await;
    s.send_message("bonjour").unwrap();
    ev.wait_for("turn_result", Duration::from_secs(10), is_turn_result)
        .await;
    let info = s.stop().await;
    println!("arrêt : {info:?}");
    assert_eq!(info.code, Some(0));
    assert!(!info.killed);
    ev.wait_for("exited", Duration::from_secs(5), is_exited)
        .await;
    assert_eq!(ev.exited()[0].0, Some(0));
    assert!(!ev.exited()[0].1, "hors tour");
    assert!(
        ev.notices("exit_nonzero").is_empty(),
        "arrêt normal sans alerte"
    );
    assert_eq!(ev.statuses().last(), Some(&SessionState::Exited));
    assert!(!alive(s.pid));
}

#[tokio::test]
async fn immediate_death_fails_start_with_stderr() {
    let (res, ev, took) = start(
        &["--fake-mode", "die-immediately", "--fake-exit-code", "3"],
        fast(),
    )
    .await;
    let Err(CoreError::SpawnFailed(msg)) = res else {
        panic!("SpawnFailed attendu, reçu {:?}", res.err())
    };
    println!("mort immédiate : {msg:?} en {took:?}");
    assert!(msg.contains("erreur fatale au démarrage"), "{msg}");
    assert!(took < Duration::from_secs(5), "pas d'attente des 20 s");
    // Les événements passent par le regroupeur (16 ms) : attendre `exited`.
    ev.wait_for("exited", Duration::from_secs(2), is_exited)
        .await;
    let exited = ev.exited();
    assert_eq!(exited.len(), 1);
    assert_eq!(exited[0].0, Some(3));
    assert_eq!(ev.notices("exit_nonzero").len(), 1, "code non nul signalé");
}

#[tokio::test]
async fn no_answer_to_initialize_times_out_and_kills() {
    let config = SessionConfig {
        initialize_timeout: Duration::from_secs(1),
        ..fast()
    };
    let (res, ev, took) = start(&["--fake-mode", "no-init"], config).await;
    let Err(CoreError::SpawnFailed(msg)) = res else {
        panic!("SpawnFailed attendu, reçu {:?}", res.err())
    };
    println!("pas de réponse à initialize : {msg:?} en {took:?}");
    assert!(msg.contains("initialize"), "{msg}");
    // Délai d'1 s + arrêt de l'arbre ; marge pour une machine chargée.
    assert!(took < Duration::from_secs(6), "{took:?}");
    ev.wait_for("exited", Duration::from_secs(2), is_exited)
        .await;
    let pid = ev.starting_pid();
    assert!(!alive(pid), "faux CLI tué");
    assert!(
        ev.notices("exit_nonzero").is_empty(),
        "arrêt voulu par l'app : pas d'alerte en plus de l'erreur"
    );
}

#[tokio::test]
async fn death_mid_turn_reports_code_and_tail() {
    let (s, ev) = start_ok(&["--fake-mode", "die-mid-turn"], fast()).await;
    s.send_message("vas-y").unwrap();
    ev.wait_for("exited", Duration::from_secs(5), is_exited)
        .await;
    let (code, during_turn, tail) = ev.exited().remove(0);
    println!("mort en plein tour : code={code:?} during_turn={during_turn} tail={tail:?}");
    assert_eq!(code, Some(1));
    assert!(during_turn);
    assert!(tail.iter().any(|l| l.contains("plantage en plein tour")));
    let n = ev.notices("exit_nonzero");
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].0, NoticeLevel::Error);
    assert_eq!(ev.statuses().last(), Some(&SessionState::Exited));
    assert_eq!(s.send_message("encore"), Err(CoreError::NotRunning));
}

#[tokio::test]
async fn grandchild_holding_stdout_does_not_delay_exited() {
    let (s, ev) = start_ok(&["--fake-mode", "orphan-stdout"], fast()).await;
    ev.wait_for(
        "pid du petit-enfant",
        Duration::from_secs(5),
        |b| matches!(b, EventBody::Stderr { line } if line.starts_with("GRANDCHILD_PID")),
    )
    .await;
    let grandchild: u32 = ev
        .bodies()
        .into_iter()
        .find_map(|b| match b {
            EventBody::Stderr { line } => line
                .strip_prefix("GRANDCHILD_PID ")
                .map(|p| p.trim().parse().unwrap()),
            _ => None,
        })
        .unwrap();
    assert!(alive(grandchild), "petit-enfant vivant avant la mort");
    let t0 = Instant::now();
    s.send_message("vas-y").unwrap();
    ev.wait_for("exited", Duration::from_secs(5), is_exited)
        .await;
    let took = t0.elapsed();
    println!("stdout tenu par le petit-enfant {grandchild} : exited après {took:?}");
    assert!(
        took < Duration::from_secs(3),
        "exited ne dépend pas de la fin de stdout"
    );
    assert_eq!(ev.exited()[0].0, Some(1));
    assert!(ev.exited()[0].1, "pendant un tour");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!alive(grandchild), "petit-enfant tué par le Job Object");
    assert!(!alive(s.pid));
}

#[tokio::test]
async fn abundant_stderr_does_not_block_and_keeps_200_lines() {
    let (s, ev) = start_ok(&["--fake-stderr-lines", "5000"], fast()).await;
    s.send_message("bonjour").unwrap();
    ev.wait_for("turn_result", Duration::from_secs(20), is_turn_result)
        .await;
    let start = Instant::now();
    while s.stderr_tail().len() < 200 && start.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let stop = s.stop().await;
    let relayed = ev
        .bodies()
        .iter()
        .filter(|b| matches!(b, EventBody::Stderr { .. }))
        .count();
    let tail = s.stderr_tail();
    println!(
        "stderr abondant : {relayed} lignes relayées, {} gardées, dernière {:?}, arrêt {stop:?}",
        tail.len(),
        tail.last()
    );
    assert_eq!(relayed, 5000);
    assert_eq!(tail.len(), 200);
    assert!(tail.last().unwrap().contains("ligne de stderr 4999"));
    assert_eq!(stop.code, Some(0));
}

#[tokio::test]
async fn huge_line_and_bad_json_give_distinct_notices() {
    let config = SessionConfig {
        max_line_bytes: 1024 * 1024,
        ..fast()
    };
    let (s, ev) = start_ok(&["--fake-huge-line", "3000000", "--fake-bad-json"], config).await;
    ev.wait_for(
        "line_too_long",
        Duration::from_secs(10),
        |b| matches!(b, EventBody::Notice { code, .. } if code == "line_too_long"),
    )
    .await;
    s.send_message("bonjour").unwrap();
    ev.wait_for("turn_result", Duration::from_secs(10), is_turn_result)
        .await;
    let too_long = ev.notices("line_too_long");
    let bad = ev.notices("bad_json");
    println!("ligne géante : {too_long:?} ; JSON illisible : {bad:?}");
    assert_eq!(too_long.len(), 1);
    assert_eq!(too_long[0].0, NoticeLevel::Warn);
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].0, NoticeLevel::Warn);
    assert!(s.is_running(), "la session continue");
    s.stop().await;
}

#[tokio::test]
async fn utf8_cut_between_writes_and_crlf_are_decoded() {
    let (s, ev) = start_ok(&["--fake-split"], fast()).await;
    s.send_message("bonjour").unwrap();
    ev.wait_for("turn_result", Duration::from_secs(10), is_turn_result)
        .await;
    let deltas: String = ev
        .bodies()
        .into_iter()
        .filter_map(|b| match b {
            EventBody::TextDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    println!("UTF-8 coupé + CRLF : texte reçu {deltas:?}");
    assert_eq!(deltas, "é€😀");
    assert!(ev.notices("bad_json").is_empty());
    s.stop().await;
}

#[tokio::test]
async fn unexpected_version_and_unknown_types_do_not_block() {
    let (s, ev) = start_ok(&["--fake-version", "9.9.9", "--fake-unknown"], fast()).await;
    for want in 1..=2 {
        s.send_message("bonjour").unwrap();
        let start = Instant::now();
        while ev.bodies().iter().filter(|b| is_turn_result(b)).count() < want {
            assert!(start.elapsed() < Duration::from_secs(10), "tour {want}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let version = ev.notices("unexpected_version");
    println!(
        "version 9.9.9 : notices {version:?}, unknown_count={}, version vue {:?}",
        s.unknown_count(),
        s.version()
    );
    assert_eq!(version.len(), 1, "un seul bandeau « version non testée »");
    assert_eq!(s.version().as_deref(), Some("9.9.9"));
    assert!(s.unknown_count() > 0);
    assert!(ev.notices("bad_json").is_empty());
    assert!(s.is_running());
    assert_eq!(s.stop().await.code, Some(0));
}

#[tokio::test]
async fn accepted_interrupt_without_result_does_not_stay_interrupting() {
    let config = SessionConfig {
        interrupt_result_timeout: Duration::from_millis(500),
        ..fast()
    };
    let (s, ev) = start_ok(&["--fake-interrupt-no-result"], config).await;
    s.send_message("long").unwrap();
    ev.wait_for("streaming", Duration::from_secs(5), |b| {
        matches!(
            b,
            EventBody::Status {
                state: SessionState::Streaming
            }
        )
    })
    .await;
    s.interrupt(false).await.expect("interrupt accepté");
    assert_eq!(s.state(), Some(SessionState::Interrupting));
    ev.wait_for(
        "interrupt_no_result",
        Duration::from_secs(3),
        |b| matches!(b, EventBody::Notice { code, .. } if code == "interrupt_no_result"),
    )
    .await;
    println!(
        "interrupt sans result : états {:?}, état final {:?}",
        ev.statuses(),
        s.state()
    );
    assert_eq!(s.state(), Some(SessionState::Streaming));
    // On peut réinterrompre.
    s.interrupt(false).await.expect("deuxième interrupt");
    assert_eq!(s.state(), Some(SessionState::Interrupting));
    s.kill();
}

#[tokio::test]
async fn broken_stdin_pipe_leaves_a_notice() {
    let (s, ev) = start_ok(&["--fake-mode", "close-stdin"], fast()).await;
    // Le faux CLI ferme son stdin juste après `initialize`.
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.send_message("perdu").unwrap();
    ev.wait_for(
        "stdin_broken",
        Duration::from_secs(5),
        |b| matches!(b, EventBody::Notice { code, .. } if code == "stdin_broken"),
    )
    .await;
    println!("stdin cassé : {:?}", ev.notices("stdin_broken"));
    assert_eq!(s.send_message("encore"), Err(CoreError::NotRunning));
    let info = s.stop().await;
    assert!(info.killed, "le faux CLI dort : tué par le Job Object");
    assert!(!alive(s.pid));
}

#[tokio::test]
async fn stop_kills_when_stdin_eof_is_ignored() {
    let (s, ev) = start_ok(&["--fake-ignore-stdin-eof"], fast()).await;
    let t0 = Instant::now();
    let info = s.stop().await;
    println!("arrêt forcé : {info:?} en {:?}", t0.elapsed());
    assert!(info.killed);
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert!(!alive(s.pid));
    ev.wait_for("exited", Duration::from_secs(2), is_exited)
        .await;
    assert!(
        ev.notices("exit_nonzero").is_empty(),
        "arrêt demandé : pas d'alerte"
    );
}

#[tokio::test]
async fn orphan_sessions_are_closed_and_removed() {
    let state = AppState::new(std::env::temp_dir());
    let (a, _) = start_ok(&[], fast()).await;
    let (b, _) = start_ok(&["--fake-ignore-stdin-eof"], fast()).await;
    state
        .sessions
        .lock()
        .unwrap()
        .insert(a.key.clone(), a.clone());
    state
        .sessions
        .lock()
        .unwrap()
        .insert(SessionKey("b".into()), b.clone());
    // Rechargement de la webview.
    let closed = state.close_orphans().await;
    println!(
        "rechargement : {closed} sessions fermées, vivantes : a={} b={}",
        alive(a.pid),
        alive(b.pid)
    );
    assert_eq!(closed, 2);
    assert!(state.sessions.lock().unwrap().is_empty());
    assert!(
        state.last.lock().unwrap().is_some(),
        "gardée pour les diagnostics"
    );
    assert!(!alive(a.pid) && !alive(b.pid), "aucun processus laissé");
    assert_eq!(state.close_orphans().await, 0);
}

#[tokio::test]
async fn close_session_removes_one_session() {
    let state = AppState::new(std::env::temp_dir());
    let (a, _) = start_ok(&[], fast()).await;
    let key = SessionKey("seule".into());
    state
        .sessions
        .lock()
        .unwrap()
        .insert(key.clone(), a.clone());
    let info = state.close_session(&key).await.expect("fermée");
    assert_eq!(info.code, Some(0));
    assert!(state.close_session(&key).await.is_none(), "déjà retirée");
    assert!(!alive(a.pid));
}
