//! Watchdog du signal « prêt » (J14, étape 2), sans Tauri : un numéro de
//! génération par armement, de sorte qu'un minuteur périmé (réarmé,
//! désarmé par `ready`) ne déclenche rien.

use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Default)]
struct Armed {
    generation: u64,
    /// Hash attendu (`None` : rien d'attendu).
    expected: Option<String>,
}

/// État du watchdog.
#[derive(Debug, Default)]
pub struct Watchdog {
    state: Mutex<Armed>,
}

impl Watchdog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attend `hash` ; remplace l'armement précédent. Renvoie le jeton à
    /// passer à [`Watchdog::expire`].
    pub fn arm(&self, hash: &str) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.generation += 1;
        s.expected = Some(hash.to_string());
        s.generation
    }

    /// N'attend plus rien (les minuteurs en cours deviennent périmés).
    pub fn disarm(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.generation += 1;
        s.expected = None;
    }

    /// Hash attendu, s'il y en a un.
    pub fn expected(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expected
            .clone()
    }

    /// Signal « prêt » de l'interface `hash` (`None` : embarquée). Désarme
    /// et renvoie vrai si c'est le hash attendu ; sinon ne change rien.
    pub fn ready(&self, hash: Option<&str>) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match (&s.expected, hash) {
            (Some(want), Some(got)) if want == got => {
                s.generation += 1;
                s.expected = None;
                true
            }
            _ => false,
        }
    }

    /// Délai écoulé pour l'armement `token` : renvoie le hash attendu (et
    /// désarme) si cet armement est toujours en cours, sinon `None`.
    pub fn expire(&self, token: u64) -> Option<String> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.generation != token {
            return None;
        }
        s.generation += 1;
        s.expected.take()
    }
}

/// Lance `f` après `timeout` sur un fil à part.
pub fn after(timeout: Duration, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name("ui-watchdog".into())
        .spawn(move || {
            std::thread::sleep(timeout);
            f();
        })
        .expect("fil du watchdog d'interface");
}
