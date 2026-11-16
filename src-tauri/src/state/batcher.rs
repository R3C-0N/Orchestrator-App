//! Regroupement des événements par lots (plan 1.5) : un lot part après 16 ms
//! d'accumulation ou à 256 événements, et immédiatement quand le flux est calme
//! (premier événement après 16 ms d'inactivité).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use crate::api::events::CoreEvent;

pub const BATCH_WINDOW: Duration = Duration::from_millis(16);
pub const BATCH_MAX: usize = 256;

/// Destination d'un lot. Renvoie `false` si la destination est fermée.
pub type Sink = Arc<dyn Fn(Vec<CoreEvent>) -> bool + Send + Sync>;

pub async fn run_batcher(mut rx: UnboundedReceiver<CoreEvent>, sink: Sink) {
    let mut last_flush = Instant::now() - BATCH_WINDOW;
    loop {
        let Some(first) = rx.recv().await else { break };
        let mut buf = vec![first];
        if last_flush.elapsed() >= BATCH_WINDOW {
            // Flux calme : on part tout de suite avec ce qui est déjà là.
            while buf.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(ev) => buf.push(ev),
                    Err(_) => break,
                }
            }
        } else {
            let deadline = last_flush + BATCH_WINDOW;
            while buf.len() < BATCH_MAX {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(ev) => buf.push(ev),
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
