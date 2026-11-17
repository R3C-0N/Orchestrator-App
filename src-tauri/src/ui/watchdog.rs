//! Watchdog du signal « prêt » (J14, étape 2), sans Tauri : un numéro de
//! génération par armement, de sorte qu'un minuteur périmé (réarmé,
//! désarmé par `ready`) ne déclenche rien. `ready` et `expire` passent par
//! le même verrou : à l'échéance, un seul des deux l'emporte.

use std::sync::Mutex;
use std::time::Duration;

/// Contexte de l'armement, qui fixe la sanction d'un dépassement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Après une bascule (`activate`, retour arrière) : version fautive
    /// dès le premier dépassement.
    Switch,
    /// Au démarrage de l'app : fautive au deuxième échec consécutif ; au
    /// premier, interface embarquée pour cette exécution seulement.
    Startup,
}

#[derive(Debug, Default)]
struct Armed {
    generation: u64,
    /// Hash attendu et contexte (`None` : rien d'attendu).
    expected: Option<(String, Mode)>,
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
    pub fn arm(&self, hash: &str, mode: Mode) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.generation += 1;
        s.expected = Some((hash.to_string(), mode));
        s.generation
    }

    /// N'attend plus rien (les minuteurs en cours deviennent périmés).
    pub fn disarm(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.generation += 1;
        s.expected = None;
    }

    /// Hash attendu et contexte, s'il y en a un.
    pub fn expected(&self) -> Option<(String, Mode)> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expected
            .clone()
    }

    /// Signal « prêt » de l'interface `hash` (`None` : embarquée). Désarme
    /// et renvoie le contexte si c'est le hash attendu ; sinon ne change
    /// rien.
    pub fn ready(&self, hash: Option<&str>) -> Option<Mode> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match (&s.expected, hash) {
            (Some((want, mode)), Some(got)) if want == got => {
                let mode = *mode;
                s.generation += 1;
                s.expected = None;
                Some(mode)
            }
            _ => None,
        }
    }

    /// Délai écoulé pour l'armement `token` : renvoie le hash attendu et
    /// le contexte (et désarme) si cet armement est toujours en cours.
    pub fn expire(&self, token: u64) -> Option<(String, Mode)> {
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
