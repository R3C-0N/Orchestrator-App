//! Fournisseur d'assets de Tauri (J14, étape 2) : sert la version active
//! lue sur disque, **en bloc** — si aucune version valide n'est active,
//! toute l'interface vient des assets embarqués ; jamais de repli fichier
//! par fichier (un fichier absent de la version active rend `None`, Tauri
//! le remplace par `index.html` de la même version).

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
#[cfg(not(dev))]
use tauri::Manager;
use tauri::{App, Assets, Runtime};

use super::store::{clean_path, read_inside, sha256, Version};
use super::watchdog::Mode;
#[cfg(not(dev))]
use super::ActiveUi;
use super::{Inner, OnFallback, STARTUP_TIMEOUT};

/// Lit `key` (une `AssetKey`, `/` initial facultatif) dans la version
/// `v` : chemin propre ([`clean_path`]), présent dans le manifeste, lu sans
/// suivre de lien sous le dossier de la version, empreinte identique à
/// celle du manifeste. Sinon `None`, avec un avertissement au journal.
pub fn read_version_file(v: &Version, key: &str) -> Option<Vec<u8>> {
    let Some(parts) = clean_path(key) else {
        tracing::warn!("interface {} : chemin refusé {key:?}", v.hash);
        return None;
    };
    let rel = parts.join("/");
    let Some(want) = v.files.get(&rel) else {
        tracing::warn!("interface {} : {rel} absent du manifeste", v.hash);
        return None;
    };
    let Some(bytes) = read_inside(&v.dir, &parts) else {
        tracing::warn!("interface {} : {rel} illisible (absent ou lien)", v.hash);
        return None;
    };
    if sha256(&bytes) != *want {
        tracing::warn!(
            "interface {} : {rel} modifié depuis l'installation (empreinte différente)",
            v.hash
        );
        return None;
    }
    Some(bytes)
}

/// Assets embarqués vides : bouche-trou le temps de sortir les vrais
/// assets du `Context` (`UiHost::install_assets`).
pub(crate) struct NoAssets;

impl<R: Runtime> Assets<R> for NoAssets {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }
    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }
    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// Fournisseur installé par `Context::set_assets`.
pub struct DiskAssets<R: Runtime> {
    inner: Arc<Inner>,
    embedded: Box<dyn Assets<R>>,
    /// Rechargement des webviews, posé par `setup` (ou par les tests) :
    /// sans lui, le watchdog de démarrage n'est pas armé.
    reload: OnceLock<OnFallback>,
    /// Watchdog de démarrage déjà armé (premier `index.html`).
    startup_armed: AtomicBool,
}

impl<R: Runtime> DiskAssets<R> {
    pub(crate) fn new(
        inner: Arc<Inner>,
        embedded: Box<dyn Assets<R>>,
        reload: Option<OnFallback>,
    ) -> Self {
        let cell = OnceLock::new();
        if let Some(r) = reload {
            let _ = cell.set(r);
        }
        DiskAssets {
            inner,
            embedded,
            reload: cell,
            startup_armed: AtomicBool::new(false),
        }
    }

    /// Premier chargement de page (`index.html`) : watchdog de démarrage
    /// armé pour la version servie (rien pour l'embarquée).
    fn arm_startup_once(&self) {
        let Some(reload) = self.reload.get() else {
            return;
        };
        if self.startup_armed.swap(true, Ordering::SeqCst) {
            return;
        }
        super::arm(
            self.inner.clone(),
            self.inner.active().hash,
            STARTUP_TIMEOUT,
            Mode::Startup,
            reload.clone(),
        );
    }
}

impl<R: Runtime> Assets<R> for DiskAssets<R> {
    /// Démarrage : retient de quoi recharger les webviews. Hors `tauri
    /// dev` seulement (l'interface vient alors de Vite ; et les binaires de
    /// test n'instancient pas ainsi le code de la webview, dont l'import de
    /// `comctl32` v6 les empêcherait de démarrer).
    fn setup(&self, app: &App<R>) {
        self.embedded.setup(app);
        #[cfg(not(dev))]
        {
            let handle = app.app_handle().clone();
            let _ = self
                .reload
                .set(Arc::new(move |_ui: &ActiveUi| super::reload_all(&handle)));
        }
    }

    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        if key.as_ref() == "/index.html" {
            self.arm_startup_once();
        }
        match self.inner.serving() {
            Some(v) => read_version_file(&v, key.as_ref()).map(Cow::Owned),
            None => self.embedded.get(key),
        }
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        match self.inner.serving() {
            Some(v) => {
                let items: Vec<(Cow<'_, str>, Cow<'_, [u8]>)> = v
                    .files
                    .keys()
                    .filter_map(|rel| {
                        read_version_file(&v, rel)
                            .map(|b| (Cow::Owned(format!("/{rel}")), Cow::Owned(b)))
                    })
                    .collect();
                Box::new(items.into_iter())
            }
            None => self.embedded.iter(),
        }
    }

    /// Version disque : aucune empreinte (la sortie de Vite n'a pas de
    /// script en ligne ; un script en ligne serait bloqué par la CSP).
    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        match self.inner.serving() {
            Some(_) => Box::new(std::iter::empty()),
            None => self.embedded.csp_hashes(html_path),
        }
    }
}
