//! Hébergement de l'interface (J14, spec décision 15) : versions compilées
//! dans `%LOCALAPPDATA%\orchestrator\ui\<hash>\`, pointeur `active.json`,
//! fournisseur d'assets disque avec repli **en bloc** sur l'interface
//! embarquée, signal « prêt » et watchdog de 5 s, nettoyage des versions.
//!
//! Étape 0 : squelette, signatures figées. Le fournisseur rend l'interface
//! embarquée telle quelle et le watchdog ne fait rien : aucun comportement
//! ne change. Propriétaire : agent B (`src-tauri/src/ui/**`). Le branchement
//! dans `lib.rs` (`Context::set_assets`, armement au démarrage) est fait à
//! l'étape 5.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Assets, Runtime};

use crate::api::types::{CoreError, UiSource};

/// Délai du signal « prêt » après une bascule ou un démarrage.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Versions gardées en plus de l'active et de la précédente.
pub const KEEP_RECENT: usize = 5;

/// Interface affichée, vue par le cœur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveUi {
    pub source: UiSource,
    pub hash: Option<String>,
    pub previous: Option<String>,
}

/// État de l'hébergement, géré par Tauri (`app.manage`).
pub struct UiHost {
    root: PathBuf,
    active: Mutex<ActiveUi>,
}

impl UiHost {
    /// `root` : dossier des versions (`default_root()` dans l'app, un
    /// dossier temporaire dans les tests). Ne touche pas au disque.
    pub fn new(root: PathBuf) -> Self {
        UiHost {
            root,
            active: Mutex::new(ActiveUi {
                source: UiSource::Embedded,
                hash: None,
                previous: None,
            }),
        }
    }

    /// `%LOCALAPPDATA%\orchestrator\ui`.
    pub fn default_root() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("orchestrator").join("ui"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Interface affichée.
    pub fn active(&self) -> ActiveUi {
        self.active.lock().unwrap().clone()
    }

    /// Fournisseur d'assets à passer à `Context::set_assets` : lit la
    /// version active sur disque, sinon `embedded`. Étape 0 : `embedded`.
    pub fn assets<R: Runtime>(&self, embedded: Box<dyn Assets<R>>) -> Box<dyn Assets<R>> {
        embedded
    }

    /// Arme le watchdog : sans `ready(hash)` dans `timeout`, retour à la
    /// version précédente puis à l'embarquée, version marquée fautive.
    /// Étape 0 : ne fait rien.
    pub fn arm_watchdog<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        hash: Option<String>,
        timeout: Duration,
    ) {
        let _ = (app, hash, timeout);
    }

    /// Signal « prêt » de l'interface (`ui_ready`) : désarme le watchdog si
    /// `hash` est celui attendu. Renvoie vrai s'il était attendu.
    pub fn ready(&self, hash: Option<&str>) -> bool {
        let _ = hash;
        false
    }

    /// Installe le build `dir` (déjà écrit) comme version `hash` : manifeste
    /// vérifié, renommage atomique. Ne l'active pas.
    pub fn install(&self, hash: &str, dir: &Path) -> Result<PathBuf, CoreError> {
        let _ = (hash, dir);
        Err(CoreError::NotImplemented("ui::install".into()))
    }

    /// Active la version `hash` (pointeur `active.json`) et recharge la
    /// fenêtre, watchdog armé.
    pub fn activate<R: Runtime>(&self, app: &AppHandle<R>, hash: &str) -> Result<(), CoreError> {
        let _ = (app, hash);
        Err(CoreError::NotImplemented("ui::activate".into()))
    }

    /// Revient à la version précédente (ou à l'embarquée) et recharge.
    pub fn rollback<R: Runtime>(&self, app: &AppHandle<R>) -> Result<ActiveUi, CoreError> {
        let _ = app;
        Err(CoreError::NotImplemented("ui_rollback".into()))
    }

    /// Supprime les versions au-delà de l'active, de la précédente et des
    /// [`KEEP_RECENT`] plus récentes valides. Renvoie le nombre supprimé.
    pub fn cleanup(&self) -> usize {
        0
    }
}
