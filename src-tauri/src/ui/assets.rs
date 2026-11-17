//! Fournisseur d'assets de Tauri (J14, étape 2) : sert la version active
//! lue sur disque, **en bloc** — si aucune version valide n'est active,
//! toute l'interface vient des assets embarqués ; jamais de repli fichier
//! par fichier (un fichier absent de la version active rend `None`, Tauri
//! le remplace par `index.html` de la même version).

use std::borrow::Cow;
use std::sync::Arc;

use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, Assets, Runtime};

use super::store::{clean_path, read_inside, sha256, Version};
use super::Inner;

/// Lit `key` (une `AssetKey`, `/` initial facultatif) dans la version
/// `v` : chemin propre ([`clean_path`]), présent dans le manifeste, lu sans
/// suivre de lien sous le dossier de la version, empreinte identique à
/// celle du manifeste. Sinon `None`.
pub fn read_version_file(v: &Version, key: &str) -> Option<Vec<u8>> {
    let parts = clean_path(key)?;
    let rel = parts.join("/");
    let want = v.files.get(&rel)?;
    let bytes = read_inside(&v.dir, &parts)?;
    (sha256(&bytes) == *want).then_some(bytes)
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
    pub(crate) inner: Arc<Inner>,
    pub(crate) embedded: Box<dyn Assets<R>>,
}

impl<R: Runtime> Assets<R> for DiskAssets<R> {
    /// Démarrage de l'app : watchdog armé si une version disque est active
    /// (pas en `tauri dev`, où l'interface vient du serveur Vite).
    fn setup(&self, app: &App<R>) {
        self.embedded.setup(app);
        #[cfg(not(dev))]
        {
            let hash = self.inner.active().hash;
            super::arm_with_reload(
                self.inner.clone(),
                app.handle().clone(),
                hash,
                super::READY_TIMEOUT,
            );
        }
    }

    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
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
