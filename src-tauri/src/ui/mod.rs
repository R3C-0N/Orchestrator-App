//! Hébergement de l'interface (J14, spec décision 15) : versions compilées
//! dans `%LOCALAPPDATA%\orchestrator\ui\<hash>\`, pointeur `active.json`,
//! fournisseur d'assets disque avec repli **en bloc** sur l'interface
//! embarquée, signal « prêt » surveillé, nettoyage des versions.
//!
//! Découpage : `store` (disque : manifeste, pointeur, état, installation,
//! nettoyage), `watchdog` (générations d'armement), `assets` (fournisseur
//! Tauri) ; ce module les relie. Tout ce qui touche la webview passe par un
//! rappel : `switch_to`, `step_back`, `arm_watchdog_with` se testent sans
//! Tauri (`tests/ui_host.rs`).
//!
//! Règle du watchdog : après une bascule (`activate`, `rollback`), pas de
//! « prêt » en [`READY_TIMEOUT`] → version fautive tout de suite, retour à
//! la précédente puis à l'embarquée. Au démarrage (armé au premier
//! chargement de `index.html` par le fournisseur), délai
//! [`STARTUP_TIMEOUT`] : premier échec → interface embarquée pour cette
//! exécution seulement ; deuxième échec consécutif → version fautive et
//! retour à la précédente. Un « prêt » remet le compteur à zéro.
//!
//! Branchement dans `lib.rs` (étape 5) : `ui.install_assets(&mut context)`
//! avant `build`, puis `app.manage(ui)`. Le cycle d'amélioration tient
//! `ui.build_lock(hash)` pendant le `vite build` vers `<root>\<hash>.tmp`.

pub mod assets;
pub mod store;
pub mod watchdog;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Assets, Manager, Runtime};

use crate::api::types::{CoreError, UiSource};
use store::{BuildLock, Store, Version};
use watchdog::{Mode, Watchdog};

/// Délai du signal « prêt » après une bascule.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Délai du signal « prêt » au démarrage de l'app (WebView2 à froid).
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
/// Échecs consécutifs au démarrage avant de marquer une version fautive.
pub const STARTUP_STRIKES: u32 = 2;
/// Versions gardées en plus de l'active et de la précédente.
pub const KEEP_RECENT: usize = 5;

/// Interface affichée, vue par le cœur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveUi {
    pub source: UiSource,
    pub hash: Option<String>,
    pub previous: Option<String>,
}

impl ActiveUi {
    fn embedded() -> Self {
        ActiveUi {
            source: UiSource::Embedded,
            hash: None,
            previous: None,
        }
    }
}

/// Rappel après un changement de version fait par le watchdog (l'app
/// recharge la webview ; les tests notent l'état).
pub type OnFallback = Arc<dyn Fn(&ActiveUi) + Send + Sync>;

/// État partagé entre `UiHost`, le fournisseur d'assets et les minuteurs.
pub(crate) struct Inner {
    store: Store,
    active: Mutex<ActiveUi>,
    /// Version servie ; `None` : interface embarquée (en bloc).
    serving: RwLock<Option<Arc<Version>>>,
    watchdog: Watchdog,
    /// Sérialise installation, bascule et nettoyage.
    op: Mutex<()>,
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::Io(e.to_string())
}

impl Inner {
    pub(crate) fn active(&self) -> ActiveUi {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn serving(&self) -> Option<Arc<Version>> {
        self.serving
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set(&self, version: Option<Version>, previous: Option<String>) -> ActiveUi {
        let ui = match &version {
            Some(v) => ActiveUi {
                source: UiSource::Disk,
                hash: Some(v.hash.clone()),
                previous,
            },
            None => ActiveUi {
                previous,
                ..ActiveUi::embedded()
            },
        };
        *self.serving.write().unwrap_or_else(|e| e.into_inner()) = version.map(Arc::new);
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = ui.clone();
        ui
    }

    /// Lit le pointeur et sert la version active si elle est valide et non
    /// fautive ; sinon toute l'interface embarquée.
    fn load(&self) -> ActiveUi {
        let _g = self.op.lock().unwrap_or_else(|e| e.into_inner());
        let p = self.store.read_pointer();
        let version = p.active.as_deref().and_then(|h| {
            self.store
                .load_version(h)
                .inspect_err(|e| tracing::warn!("interface {h} non servie : {e}"))
                .ok()
        });
        let previous = match &version {
            Some(v) => self
                .store
                .previous_of(&self.store.read_state(), Some(&v.hash)),
            None => None,
        };
        self.set(version, previous)
    }

    fn switch_to(&self, hash: &str) -> Result<ActiveUi, CoreError> {
        let _g = self.op.lock().unwrap_or_else(|e| e.into_inner());
        let v = self
            .store
            .load_version(hash)
            .map_err(|e| CoreError::InvalidArgument(format!("interface {hash} : {e}")))?;
        let p = self.store.record_activation(hash).map_err(io_err)?;
        Ok(self.set(Some(v), p.previous))
    }

    /// Quitte `current` pour la précédente valide (ou l'embarquée).
    fn step_back(&self, current: Option<&str>, bad: bool) -> Result<ActiveUi, CoreError> {
        let _g = self.op.lock().unwrap_or_else(|e| e.into_inner());
        let p = self.store.step_back(current, bad).map_err(io_err)?;
        let version = p
            .active
            .as_deref()
            .and_then(|h| self.store.load_version(h).ok());
        Ok(self.set(version, p.previous))
    }

    /// Délai écoulé sans « prêt » de `hash` (armé en `mode`). Renvoie la
    /// nouvelle interface, ou `None` si `hash` n'est plus l'active
    /// (bascule entre-temps) : rien à faire.
    fn fail(&self, hash: &str, mode: Mode) -> Option<ActiveUi> {
        if self.active().hash.as_deref() != Some(hash) {
            return None;
        }
        if mode == Mode::Startup {
            let n = self
                .store
                .record_startup_failure(hash)
                .unwrap_or(STARTUP_STRIKES);
            if n < STARTUP_STRIKES {
                tracing::warn!(
                    "interface {hash} : pas de « prêt » au démarrage ({n}/{STARTUP_STRIKES}), \
                     interface embarquée pour cette exécution"
                );
                // Pointeur et version inchangés : retentée au prochain
                // lancement.
                return Some(self.set(None, None));
            }
        }
        tracing::warn!("interface {hash} : pas de « prêt », version fautive, retour arrière");
        match self.step_back(Some(hash), true) {
            Ok(ui) => Some(ui),
            Err(e) => {
                tracing::error!("retour arrière impossible : {e}");
                // Au pire : interface embarquée en mémoire, la version reste
                // au moins marquée.
                let _ = self.store.mark_bad(hash);
                Some(self.set(None, None))
            }
        }
    }
}

/// Arme le watchdog pour `hash` : à l'échéance sans « prêt », `fail`, puis
/// réarmement (même contexte) pour la nouvelle version disque, puis
/// `on_fallback`. Chaîne finie : on ne fait que reculer, jusqu'à
/// l'embarquée, qui n'arme rien.
fn arm(
    inner: Arc<Inner>,
    hash: Option<String>,
    timeout: Duration,
    mode: Mode,
    on_fallback: OnFallback,
) {
    let Some(hash) = hash else {
        inner.watchdog.disarm();
        return;
    };
    let token = inner.watchdog.arm(&hash, mode);
    watchdog::after(timeout, move || {
        let Some((expected, mode)) = inner.watchdog.expire(token) else {
            return;
        };
        if let Some(ui) = inner.fail(&expected, mode) {
            // Armer avant de recharger : le « prêt » de la page rechargée
            // ne doit pas arriver avant l'armement.
            arm(inner, ui.hash.clone(), timeout, mode, on_fallback.clone());
            on_fallback(&ui);
        }
    });
}

/// Recharge toutes les webviews de l'app.
fn reload_all<R: Runtime>(app: &AppHandle<R>) {
    for (label, w) in app.webview_windows() {
        if let Err(e) = w.reload() {
            tracing::warn!("rechargement de {label} : {e}");
        }
    }
}

pub(crate) fn arm_with_reload<R: Runtime>(
    inner: Arc<Inner>,
    app: AppHandle<R>,
    hash: Option<String>,
    timeout: Duration,
    mode: Mode,
) {
    arm(
        inner,
        hash,
        timeout,
        mode,
        Arc::new(move |_ui: &ActiveUi| reload_all(&app)),
    );
}

/// État de l'hébergement, géré par Tauri (`app.manage`).
pub struct UiHost {
    inner: Arc<Inner>,
}

impl UiHost {
    /// `root` : dossier des versions (`default_root()` dans l'app, un
    /// dossier temporaire dans les tests). Ne touche pas au disque.
    pub fn new(root: PathBuf) -> Self {
        UiHost {
            inner: Arc::new(Inner {
                store: Store::new(root),
                active: Mutex::new(ActiveUi::embedded()),
                serving: RwLock::new(None),
                watchdog: Watchdog::new(),
                op: Mutex::new(()),
            }),
        }
    }

    /// `%LOCALAPPDATA%\orchestrator\ui`.
    pub fn default_root() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("orchestrator").join("ui"))
    }

    pub fn root(&self) -> &Path {
        self.inner.store.root()
    }

    /// Accès disque (manifeste, pointeur, état).
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Interface affichée.
    pub fn active(&self) -> ActiveUi {
        self.inner.active()
    }

    /// Lit `active.json` et choisit l'interface servie : la version active
    /// si son dossier, son manifeste et ses empreintes sont bons et qu'elle
    /// n'est pas fautive ; sinon toute l'interface embarquée.
    pub fn load(&self) -> ActiveUi {
        self.inner.load()
    }

    /// Fournisseur d'assets à passer à `Context::set_assets` : lit la
    /// version active sur disque (`load`), sinon `embedded`. En `tauri dev`
    /// (interface servie par Vite), renvoie `embedded` sans rien lire.
    pub fn assets<R: Runtime>(&self, embedded: Box<dyn Assets<R>>) -> Box<dyn Assets<R>> {
        if cfg!(dev) {
            embedded
        } else {
            self.provider(embedded)
        }
    }

    /// Fournisseur disque, quel que soit le mode de compilation (`load`
    /// fait ici). Il arme le watchdog de démarrage au premier chargement
    /// de `index.html`, une fois `Assets::setup` passé.
    pub fn provider<R: Runtime>(&self, embedded: Box<dyn Assets<R>>) -> Box<dyn Assets<R>> {
        self.load();
        Box::new(assets::DiskAssets::new(self.inner.clone(), embedded, None))
    }

    /// [`UiHost::provider`] avec le rappel de repli déjà posé (tests : le
    /// premier `index.html` arme le watchdog de démarrage sans `setup`).
    pub fn provider_with_reload<R: Runtime>(
        &self,
        embedded: Box<dyn Assets<R>>,
        on_fallback: OnFallback,
    ) -> Box<dyn Assets<R>> {
        self.load();
        Box::new(assets::DiskAssets::new(
            self.inner.clone(),
            embedded,
            Some(on_fallback),
        ))
    }

    /// Hash attendu par le watchdog, s'il est armé.
    pub fn watchdog_expected(&self) -> Option<String> {
        self.inner.watchdog.expected().map(|(h, _)| h)
    }

    /// Remplace les assets de `context` par [`UiHost::assets`] (à faire
    /// avant `Builder::build`).
    pub fn install_assets<R: Runtime>(&self, context: &mut tauri::Context<R>) {
        let embedded = context.set_assets(Box::new(assets::NoAssets));
        context.set_assets(self.assets(embedded));
    }

    /// Arme le watchdog après une bascule : sans `ready(hash)` dans
    /// `timeout`, version marquée fautive, retour à la précédente puis à
    /// l'embarquée, webviews rechargées. `hash` `None` (embarquée) :
    /// désarme.
    pub fn arm_watchdog<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        hash: Option<String>,
        timeout: Duration,
    ) {
        arm_with_reload(self.inner.clone(), app.clone(), hash, timeout, Mode::Switch);
    }

    /// Arme le watchdog de démarrage pour l'interface servie
    /// ([`STARTUP_TIMEOUT`], règle des deux échecs). Le fournisseur le fait
    /// seul au premier chargement de page ; à n'appeler qu'à défaut.
    pub fn arm_startup_watchdog<R: Runtime>(&self, app: &AppHandle<R>) {
        let hash = self.active().hash;
        arm_with_reload(
            self.inner.clone(),
            app.clone(),
            hash,
            STARTUP_TIMEOUT,
            Mode::Startup,
        );
    }

    /// [`UiHost::arm_watchdog`] sans Tauri : `on_fallback` remplace le
    /// rechargement.
    pub fn arm_watchdog_with(
        &self,
        hash: Option<String>,
        timeout: Duration,
        on_fallback: OnFallback,
    ) {
        arm(self.inner.clone(), hash, timeout, Mode::Switch, on_fallback);
    }

    /// [`UiHost::arm_startup_watchdog`] sans Tauri.
    pub fn arm_startup_watchdog_with(
        &self,
        hash: Option<String>,
        timeout: Duration,
        on_fallback: OnFallback,
    ) {
        arm(
            self.inner.clone(),
            hash,
            timeout,
            Mode::Startup,
            on_fallback,
        );
    }

    /// Signal « prêt » de l'interface (`ui_ready`) : désarme le watchdog si
    /// `hash` est celui attendu (et remet à zéro ses échecs au démarrage).
    /// Renvoie vrai s'il était attendu.
    pub fn ready(&self, hash: Option<&str>) -> bool {
        match (self.inner.watchdog.ready(hash), hash) {
            (Some(_), Some(h)) => {
                let _ = self.inner.store.reset_startup_failures(h);
                true
            }
            _ => false,
        }
    }

    /// Verrou de build de `hash`, à tenir pendant l'écriture de
    /// `<root>\<hash>.tmp` : le nettoyage n'y touche pas tant qu'il existe.
    pub fn build_lock(&self, hash: &str) -> Result<BuildLock, CoreError> {
        self.inner.store.build_lock(hash).map_err(io_err)
    }

    /// Installe le build `dir` (déjà écrit ; `<root>\<hash>.tmp` évite une
    /// copie) comme version `hash` : manifeste vérifié, renommage atomique.
    /// Ne l'active pas. Refusé pour la version servie et la précédente.
    pub fn install(&self, hash: &str, dir: &Path) -> Result<PathBuf, CoreError> {
        self.install_with(hash, dir, &mut |_| Ok(()))
    }

    /// [`UiHost::install`] avec un crochet à chaque étape (tests : arrêt
    /// brutal simulé).
    pub fn install_with(
        &self,
        hash: &str,
        dir: &Path,
        hook: &mut dyn FnMut(store::InstallStage) -> std::io::Result<()>,
    ) -> Result<PathBuf, CoreError> {
        let _g = self.inner.op.lock().unwrap_or_else(|e| e.into_inner());
        let ui = self.inner.active();
        let p = self.inner.store.read_pointer();
        if [&ui.hash, &ui.previous, &p.active, &p.previous]
            .iter()
            .any(|h| h.as_deref() == Some(hash))
        {
            return Err(CoreError::InvalidArgument(format!(
                "interface {hash} : version active ou précédente, pas de réinstallation"
            )));
        }
        self.inner
            .store
            .install_with(hash, dir, hook)
            .map_err(io_err)
    }

    /// Bascule vers `hash` sans toucher la webview : version vérifiée,
    /// `state.json` puis pointeur, fournisseur. Le watchdog n'est pas armé.
    pub fn switch_to(&self, hash: &str) -> Result<ActiveUi, CoreError> {
        self.inner.switch_to(hash)
    }

    /// Retour arrière sans toucher la webview : la version quittée est
    /// retirée des activations (pas marquée fautive) ; la précédente
    /// valide, sinon l'embarquée, est servie.
    ///
    /// Depuis l'interface embarquée : si le pointeur désigne encore une
    /// version (repli de démarrage pour cette exécution), c'est elle qui
    /// est quittée ; on active alors la dernière version disque valide
    /// **précédente** s'il y en a une, sinon on reste sur l'embarquée.
    pub fn step_back(&self) -> Result<ActiveUi, CoreError> {
        let cur = self
            .inner
            .active()
            .hash
            .or_else(|| self.inner.store.read_pointer().active);
        self.inner.step_back(cur.as_deref(), false)
    }

    /// Active la version `hash` (pointeur `active.json`) et recharge la
    /// fenêtre, watchdog armé.
    pub fn activate<R: Runtime>(&self, app: &AppHandle<R>, hash: &str) -> Result<(), CoreError> {
        let ui = self.switch_to(hash)?;
        self.arm_watchdog(app, ui.hash, READY_TIMEOUT);
        reload_all(app);
        Ok(())
    }

    /// Revient à la version précédente (ou à l'embarquée) et recharge.
    pub fn rollback<R: Runtime>(&self, app: &AppHandle<R>) -> Result<ActiveUi, CoreError> {
        let ui = self.step_back()?;
        self.arm_watchdog(app, ui.hash.clone(), READY_TIMEOUT);
        reload_all(app);
        Ok(ui)
    }

    /// Supprime les versions au-delà de l'active, de la précédente et des
    /// [`KEEP_RECENT`] plus récentes valides, les `.tmp` abandonnés (sans
    /// verrou de build, 30 min sans écriture) et les `.part` orphelins.
    /// Renvoie le nombre de dossiers supprimés.
    pub fn cleanup(&self) -> usize {
        self.cleanup_at(SystemTime::now())
    }

    /// [`UiHost::cleanup`] à la date `now` (tests).
    pub fn cleanup_at(&self, now: SystemTime) -> usize {
        let _g = self.inner.op.lock().unwrap_or_else(|e| e.into_inner());
        let ui = self.inner.active();
        let p = self.inner.store.read_pointer();
        let keep: Vec<&str> = [&ui.hash, &ui.previous, &p.active, &p.previous]
            .into_iter()
            .filter_map(|h| h.as_deref())
            .collect();
        self.inner.store.cleanup_at(&keep, KEEP_RECENT, now)
    }
}
