//! Sessions rattachables (J14, étape 1, décision 15) : journal des
//! `CoreEvent` d'une session et destination remplaçable (`Outlet`).
//!
//! Critères du plan : rattachement de 10 000 événements en moins de 1 s,
//! aucun trou ni doublon de `seq` quand le rattachement a lieu pendant que la
//! session émet ; mesure du journal sur la fixture longue `t15`.
//! Le même pid avant et après un rechargement est vérifié avec le faux CLI
//! (`devtools/tests/supervisor_fake.rs`) et le vrai (`tests/supervisor_live.rs`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::events::{CoreEvent, EventBody};
use orchestrator_lib::api::types::{SessionKey, SessionResync, SessionRole};
use orchestrator_lib::process::supervisor::{Attachment, Outlet};
use orchestrator_lib::state::batcher::Sink;
use orchestrator_lib::state::replay::replay_fixture;

type Got = Arc<Mutex<Vec<u64>>>;

fn recorder() -> (Sink, Got) {
    let got: Got = Arc::default();
    let g = got.clone();
    let sink: Sink = Arc::new(move |batch: Vec<CoreEvent>| {
        g.lock().unwrap().extend(batch.iter().map(|e| e.seq));
        true
    });
    (sink, got)
}

fn line(i: u64) -> EventBody {
    EventBody::Stderr {
        line: format!("ligne {i} : un texte de longueur ordinaire pour la mesure"),
    }
}

async fn wait_until(what: &str, within: Duration, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < within, "{what} : pas atteint en {within:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn assert_contiguous(seqs: &[u64], from: u64, what: &str) {
    for (i, s) in seqs.iter().enumerate() {
        assert_eq!(
            *s,
            from + i as u64,
            "{what} : trou ou doublon à l'indice {i}"
        );
    }
}

/// Critère du plan : rattacher une session de 10 000 événements (instantané
/// du journal, puis sérialisation JSON de la réponse comme le fait l'IPC)
/// prend moins de 1 s.
#[tokio::test(flavor = "multi_thread")]
async fn attach_10000_events_under_one_second() {
    let (a, got_a) = recorder();
    let outlet = Outlet::spawn(
        SessionKey("s".into()),
        Some(Attachment {
            sink: a,
            page_gen: 0,
        }),
    );
    for i in 0..10_000 {
        outlet.emit(line(i), i);
    }
    wait_until("10 000 livrés", Duration::from_secs(5), || {
        got_a.lock().unwrap().len() == 10_000
    })
    .await;
    assert!(outlet.detach_before(1), "rechargement : détachée");
    assert!(outlet.detached_for().is_some());

    let (b, got_b) = recorder();
    let t0 = Instant::now();
    let events = outlet.attach(b, 1);
    let snap = t0.elapsed();
    let resync = SessionResync {
        session: SessionKey("s".into()),
        role: SessionRole::Main,
        cwd: "C:\\x".into(),
        pid: 1,
        resumed_from: None,
        history: None,
        events,
        running: true,
    };
    let json = serde_json::to_vec(&vec![resync.clone()]).unwrap();
    let total = t0.elapsed();
    println!(
        "rattachement de 10 000 événements : instantané {snap:?}, avec JSON ({} Ko) {total:?}",
        json.len() / 1024
    );
    assert!(total < Duration::from_secs(1), "{total:?}");
    let seqs: Vec<u64> = resync.events.iter().map(|e| e.seq).collect();
    assert_contiguous(&seqs, 0, "instantané");
    assert_eq!(seqs.len(), 10_000);

    // La suite arrive sur le nouveau canal, à partir de 10 000.
    for i in 10_000..10_010 {
        outlet.emit(line(i), i);
    }
    wait_until("suite livrée", Duration::from_secs(5), || {
        got_b.lock().unwrap().len() == 10
    })
    .await;
    assert_contiguous(&got_b.lock().unwrap(), 10_000, "nouveau canal");
    assert_eq!(got_a.lock().unwrap().len(), 10_000, "ancien canal muet");
}

/// Rattachements répétés pendant que deux fils émettent : pour chaque
/// destination, instantané + reçus forment une suite continue de `seq`
/// (aucun trou, aucun doublon) ; la dernière reçoit tout jusqu'au bout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reattach_while_emitting_has_no_gap_nor_duplicate() {
    const PER_THREAD: u64 = 20_000;
    let (first, _) = recorder();
    let outlet = Outlet::spawn(
        SessionKey("s".into()),
        Some(Attachment {
            sink: first,
            page_gen: 0,
        }),
    );
    let producers: Vec<_> = (0..2)
        .map(|k| {
            let o = outlet.clone();
            std::thread::spawn(move || {
                for i in 0..PER_THREAD {
                    o.emit(line(k * PER_THREAD + i), i);
                    if i % 500 == 0 {
                        std::thread::sleep(Duration::from_micros(200));
                    }
                }
            })
        })
        .collect();

    let mut pages = Vec::new();
    for gen in 1..=20u64 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        outlet.detach_before(gen);
        let (sink, got) = recorder();
        let snap = outlet.attach(sink, gen);
        pages.push((snap, got));
    }
    for p in producers {
        p.join().unwrap();
    }
    let total = 2 * PER_THREAD;
    let (last_snap, last_got) = pages.last().unwrap();
    let last_from = last_snap.len() as u64;
    wait_until("tout livré", Duration::from_secs(10), || {
        last_got.lock().unwrap().len() as u64 == total - last_from
    })
    .await;

    for (n, (snap, got)) in pages.iter().enumerate() {
        let seqs: Vec<u64> = snap.iter().map(|e| e.seq).collect();
        assert_contiguous(&seqs, 0, &format!("instantané {n}"));
        let got = got.lock().unwrap();
        assert_contiguous(&got, snap.len() as u64, &format!("page {n}"));
    }
    let mut all: Vec<u64> = last_snap.iter().map(|e| e.seq).collect();
    all.extend(last_got.lock().unwrap().iter());
    assert_contiguous(&all, 0, "dernière page");
    assert_eq!(all.len() as u64, total);
    let sizes: Vec<usize> = pages.iter().map(|(s, _)| s.len()).collect();
    println!("{total} événements, 20 rattachements, instantanés : {sizes:?}");
}

/// Une destination ancienne génération n'est plus servie ; une destination
/// qui refuse un lot détache la session (le journal continue).
#[tokio::test(flavor = "multi_thread")]
async fn detached_session_writes_only_to_its_journal() {
    let (a, got_a) = recorder();
    let outlet = Outlet::spawn(
        SessionKey("s".into()),
        Some(Attachment {
            sink: a,
            page_gen: 3,
        }),
    );
    outlet.emit(line(0), 0);
    wait_until("livré", Duration::from_secs(2), || {
        got_a.lock().unwrap().len() == 1
    })
    .await;
    assert!(!outlet.detach_before(3), "même génération : gardée");
    assert!(outlet.detach_before(4));
    assert!(!outlet.is_attached());
    outlet.emit(line(1), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(got_a.lock().unwrap().len(), 1, "rien après le détachement");
    assert_eq!(outlet.len(), 2, "journal complet");

    // Destination fermée (canal refusé) : détachement par l'envoi.
    let refused: Sink = Arc::new(|_| false);
    assert_eq!(outlet.attach(refused, 4).len(), 2);
    assert!(outlet.is_attached());
    outlet.emit(line(2), 2);
    wait_until("détachée par l'échec", Duration::from_secs(2), || {
        !outlet.is_attached()
    })
    .await;
    assert!(outlet.detached_for().is_some());
    outlet.emit(line(3), 3);
    let (c, got_c) = recorder();
    let snap = outlet.attach(c, 5);
    assert_eq!(snap.len(), 4, "journal sans trou malgré l'échec");
    outlet.emit(line(4), 4);
    wait_until("livré", Duration::from_secs(2), || {
        got_c.lock().unwrap().len() == 1
    })
    .await;
    assert_eq!(*got_c.lock().unwrap(), vec![4]);
}

/// J16 : la conversation d'un cycle d'amélioration est rendue par
/// `attach_all` sans aucun processus (avant le premier tour, entre deux
/// tours), une seule fois par rattachement, avec ses lignes d'état
/// `improve.*` ; un rechargement la détache sans la fermer ;
/// `dismiss_improve` la retire.
#[tokio::test(flavor = "multi_thread")]
async fn improve_conversation_without_process_is_attached_until_dismissed() {
    use orchestrator_lib::api::commands::AppState;
    use orchestrator_lib::api::events::NoticeLevel;

    let state = AppState::new(std::env::temp_dir());
    let key = SessionKey("8a7c1f3e-2b4d-4e6f-9a0b-1c2d3e4f5a6b".into());
    let conv = state.improve_conversation(&key, &std::env::temp_dir());
    assert_eq!(conv.outlet.key(), &key);
    assert!(state.improve_note(&key, NoticeLevel::Info, "improve.step", "préparation"));
    assert!(!state.improve_note(
        &SessionKey("autre".into()),
        NoticeLevel::Info,
        "improve.step",
        "x"
    ));
    let (a, got_a) = recorder();
    let resync: Vec<SessionResync> = state.attach_all(state.attachment(a));
    assert_eq!(resync.len(), 1);
    let r = &resync[0];
    assert_eq!(
        (r.session.clone(), r.role, r.pid, r.running),
        (key.clone(), SessionRole::Improve, 0, false)
    );
    assert!(r.resumed_from.is_none() && r.history.is_none());
    assert_eq!(r.events.len(), 1);
    assert!(matches!(
        &r.events[0].body,
        EventBody::Notice { code, .. } if code == "improve.step"
    ));
    // La suite part vers la page rattachée.
    conv.notice(NoticeLevel::Warn, "improve.check_failed", "tests en échec");
    wait_until("livré", Duration::from_secs(2), || {
        got_a.lock().unwrap().as_slice() == [1]
    })
    .await;

    // Rechargement : détachée, jamais fermée ; rendue en entier ensuite.
    state.close_orphans().await;
    assert!(!conv.outlet.is_attached());
    conv.notice(NoticeLevel::Info, "improve.step", "tour 2");
    let (b, _) = recorder();
    let again = state.attach_all(state.attachment(b));
    assert_eq!(again.len(), 1);
    let s: Vec<u64> = again[0].events.iter().map(|e| e.seq).collect();
    assert_contiguous(&s, 0, "conversation");
    assert_eq!(s.len(), 3);

    assert!(state.dismiss_improve(&key).await);
    let (c, _) = recorder();
    assert!(state.attach_all(state.attachment(c)).is_empty());
}

/// Mesure (risque du plan J14) : taille du journal sur la fixture longue
/// `t15` (1 000 tours, `stream_event` gardés dans 1 tour sur 10), et
/// projection avec tous les deltas. Seuil de compactage : 50 Mo sur 4 h.
#[tokio::test(flavor = "multi_thread")]
async fn journal_size_on_long_fixture() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/stream-json/t15.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let rep = replay_fixture(&text, "t15");
    let events = rep.events;
    let json_len = |evs: &[&CoreEvent]| -> usize {
        evs.iter()
            .map(|e| serde_json::to_vec(e).unwrap().len())
            .sum()
    };
    let all: Vec<&CoreEvent> = events.iter().collect();
    let deltas: Vec<&CoreEvent> = events
        .iter()
        .filter(|e| matches!(e.body, EventBody::TextDelta { .. }))
        .collect();
    let turns = events
        .iter()
        .filter(|e| matches!(e.body, EventBody::TurnResult { .. }))
        .count();
    let total = json_len(&all);
    let delta = json_len(&deltas);
    // Deltas présents dans 1 tour sur 10 : projection « tous les tours ».
    let full = total - delta + delta * 10;
    let mb = |b: usize| b as f64 / 1_048_576.0;
    println!(
        "t15 : {} événements, {turns} tours, journal {:.1} Mo (dont text_delta {} événements, {:.1} Mo) ; \
         tous les deltas : {:.1} Mo, soit {:.1} Ko par tour",
        events.len(),
        mb(total),
        deltas.len(),
        mb(delta),
        mb(full),
        full as f64 / 1024.0 / turns as f64
    );

    // Rattachement du journal complet de t15.
    let outlet = Outlet::spawn(SessionKey("t15".into()), None);
    for e in &events {
        outlet.emit(e.body.clone(), e.t);
    }
    let (sink, _) = recorder();
    let t0 = Instant::now();
    let snap = outlet.attach(sink, 1);
    let json = serde_json::to_vec(&snap).unwrap();
    let took = t0.elapsed();
    println!(
        "rattachement t15 : {} événements, {:.1} Mo en {took:?}",
        snap.len(),
        mb(json.len())
    );
    assert_eq!(snap.len(), events.len());
    assert!(turns >= 1000);
}

/// Même mesure sur une copie de flux brut réel (`logs/raw/*.ndjson`, même
/// format que les fixtures) : `ORCH_JOURNAL_RAW=<fichier> cargo test --test
/// attach_sessions -- --ignored --nocapture`.
#[test]
#[ignore = "mesure sur un flux brut local (ORCH_JOURNAL_RAW)"]
fn journal_size_on_raw_log() {
    let path = std::env::var("ORCH_JOURNAL_RAW").expect("ORCH_JOURNAL_RAW");
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
    let rep = replay_fixture(&text, "raw");
    let mut by_kind: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for e in &rep.events {
        let v = serde_json::to_value(e).unwrap();
        let k = v["kind"].as_str().unwrap_or("?").to_string();
        let n = serde_json::to_vec(&v).unwrap().len();
        let slot = by_kind.entry(k).or_default();
        slot.0 += 1;
        slot.1 += n;
    }
    let total: usize = by_kind.values().map(|x| x.1).sum();
    println!(
        "{path} : {} octets bruts, {} événements, journal {:.2} Mo",
        text.len(),
        rep.events.len(),
        total as f64 / 1_048_576.0
    );
    for (k, (n, b)) in by_kind {
        println!("  {k:<16} {n:>7} événements {:>8.1} Ko", b as f64 / 1024.0);
    }
}
