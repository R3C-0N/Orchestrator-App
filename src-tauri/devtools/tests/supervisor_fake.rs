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
use orchestrator_lib::api::types::{CoreError, SessionKey, SessionRole};
use orchestrator_lib::process::spawn::LaunchOptions;
use orchestrator_lib::process::supervisor::{Session, SessionConfig, TurnWait};
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
    // Tout stderr est relayé avant `exited` : l'attendre avant de compter.
    ev.wait_for("exited", Duration::from_secs(5), is_exited)
        .await;
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

/// Relecture J6, B1 : un second Échap pendant `Interrupting` ne doit pas
/// désarmer la garde de la première interruption.
#[tokio::test]
async fn two_close_interrupts_without_result_do_not_stay_interrupting() {
    let config = SessionConfig {
        interrupt_result_timeout: Duration::from_millis(800),
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
    let t0 = Instant::now();
    s.interrupt(false).await.expect("premier interrupt accepté");
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.interrupt(false).await.expect("second interrupt accepté");
    assert_eq!(s.state(), Some(SessionState::Interrupting));
    ev.wait_for(
        "interrupt_no_result",
        Duration::from_secs(3),
        |b| matches!(b, EventBody::Notice { code, .. } if code == "interrupt_no_result"),
    )
    .await;
    println!(
        "double interrupt sans result : états {:?}, état final {:?} après {:?}",
        ev.statuses(),
        s.state(),
        t0.elapsed()
    );
    assert_eq!(s.state(), Some(SessionState::Streaming));
    assert_eq!(ev.notices("interrupt_no_result").len(), 1);
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

fn opts(args: &[&str]) -> LaunchOptions {
    LaunchOptions {
        resume: None,
        model: None,
        extra_args: args.iter().map(|s| s.to_string()).collect(),
    }
}

fn seqs(events: &[CoreEvent]) -> Vec<u64> {
    events.iter().map(|e| e.seq).collect()
}

fn turn_results(events: &[CoreEvent]) -> usize {
    events.iter().filter(|e| is_turn_result(&e.body)).count()
}

/// J14 (décision 15) : un rechargement de la webview détache les sessions
/// au lieu de les fermer (J6 remplacé). Le processus survit (même pid), ses
/// événements vont au journal pendant le détachement, et la page suivante
/// reçoit le journal complet puis la suite, sans trou ni doublon de `seq`.
#[tokio::test]
async fn reload_detaches_then_attach_resumes_the_same_process() {
    let state = AppState::new(std::env::temp_dir());
    let page1 = Events::default();
    let s = state
        .spawn_session(
            SessionRole::Main,
            &fake_exe(),
            &std::env::temp_dir(),
            opts(&[]),
            Some(state.attachment(page1.sink())),
            fast(),
        )
        .await
        .unwrap();
    s.send_message("un").unwrap();
    page1
        .wait_for("turn_result", Duration::from_secs(10), is_turn_result)
        .await;
    assert!(state.main_idle(), "tour fini : au repos");

    // Rechargement de la webview (`on_page_load`).
    assert_eq!(state.close_orphans().await, 1);
    assert!(!s.outlet().is_attached());
    assert!(alive(s.pid), "le processus survit au rechargement");
    let seen_by_page1 = page1.all().len();

    // Un tour pendant le détachement : journal seul.
    let before = s.turns();
    s.send_message("deux").unwrap();
    assert_eq!(
        s.wait_turn_end(before, Duration::from_secs(10)).await,
        TurnWait::Done(2)
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(page1.all().len(), seen_by_page1, "ancienne page muette");

    // Nouvelle page : rattachement.
    let page2 = Events::default();
    let t0 = Instant::now();
    let resync = state.attach_all(state.attachment(page2.sink()));
    println!(
        "rattachement : {} session(s), {} événements en {:?}",
        resync.len(),
        resync[0].events.len(),
        t0.elapsed()
    );
    assert_eq!(resync.len(), 1);
    let r = &resync[0];
    assert_eq!(
        (r.session.clone(), r.pid, r.role),
        (s.key.clone(), s.pid, SessionRole::Main)
    );
    assert!(r.running);
    assert_eq!(
        seqs(&r.events),
        (0..r.events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(
        turn_results(&r.events),
        2,
        "les deux tours sont dans le journal"
    );
    assert!(page1.all().iter().all(|e| e.seq < r.events.len() as u64));

    // La suite arrive sur le nouveau canal, à partir de la fin du journal.
    s.send_message("trois").unwrap();
    page2
        .wait_for("turn_result", Duration::from_secs(10), is_turn_result)
        .await;
    let mut all = seqs(&r.events);
    all.extend(seqs(&page2.all()));
    assert_eq!(
        all,
        (0..all.len() as u64).collect::<Vec<_>>(),
        "aucun trou ni doublon"
    );
    assert_eq!(state.sessions.lock().unwrap().len(), 1);
    s.stop().await;
    assert!(!alive(s.pid));
}

/// Pas de rattachement dans le délai : la session est fermée et retirée
/// (esprit de J6 : pas d'orphelin), même un CLI qui ignore la fin de stdin.
#[tokio::test]
async fn detached_sessions_are_closed_after_the_grace_delay() {
    let mut state = AppState::new(std::env::temp_dir());
    state.detach_grace = Duration::from_millis(500);
    let mut pids = Vec::new();
    for args in [&[][..], &["--fake-ignore-stdin-eof"][..]] {
        let ev = Events::default();
        let s = state
            .spawn_session(
                SessionRole::Main,
                &fake_exe(),
                &std::env::temp_dir(),
                opts(args),
                Some(state.attachment(ev.sink())),
                fast(),
            )
            .await
            .unwrap();
        pids.push(s.pid);
    }
    assert_eq!(state.close_orphans().await, 2);
    assert!(pids.iter().all(|p| alive(*p)), "détachées, pas fermées");
    let t0 = Instant::now();
    while !state.sessions.lock().unwrap().is_empty() {
        assert!(
            t0.elapsed() < Duration::from_secs(8),
            "pas fermées après le délai"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // L'arrêt (fermeture de stdin, puis Job Object pour la seconde) se finit.
    let t0 = Instant::now();
    while pids.iter().any(|p| alive(*p)) {
        assert!(t0.elapsed() < Duration::from_secs(8), "processus laissé");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("fermées et arrêtées {:?} après le délai", t0.elapsed());
    assert!(
        state.last.lock().unwrap().is_some(),
        "gardée pour les diagnostics"
    );
    assert_eq!(state.close_orphans().await, 0);
}

/// Rattachée avant le délai : la session n'est pas fermée.
#[tokio::test]
async fn attach_within_the_delay_keeps_the_session() {
    let mut state = AppState::new(std::env::temp_dir());
    state.detach_grace = Duration::from_millis(400);
    let ev = Events::default();
    let s = state
        .spawn_session(
            SessionRole::Main,
            &fake_exe(),
            &std::env::temp_dir(),
            opts(&[]),
            Some(state.attachment(ev.sink())),
            fast(),
        )
        .await
        .unwrap();
    state.close_orphans().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let page2 = Events::default();
    assert_eq!(state.attach_all(state.attachment(page2.sink())).len(), 1);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(state.sessions.lock().unwrap().len(), 1);
    assert!(alive(s.pid) && s.is_running());
    s.stop().await;
}

/// Étape 5 : session `Improve` sans canal propre ; ses événements vont au
/// canal rattaché ; attente de la fin de son tour ; repos de la session
/// principale (tour ouvert : pas au repos).
#[tokio::test]
async fn improve_session_reports_to_the_attached_page() {
    let state = AppState::new(std::env::temp_dir());
    let page = Events::default();
    assert!(state.attach_all(state.attachment(page.sink())).is_empty());
    assert!(state.main_idle(), "aucune session principale");

    let main_ev = Events::default();
    let main = state
        .spawn_session(
            SessionRole::Main,
            &fake_exe(),
            &std::env::temp_dir(),
            opts(&["--fake-interrupt-no-result"]),
            Some(state.attachment(main_ev.sink())),
            fast(),
        )
        .await
        .unwrap();
    assert!(state.main_idle());
    main.send_message("long").unwrap();
    assert!(!state.main_idle(), "tour en cours : pas au repos");
    assert!(!main.wait_idle(Duration::from_millis(300)).await);

    let imp = state
        .start_improve_with(
            &fake_exe(),
            &std::env::temp_dir(),
            "améliore l'interface",
            opts(&[]),
            fast(),
        )
        .await
        .unwrap();
    assert_eq!(imp.role, SessionRole::Improve);
    assert_eq!(
        imp.wait_turn_end(0, Duration::from_secs(10)).await,
        TurnWait::Done(1)
    );
    page.wait_for(
        "turn_result de l'amélioration",
        Duration::from_secs(5),
        is_turn_result,
    )
    .await;
    let on_page: Vec<CoreEvent> = page
        .all()
        .into_iter()
        .filter(|e| e.session == imp.key)
        .collect();
    assert_eq!(
        seqs(&on_page),
        (0..on_page.len() as u64).collect::<Vec<_>>()
    );
    assert!(
        page.all().iter().all(|e| e.session == imp.key),
        "pas les événements de main"
    );

    // Rechargement puis rattachement : les deux sessions, dans l'ordre de
    // démarrage.
    state.close_orphans().await;
    let page2 = Events::default();
    let resync = state.attach_all(state.attachment(page2.sink()));
    let roles: Vec<SessionRole> = resync.iter().map(|r| r.role).collect();
    assert_eq!(roles, vec![SessionRole::Main, SessionRole::Improve]);
    assert_eq!(
        state.close_session(&imp.key).await.map(|i| i.code),
        Some(Some(0))
    );
    assert_eq!(
        imp.wait_turn_end(1, Duration::from_secs(1)).await,
        TurnWait::Exited
    );
    main.stop().await;
    assert!(!alive(main.pid) && !alive(imp.pid));
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

async fn wait_gone(state: &AppState, pids: &[u32], within: Duration) -> Duration {
    let t0 = Instant::now();
    while !state.sessions.lock().unwrap().is_empty() || pids.iter().any(|p| alive(*p)) {
        assert!(
            t0.elapsed() < within,
            "sessions encore là : {} dans la table, vivantes {:?}",
            state.sessions.lock().unwrap().len(),
            pids.iter().filter(|p| alive(**p)).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    t0.elapsed()
}

/// Relecture de 9d91aea, bloquant 1 : un canal qui refuse les lots sans
/// changement de génération (fenêtre fermée sans rechargement), partagé par
/// plusieurs sessions : toutes sont fermées après le délai.
#[tokio::test]
async fn refused_channel_closes_all_its_sessions_after_the_delay() {
    let mut state = AppState::new(std::env::temp_dir());
    state.detach_grace = Duration::from_millis(500);
    let refused: Sink = Arc::new(|_| false);
    let mut pids = Vec::new();
    for _ in 0..3 {
        let s = state
            .spawn_session(
                SessionRole::Main,
                &fake_exe(),
                &std::env::temp_dir(),
                opts(&[]),
                Some(state.attachment(refused.clone())),
                fast(),
            )
            .await
            .unwrap();
        pids.push(s.pid);
    }
    assert_eq!(state.page_gen(), 0, "aucun rechargement");
    let took = wait_gone(&state, &pids, Duration::from_secs(10)).await;
    println!("canal refusé : 3 sessions fermées en {took:?}");
}

/// Relecture de 9d91aea, bloquant 2 : une session `Improve` lancée sans page
/// rattachée est fermée après le délai si aucune page ne s'y rattache.
#[tokio::test]
async fn never_attached_improve_is_closed_after_the_delay() {
    let mut state = AppState::new(std::env::temp_dir());
    state.detach_grace = Duration::from_millis(500);
    let imp = state
        .start_improve_with(
            &fake_exe(),
            &std::env::temp_dir(),
            "améliore",
            opts(&["--fake-ignore-stdin-eof"]),
            fast(),
        )
        .await
        .unwrap();
    assert!(!imp.outlet().is_attached());
    assert!(
        imp.outlet().detached_for().is_some(),
        "compte comme détachée"
    );
    let took = wait_gone(&state, &[imp.pid], Duration::from_secs(10)).await;
    println!("amélioration sans page : fermée en {took:?}");
    assert_eq!(imp.turns(), 1, "son tour a eu lieu (journal)");
}

/// Une session `Improve` sort de la table à la fin de son processus, même
/// sans `close_session`. Une session morte est rendue une fois par
/// `attach_all`, puis retirée.
#[tokio::test]
async fn dead_sessions_leave_the_table() {
    let state = AppState::new(std::env::temp_dir());
    let page = Events::default();
    state.attach_all(state.attachment(page.sink()));
    let imp = state
        .start_improve_with(
            &fake_exe(),
            &std::env::temp_dir(),
            "améliore",
            opts(&["--fake-mode", "die-mid-turn"]),
            fast(),
        )
        .await
        .unwrap();
    wait_gone(&state, &[imp.pid], Duration::from_secs(10)).await;
    assert_eq!(
        imp.wait_turn_end(0, Duration::from_secs(1)).await,
        TurnWait::Exited
    );

    let ev = Events::default();
    let main = state
        .spawn_session(
            SessionRole::Main,
            &fake_exe(),
            &std::env::temp_dir(),
            opts(&["--fake-mode", "die-mid-turn"]),
            Some(state.attachment(ev.sink())),
            fast(),
        )
        .await
        .unwrap();
    main.send_message("x").unwrap();
    assert!(main.wait_exit(Duration::from_secs(10)).await.is_some());
    state.close_orphans().await;
    let first = state.attach_all(state.attachment(Events::default().sink()));
    assert_eq!(first.len(), 1);
    assert!(!first[0].running);
    assert!(first[0].events.iter().any(|e| is_exited(&e.body)));
    state.close_orphans().await;
    let second = state.attach_all(state.attachment(Events::default().sink()));
    assert!(
        second.is_empty(),
        "journal d'une session morte rendu une seule fois"
    );
}
