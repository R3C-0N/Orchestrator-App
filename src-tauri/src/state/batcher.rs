//! Regroupement des événements par lots (plan 1.5) : un lot part après 16 ms
//! d'accumulation ou à 256 événements, et immédiatement au **bord avant**
//! d'une rafale.
//!
//! Bord avant : l'événement part tout de suite, avec ce qui est déjà en file,
//! si aucun lot n'est parti depuis 16 ms (flux calme, règle d'origine) **ou**
//! si l'événement précédent date d'au moins [`BURST_GAP`]. La seconde
//! condition est celle de la recette J12 : à 100 `text_delta`/s (un toutes les
//! 10 ms), le dernier lot date toujours de moins de 16 ms, donc la règle
//! d'origine ne se déclenchait jamais et chaque delta attendait sa fenêtre
//! (7,8 ms en moyenne, test `steady_100_per_second`). Une rafale, elle, est
//! faite d'événements à moins de [`BURST_GAP`] d'écart : seul son premier
//! part seul, les suivants restent regroupés par fenêtres de 16 ms ou par 256
//! (même nombre de lots qu'avant, tests `burst_*` et `dense_*`).
//!
//! Écho d'un message tapé (`user_message`, origine `Typed`) : il part tout de
//! suite, avec ce qui est déjà en file, même au milieu d'une rafale (critère
//! « message visible < 50 ms après Entrée », recette J12). La fenêtre de
//! 16 ms sert au débit du flux, pas aux actions de l'utilisateur.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use crate::api::events::{CoreEvent, EventBody, UserOrigin};

pub const BATCH_WINDOW: Duration = Duration::from_millis(16);
pub const BATCH_MAX: usize = 256;
/// Écart minimal avec l'événement précédent pour qu'un événement ouvre une
/// nouvelle rafale (bord avant). Au-dessous, il rejoint la rafale en cours.
/// 5 ms : au plus 200 lots/s pour un flux régulier, un flux plus dense est
/// regroupé ; le flux en streaming du CLI (≈ 100 deltas/s) passe sans attente.
pub const BURST_GAP: Duration = Duration::from_millis(5);

/// Événement qui n'attend jamais la fenêtre : l'écho d'un message tapé.
fn urgent(ev: &CoreEvent) -> bool {
    matches!(
        ev.body,
        EventBody::UserMessage {
            origin: UserOrigin::Typed,
            ..
        }
    )
}

/// Destination d'un lot. Renvoie `false` si la destination est fermée.
pub type Sink = Arc<dyn Fn(Vec<CoreEvent>) -> bool + Send + Sync>;

pub async fn run_batcher(mut rx: UnboundedReceiver<CoreEvent>, sink: Sink) {
    let mut last_flush = Instant::now() - BATCH_WINDOW;
    let mut last_event: Option<Instant> = None;
    loop {
        let Some(first) = rx.recv().await else { break };
        let now = Instant::now();
        let leading_edge = last_flush.elapsed() >= BATCH_WINDOW
            || last_event.is_none_or(|t| now.duration_since(t) >= BURST_GAP)
            || urgent(&first);
        last_event = Some(now);
        let mut buf = vec![first];
        if leading_edge {
            // Bord avant : on part tout de suite avec ce qui est déjà là.
            // Les événements vidés ici comptent comme reçus maintenant : le
            // suivant mesure son écart à eux, pas seulement à `first`.
            while buf.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(ev) => {
                        last_event = Some(Instant::now());
                        buf.push(ev);
                    }
                    Err(_) => break,
                }
            }
        } else {
            let deadline = last_flush + BATCH_WINDOW;
            while buf.len() < BATCH_MAX {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(ev) => {
                            last_event = Some(Instant::now());
                            let now_urgent = urgent(&ev);
                            buf.push(ev);
                            if now_urgent {
                                break;
                            }
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep_until(deadline) => break,
                }
            }
        }
        last_flush = Instant::now();
        if !sink(buf) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBody;
    use crate::api::types::SessionKey;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    fn ev(seq: u64) -> CoreEvent {
        CoreEvent {
            seq,
            t: 0,
            session: SessionKey("k".into()),
            body: EventBody::Stderr {
                line: String::new(),
            },
        }
    }

    /// Lots reçus : (instant d'envoi, seq du lot).
    type Batches = Arc<Mutex<Vec<(Instant, Vec<u64>)>>>;

    /// Envoie `n` événements espacés de `gap` (zéro : d'un seul coup), puis
    /// renvoie les lots et l'instant d'émission de chaque événement.
    async fn run(n: u64, gap: Duration) -> (Vec<(Instant, Vec<u64>)>, Vec<Instant>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let batches: Batches = Arc::default();
        let b = batches.clone();
        let sink: Sink = Arc::new(move |batch: Vec<CoreEvent>| {
            b.lock()
                .unwrap()
                .push((Instant::now(), batch.iter().map(|e| e.seq).collect()));
            true
        });
        let task = tokio::spawn(run_batcher(rx, sink));
        let mut sent = Vec::new();
        for i in 0..n {
            sent.push(Instant::now());
            tx.send(ev(i)).unwrap();
            if !gap.is_zero() {
                tokio::time::sleep(gap).await;
            }
        }
        drop(tx);
        task.await.unwrap();
        let out = batches.lock().unwrap().clone();
        (out, sent)
    }

    /// Attente de chaque événement dans le regroupeur (ms) et nombre de lots.
    fn waits(batches: &[(Instant, Vec<u64>)], sent: &[Instant]) -> Vec<f64> {
        let mut w = vec![0.0; sent.len()];
        for (at, seqs) in batches {
            for &s in seqs {
                w[s as usize] = (*at - sent[s as usize]).as_secs_f64() * 1000.0;
            }
        }
        w
    }

    fn all_in_order(batches: &[(Instant, Vec<u64>)], n: u64) {
        let seqs: Vec<u64> = batches.iter().flat_map(|(_, s)| s.clone()).collect();
        assert_eq!(seqs, (0..n).collect::<Vec<_>>(), "ordre et exhaustivité");
    }

    #[tokio::test(start_paused = true)]
    async fn calm_event_leaves_at_once() {
        let (b, sent) = run(1, Duration::ZERO).await;
        assert_eq!(b.len(), 1);
        assert_eq!(waits(&b, &sent), vec![0.0]);
    }

    /// Flux régulier à 100 événements/s (un `text_delta` toutes les 10 ms,
    /// comme la recette) : attente moyenne et maximale dans le regroupeur.
    #[tokio::test(start_paused = true)]
    async fn steady_100_per_second() {
        let (b, sent) = run(200, Duration::from_millis(10)).await;
        all_in_order(&b, 200);
        let w = waits(&b, &sent);
        let mean = w.iter().sum::<f64>() / w.len() as f64;
        let max = w.iter().cloned().fold(0.0, f64::max);
        println!(
            "100/s : {} lots, attente moyenne {mean:.2} ms, max {max:.2} ms",
            b.len()
        );
        // Avant le bord avant par écart (règle d'origine seule) : 126 lots,
        // attente moyenne 7,80 ms, max 16 ms. Maintenant : chaque delta part
        // à son arrivée.
        assert_eq!(b.len(), 200);
        assert!(max < 1e-9, "attente max {max} ms");
    }

    /// Flux régulier mais plus dense que `BURST_GAP` (un événement toutes les
    /// 4 ms) : regroupé comme avant, pas un lot par événement.
    #[tokio::test(start_paused = true)]
    async fn steady_250_per_second_stays_grouped() {
        let (b, sent) = run(250, Duration::from_millis(4)).await;
        all_in_order(&b, 250);
        let max = waits(&b, &sent).iter().cloned().fold(0.0, f64::max);
        println!("250/s : {} lots, attente max {max:.2} ms", b.len());
        assert!(b.len() <= 1000 / 16 + 2, "{} lots", b.len());
        assert!(max <= 16.0 + 1e-9);
    }

    /// Flux régulier à exactement `BURST_GAP` (5 ms) : frontière `>=`, chaque
    /// événement est un bord avant. C'est le plafond du bord avant : environ
    /// 200 lots/s (1 000 / 5) pour un flux régulier.
    #[tokio::test(start_paused = true)]
    async fn steady_at_burst_gap_is_leading_edge() {
        let (b, sent) = run(200, BURST_GAP).await;
        all_in_order(&b, 200);
        let w = waits(&b, &sent);
        let max = w.iter().cloned().fold(0.0, f64::max);
        let span = (b.last().unwrap().0 - b[0].0).as_secs_f64();
        let per_second = (b.len() - 1) as f64 / span;
        println!(
            "200/s : {} lots, {per_second:.0} lots/s, attente max {max:.2} ms",
            b.len()
        );
        assert_eq!(b.len(), 200, "un lot par événement");
        assert!(max < 1e-9, "attente max {max} ms");
        assert!((per_second - 200.0).abs() < 1e-6, "{per_second} lots/s");
    }

    /// Rafale après un flux régulier : seul son premier événement part seul.
    #[tokio::test(start_paused = true)]
    async fn burst_after_steady_stream() {
        let (tx, rx) = mpsc::unbounded_channel();
        let batches: Batches = Arc::default();
        let bb = batches.clone();
        let sink: Sink = Arc::new(move |batch: Vec<CoreEvent>| {
            bb.lock()
                .unwrap()
                .push((Instant::now(), batch.iter().map(|e| e.seq).collect()));
            true
        });
        let task = tokio::spawn(run_batcher(rx, sink));
        for i in 0..10 {
            tx.send(ev(i)).unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for i in 10..1010 {
            tx.send(ev(i)).unwrap();
        }
        drop(tx);
        task.await.unwrap();
        let b = batches.lock().unwrap().clone();
        all_in_order(&b, 1010);
        // 10 lots du flux régulier, puis la rafale de 1 000 : le premier seul
        // avec ce qui est déjà en file (256), puis par 256.
        println!("régulier puis rafale : {} lots", b.len());
        assert_eq!(b.len(), 10 + 1000_usize.div_ceil(BATCH_MAX));
    }

    /// Rafale : 2 000 événements d'un seul coup. Autant de lots qu'avant
    /// (un par 256), dans l'ordre.
    #[tokio::test(start_paused = true)]
    async fn burst_is_batched_by_256() {
        let (b, _) = run(2000, Duration::ZERO).await;
        all_in_order(&b, 2000);
        println!("rafale 2000 : {} lots", b.len());
        assert_eq!(b.len(), 2000_usize.div_ceil(BATCH_MAX));
    }

    /// Flux dense (1 événement par ms pendant 320 ms) : regroupé par
    /// fenêtres de 16 ms, pas un lot par événement.
    #[tokio::test(start_paused = true)]
    async fn dense_stream_stays_grouped() {
        let (b, sent) = run(320, Duration::from_millis(1)).await;
        all_in_order(&b, 320);
        let w = waits(&b, &sent);
        let max = w.iter().cloned().fold(0.0, f64::max);
        println!("1000/s : {} lots, attente max {max:.2} ms", b.len());
        assert!(b.len() <= 320 / 16 + 2, "{} lots", b.len());
        assert!(max <= 16.0 + 1e-9);
    }

    /// Débit : 100 000 événements d'un seul coup, lots pleins.
    #[tokio::test(start_paused = true)]
    async fn throughput_full_batches() {
        let (b, _) = run(100_000, Duration::ZERO).await;
        all_in_order(&b, 100_000);
        assert_eq!(b.len(), 100_000_usize.div_ceil(BATCH_MAX));
    }

    /// Écho d'un message tapé au milieu d'une rafale : il part tout de suite
    /// avec ce qui est en file, sans attendre l'échéance de 16 ms.
    #[tokio::test(start_paused = true)]
    async fn typed_user_message_is_never_held() {
        let (tx, rx) = mpsc::unbounded_channel();
        let batches: Batches = Arc::default();
        let bb = batches.clone();
        let sink: Sink = Arc::new(move |batch: Vec<CoreEvent>| {
            bb.lock()
                .unwrap()
                .push((Instant::now(), batch.iter().map(|e| e.seq).collect()));
            true
        });
        let task = tokio::spawn(run_batcher(rx, sink));
        // Rafale : 0 part seul (bord avant), puis 1 et 2, 1 ms après,
        // attendent l'échéance de 16 ms…
        tx.send(ev(0)).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        for i in 1..3 {
            tx.send(ev(i)).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
        let typed_at = Instant::now();
        let mut echo = ev(3);
        echo.body = EventBody::UserMessage {
            thread: crate::api::events::ThreadId::main(),
            uuid: None,
            text: "bonjour".into(),
            origin: UserOrigin::Typed,
        };
        tx.send(echo).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        // … et un écho à 2 ms les emporte tout de suite (à 0 ms près).
        drop(tx);
        task.await.unwrap();
        let b = batches.lock().unwrap().clone();
        all_in_order(&b, 4);
        let (at, seqs) = b.iter().find(|(_, s)| s.contains(&3)).unwrap();
        assert_eq!(seqs, &vec![1, 2, 3]);
        assert_eq!(*at, typed_at, "écho retenu {:?}", *at - typed_at);
    }

    /// Écho tapé en tête de boucle (`urgent(&first)`) : 1 ms après un lot,
    /// donc écart < `BURST_GAP` et dernier lot < `BATCH_WINDOW`. Sans
    /// `urgent`, il attendrait l'échéance du lot (15 ms). Il emporte ce qui
    /// est déjà en file derrière lui.
    #[tokio::test(start_paused = true)]
    async fn typed_echo_at_loop_head_leaves_at_once() {
        let (tx, rx) = mpsc::unbounded_channel();
        let batches: Batches = Arc::default();
        let bb = batches.clone();
        let sink: Sink = Arc::new(move |batch: Vec<CoreEvent>| {
            bb.lock()
                .unwrap()
                .push((Instant::now(), batch.iter().map(|e| e.seq).collect()));
            true
        });
        let task = tokio::spawn(run_batcher(rx, sink));
        tx.send(ev(0)).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        // 0 est parti seul (bord avant) ; la boucle attend en tête.
        assert_eq!(batches.lock().unwrap().len(), 1);
        let typed_at = Instant::now();
        let mut echo = ev(1);
        echo.body = EventBody::UserMessage {
            thread: crate::api::events::ThreadId::main(),
            uuid: None,
            text: "bonjour".into(),
            origin: UserOrigin::Typed,
        };
        tx.send(echo).unwrap();
        tx.send(ev(2)).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(tx);
        task.await.unwrap();
        let b = batches.lock().unwrap().clone();
        all_in_order(&b, 3);
        assert_eq!(b.len(), 2, "{b:?}");
        assert_eq!(b[1].1, vec![1, 2]);
        assert_eq!(b[1].0, typed_at, "écho retenu {:?}", b[1].0 - typed_at);
    }
}
