//! Stockage des versions d'interface sur disque (J14, étape 2), sans Tauri :
//! dossiers `<root>\<hash>\`, manifeste SHA-256, pointeur `active.json`,
//! état `state.json` (versions fautives, ordre des installations et des
//! activations), installation atomique et nettoyage.
//!
//! Disposition de `<root>` :
//! - `<hash>\` : une version installée, avec son `manifest.json` ;
//! - `<hash>.tmp\` : une version en cours d'installation (abandonnée si
//!   l'app s'est arrêtée au milieu : supprimée par le nettoyage) ;
//! - `active.json` : `{ "active": "<hash>" | null, "previous": … }` ;
//! - `state.json` : `{ "bad": […], "installs": […], "activations": […] }`.
//!
//! Toute écriture de `active.json` et `state.json` passe par un fichier
//! temporaire puis un renommage : un arrêt brutal laisse l'ancien contenu.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Pointeur vers la version active.
pub const ACTIVE_FILE: &str = "active.json";
/// Versions fautives, ordre des installations et des activations.
pub const STATE_FILE: &str = "state.json";
/// Manifeste d'une version (à la racine de son dossier, hors de sa liste).
pub const MANIFEST_FILE: &str = "manifest.json";
/// Suffixe d'un dossier en cours d'installation.
pub const TMP_SUFFIX: &str = ".tmp";
/// Suffixe d'une version mise de côté pendant son remplacement.
pub const OLD_SUFFIX: &str = ".old";
/// Suffixe du verrou d'un build en cours (`<hash>.lock`, fichier).
pub const LOCK_SUFFIX: &str = ".lock";
/// Suffixe d'un fichier temporaire de [`write_atomic`].
pub const PART_SUFFIX: &str = ".part";
/// Âge au-delà duquel un `.tmp` sans verrou ou un `.part` est abandonné.
pub const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Version du format du manifeste.
pub const MANIFEST_VERSION: u32 = 1;
/// Activations gardées dans `state.json` (chaîne des retours arrière).
const MAX_ACTIVATIONS: usize = 50;

/// `active.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pointer {
    pub active: Option<String>,
    pub previous: Option<String>,
}

/// `state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Versions fautives (pas de signal « prêt ») : jamais réactivées.
    pub bad: BTreeSet<String>,
    /// Ordre des installations, la plus récente à la fin.
    pub installs: Vec<String>,
    /// Ordre des activations, la plus récente à la fin : la précédente
    /// d'une version est la dernière activation valide avant elle.
    pub activations: Vec<String>,
    /// Échecs consécutifs du signal « prêt » au démarrage, par version
    /// (remis à zéro par un « prêt ») : fautive au deuxième.
    pub startup_failures: BTreeMap<String, u32>,
}

/// `manifest.json` : chemin relatif (séparateur `/`) → SHA-256 en hexa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub hash: String,
    pub files: BTreeMap<String, String>,
}

/// Version vérifiée : chaque fichier du manifeste existe et a la bonne
/// empreinte au moment de la vérification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub hash: String,
    pub dir: PathBuf,
    pub files: BTreeMap<String, [u8; 32]>,
}

/// Raison pour laquelle une version n'est pas servie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    BadHash,
    Missing,
    Link,
    Manifest(String),
    MarkedBad,
    File(String),
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Invalid::BadHash => write!(f, "hash invalide"),
            Invalid::Missing => write!(f, "dossier absent"),
            Invalid::Link => write!(f, "lien refusé"),
            Invalid::Manifest(e) => write!(f, "manifeste invalide : {e}"),
            Invalid::MarkedBad => write!(f, "version marquée fautive"),
            Invalid::File(p) => write!(f, "fichier absent ou modifié : {p}"),
        }
    }
}

/// Étapes de l'installation, signalées au crochet de `install_with` (un
/// `Err` du crochet arrête l'installation comme un arrêt brutal : rien
/// n'est nettoyé, le pointeur n'est pas touché).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStage {
    /// Build copié dans `<hash>.tmp`.
    Copied,
    /// `manifest.json` écrit dans `<hash>.tmp`.
    ManifestWritten,
    /// Manifeste relu et empreintes vérifiées.
    Verified,
    /// `<hash>.tmp` renommé en `<hash>`.
    Renamed,
}

/// Longueur minimale d'un hash de version (hash court de git).
pub const HASH_MIN: usize = 7;
/// Longueur maximale d'un hash de version (SHA-256 en hexa).
pub const HASH_MAX: usize = 64;

/// Hash de version accepté : hexadécimal minuscule, de [`HASH_MIN`] à
/// [`HASH_MAX`] caractères (un hash de commit git, court ou long). Exclut
/// d'office la casse mêlée, les noms réservés de Windows et les noms de
/// dossiers ordinaires (`logs`, `cache`…).
pub fn valid_hash(hash: &str) -> bool {
    (HASH_MIN..=HASH_MAX).contains(&hash.len())
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Nom réservé de Windows (`CON`, `PRN`, `AUX`, `NUL`, `COM0-9`,
/// `LPT0-9`), avec ou sans extension, quelle que soit la casse.
fn reserved_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s if s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) => {
            s.as_bytes()[3].is_ascii_digit()
        }
        _ => false,
    }
}

/// Chemin relatif réduit à des composants normaux, séparés par `/` :
/// refuse vide, `.`, `..`, `\`, `:` (lecteur, flux NTFS), `%` (encodage
/// résiduel, dont `%2e%2e`), caractères de contrôle, chemin absolu
/// (`/` initial accepté une fois, comme dans une `AssetKey`, mais pas `//`),
/// composant finissant par un point ou une espace (Windows les retire en
/// silence), noms réservés de Windows.
pub fn clean_path(raw: &str) -> Option<Vec<&str>> {
    let rel = raw.strip_prefix('/').unwrap_or(raw);
    if rel.is_empty() || rel.starts_with('/') {
        return None;
    }
    if rel
        .chars()
        .any(|c| matches!(c, '\\' | ':' | '%') || c.is_control())
    {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || p.ends_with(['.', ' ']) || reserved_name(p))
    {
        return None;
    }
    Some(parts)
}

/// SHA-256 en hexadécimal minuscule.
pub fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Lit `parts` sous `dir` sans suivre de lien (chaque composant est
/// vérifié). `None` si absent, lien ou pas un fichier.
pub fn read_inside(dir: &Path, parts: &[&str]) -> Option<Vec<u8>> {
    let mut p = dir.to_path_buf();
    for part in parts {
        p.push(part);
        let m = std::fs::symlink_metadata(&p).ok()?;
        if m.file_type().is_symlink() {
            return None;
        }
    }
    if !std::fs::symlink_metadata(&p).ok()?.is_file() {
        return None;
    }
    std::fs::read(&p).ok()
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Écrit `path` par un fichier temporaire voisin puis un renommage
/// (remplace l'ancien fichier d'un coup, `MoveFileExW` sous Windows).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}-{n}{PART_SUFFIX}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    rename_retry(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Renommage, réessayé brièvement (antivirus, indexeur qui tient un
/// fichier ouvert sous Windows).
fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut last = None;
    for i in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(20 * (i + 1)));
    }
    Err(last.unwrap())
}

/// Date de modification la plus récente de l'arborescence `p` (un build
/// en cours écrit dans des sous-dossiers sans toucher la racine).
fn tree_mtime(p: &Path) -> SystemTime {
    let mut newest = std::fs::symlink_metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let t = if ft.is_dir() {
                tree_mtime(&e.path())
            } else {
                e.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH)
            };
            newest = newest.max(t);
        }
    }
    newest
}

fn stale(t: SystemTime, now: SystemTime) -> bool {
    now.duration_since(t).is_ok_and(|age| age > STALE_AFTER)
}

/// Verrou d'un build en cours (`<root>\<hash>.lock`) : tant qu'il existe,
/// le nettoyage ne touche pas `<hash>.tmp`. Retiré à la destruction.
#[derive(Debug)]
pub struct BuildLock {
    path: PathBuf,
}

impl BuildLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if ft.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("lien refusé dans le build : {}", entry.path().display()),
            ));
        } else if ft.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Liste les fichiers de `dir` (chemins relatifs `/`), sans le manifeste
/// de la racine ; refuse les liens et les noms qui ne passent pas
/// [`clean_path`].
fn list_files(dir: &Path) -> io::Result<Vec<String>> {
    fn walk(base: &Path, rel: &str, out: &mut Vec<String>) -> io::Result<()> {
        let here = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rel)
        };
        for entry in std::fs::read_dir(here)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_symlink() || clean_path(&path).is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("entrée refusée : {path}"),
                ));
            }
            if ft.is_dir() {
                walk(base, &path, out)?;
            } else if !(rel.is_empty() && name == MANIFEST_FILE) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    out.sort();
    Ok(out)
}

/// Accès aux versions sous `root`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>\<hash>` (sans vérifier le hash).
    pub fn version_dir(&self, hash: &str) -> PathBuf {
        self.root.join(hash)
    }

    /// Pose le verrou de build de `hash` (échoue s'il est déjà posé).
    pub fn build_lock(&self, hash: &str) -> io::Result<BuildLock> {
        if !valid_hash(hash) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("hash invalide : {hash:?}"),
            ));
        }
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{hash}{LOCK_SUFFIX}"));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(BuildLock { path })
    }

    /// Le dossier `dir` porte un manifeste lisible du hash `hash` (sans
    /// vérifier les fichiers) : c'est une version à nous.
    pub fn has_manifest(&self, hash: &str, dir: &Path) -> bool {
        valid_hash(hash)
            && !is_link(dir)
            && read_inside(dir, &[MANIFEST_FILE])
                .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
                .is_some_and(|m| m.hash == hash)
    }

    /// Arrêt brutal pendant un remplacement : `<hash>` absent mais
    /// `<hash>.old` présent → remis en place.
    fn restore_old(&self, hash: &str) {
        let fin = self.version_dir(hash);
        let old = self.root.join(format!("{hash}{OLD_SUFFIX}"));
        if std::fs::symlink_metadata(&fin).is_err() && self.has_manifest(hash, &old) {
            let _ = rename_retry(&old, &fin);
        }
    }

    /// `active.json` ; absent ou illisible : aucun (interface embarquée).
    pub fn read_pointer(&self) -> Pointer {
        std::fs::read(self.root.join(ACTIVE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Pointer>(&b).ok())
            .map(|p| Pointer {
                active: p.active.filter(|h| valid_hash(h)),
                previous: p.previous.filter(|h| valid_hash(h)),
            })
            .unwrap_or_default()
    }

    pub fn write_pointer(&self, p: &Pointer) -> io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        write_atomic(
            &self.root.join(ACTIVE_FILE),
            &serde_json::to_vec_pretty(p).map_err(io::Error::other)?,
        )
    }

    /// `state.json` ; absent ou illisible : vide.
    pub fn read_state(&self) -> UiState {
        std::fs::read(self.root.join(STATE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn write_state(&self, s: &UiState) -> io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        write_atomic(
            &self.root.join(STATE_FILE),
            &serde_json::to_vec_pretty(s).map_err(io::Error::other)?,
        )
    }

    /// Marque `hash` fautive dans `state.json`.
    pub fn mark_bad(&self, hash: &str) -> io::Result<()> {
        let mut s = self.read_state();
        if s.bad.insert(hash.to_string()) {
            self.write_state(&s)?;
        }
        Ok(())
    }

    /// Un échec de plus au démarrage pour `hash` ; renvoie le total
    /// consécutif.
    pub fn record_startup_failure(&self, hash: &str) -> io::Result<u32> {
        let mut s = self.read_state();
        let n = s.startup_failures.entry(hash.to_string()).or_insert(0);
        *n += 1;
        let n = *n;
        self.write_state(&s)?;
        Ok(n)
    }

    /// « Prêt » reçu au démarrage : compteur d'échecs de `hash` remis à 0.
    pub fn reset_startup_failures(&self, hash: &str) -> io::Result<()> {
        let mut s = self.read_state();
        if s.startup_failures.remove(hash).is_some() {
            self.write_state(&s)?;
        }
        Ok(())
    }

    /// Vérifie la version `hash` (ou un dossier `dir` qui la contient) :
    /// dossier présent et pas un lien, manifeste lisible et de ce hash,
    /// chemins propres, `index.html` présent, chaque fichier présent avec
    /// la bonne empreinte. Ne regarde pas `state.json`.
    pub fn verify_dir(&self, hash: &str, dir: &Path) -> Result<Version, Invalid> {
        if !valid_hash(hash) {
            return Err(Invalid::BadHash);
        }
        let meta = std::fs::symlink_metadata(dir).map_err(|_| Invalid::Missing)?;
        if meta.file_type().is_symlink() {
            return Err(Invalid::Link);
        }
        if !meta.is_dir() {
            return Err(Invalid::Missing);
        }
        let raw =
            read_inside(dir, &[MANIFEST_FILE]).ok_or_else(|| Invalid::Manifest("absent".into()))?;
        let m: Manifest =
            serde_json::from_slice(&raw).map_err(|e| Invalid::Manifest(e.to_string()))?;
        if m.version != MANIFEST_VERSION {
            return Err(Invalid::Manifest(format!("format {}", m.version)));
        }
        if m.hash != hash {
            return Err(Invalid::Manifest(format!("hash {} ≠ {hash}", m.hash)));
        }
        if !m.files.contains_key("index.html") {
            return Err(Invalid::Manifest("index.html absent".into()));
        }
        let mut files = BTreeMap::new();
        for (path, digest) in &m.files {
            let parts = clean_path(path)
                .filter(|_| !path.starts_with('/'))
                .ok_or_else(|| Invalid::Manifest(format!("chemin refusé : {path}")))?;
            if path == MANIFEST_FILE {
                return Err(Invalid::Manifest("se liste lui-même".into()));
            }
            let want = parse_hex32(digest)
                .ok_or_else(|| Invalid::Manifest(format!("empreinte illisible : {path}")))?;
            let bytes = read_inside(dir, &parts).ok_or_else(|| Invalid::File(path.clone()))?;
            if sha256(&bytes) != want {
                return Err(Invalid::File(path.clone()));
            }
            files.insert(path.clone(), want);
        }
        Ok(Version {
            hash: hash.to_string(),
            dir: dir.to_path_buf(),
            files,
        })
    }

    /// Version installée `hash`, vérifiée, non fautive.
    pub fn load_version(&self, hash: &str) -> Result<Version, Invalid> {
        if !valid_hash(hash) {
            return Err(Invalid::BadHash);
        }
        if self.read_state().bad.contains(hash) {
            return Err(Invalid::MarkedBad);
        }
        self.restore_old(hash);
        self.verify_dir(hash, &self.version_dir(hash))
    }

    /// Dernière activation valide et non fautive avant `current` (la
    /// version à laquelle un retour arrière mènerait).
    pub fn previous_of(&self, state: &UiState, current: Option<&str>) -> Option<String> {
        state
            .activations
            .iter()
            .rev()
            .filter(|h| Some(h.as_str()) != current && !state.bad.contains(*h))
            .find(|h| {
                self.restore_old(h);
                self.verify_dir(h, &self.version_dir(h)).is_ok()
            })
            .cloned()
    }

    /// Enregistre l'activation de `hash` (`state.json`, puis le pointeur
    /// en dernier). Ne vérifie pas la version.
    pub fn record_activation(&self, hash: &str) -> io::Result<Pointer> {
        let mut s = self.read_state();
        s.activations.retain(|h| h != hash);
        s.activations.push(hash.to_string());
        let excess = s.activations.len().saturating_sub(MAX_ACTIVATIONS);
        s.activations.drain(..excess);
        self.write_state(&s)?;
        let p = Pointer {
            active: Some(hash.to_string()),
            previous: self.previous_of(&s, Some(hash)),
        };
        self.write_pointer(&p)?;
        Ok(p)
    }

    /// Quitte la version `current` (retirée des activations, marquée
    /// fautive si `bad`) pour la précédente valide, ou l'embarquée.
    /// `state.json` puis le pointeur. Renvoie le nouveau pointeur.
    pub fn step_back(&self, current: Option<&str>, bad: bool) -> io::Result<Pointer> {
        let mut s = self.read_state();
        if let Some(cur) = current {
            if bad {
                s.bad.insert(cur.to_string());
                s.startup_failures.remove(cur);
            }
            s.activations.retain(|h| h != cur);
        }
        let target = self.previous_of(&s, current);
        self.write_state(&s)?;
        let p = Pointer {
            previous: target
                .as_deref()
                .and_then(|t| self.previous_of(&s, Some(t)))
                .filter(|p| Some(p.as_str()) != current),
            active: target,
        };
        self.write_pointer(&p)?;
        Ok(p)
    }

    /// Installe le build `dir` comme version `hash` (sans l'activer) :
    /// copie dans `<hash>.tmp` (sauf si `dir` est déjà ce dossier),
    /// manifeste SHA-256, vérification, renommage en `<hash>`, puis ajout à
    /// `state.json`. Le pointeur n'est jamais touché.
    pub fn install_with(
        &self,
        hash: &str,
        dir: &Path,
        hook: &mut dyn FnMut(InstallStage) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let invalid = |m: String| io::Error::new(io::ErrorKind::InvalidInput, m);
        if !valid_hash(hash) {
            return Err(invalid(format!("hash invalide : {hash:?}")));
        }
        std::fs::create_dir_all(&self.root)?;
        let tmp = self.root.join(format!("{hash}{TMP_SUFFIX}"));
        let in_place = match (dir.canonicalize(), tmp.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !in_place {
            if !dir.is_dir() {
                return Err(invalid(format!("build absent : {}", dir.display())));
            }
            if tmp.exists() {
                std::fs::remove_dir_all(&tmp)?;
            }
            copy_tree(dir, &tmp)?;
        } else if is_link(&tmp) {
            return Err(invalid("lien refusé".into()));
        }
        hook(InstallStage::Copied)?;

        let mut files = BTreeMap::new();
        for path in list_files(&tmp)? {
            let parts: Vec<&str> = path.split('/').collect();
            let bytes = read_inside(&tmp, &parts).ok_or_else(|| invalid(path.clone()))?;
            files.insert(path, hex(&sha256(&bytes)));
        }
        if !files.contains_key("index.html") {
            return Err(invalid("index.html absent du build".into()));
        }
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            hash: hash.to_string(),
            files,
        };
        write_atomic(
            &tmp.join(MANIFEST_FILE),
            &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?,
        )?;
        hook(InstallStage::ManifestWritten)?;

        self.verify_dir(hash, &tmp)
            .map_err(|e| invalid(format!("vérification : {e}")))?;
        hook(InstallStage::Verified)?;

        // Remplacement : l'ancienne version est mise de côté (`.old`) puis
        // supprimée après le renommage ; un arrêt brutal entre les deux la
        // laisse récupérable (`restore_old`, nettoyage).
        let fin = self.version_dir(hash);
        let old = self.root.join(format!("{hash}{OLD_SUFFIX}"));
        let replaced = match std::fs::symlink_metadata(&fin) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::remove_file(&fin).or_else(|_| std::fs::remove_dir(&fin))?;
                false
            }
            Ok(_) => {
                if let Ok(m) = std::fs::symlink_metadata(&old) {
                    remove_entry(&old, &m);
                }
                rename_retry(&fin, &old)?;
                true
            }
            Err(_) => false,
        };
        rename_retry(&tmp, &fin)?;
        hook(InstallStage::Renamed)?;
        if replaced {
            if let Ok(m) = std::fs::symlink_metadata(&old) {
                remove_entry(&old, &m);
            }
        }

        let mut s = self.read_state();
        s.installs.retain(|h| h != hash);
        s.installs.push(hash.to_string());
        self.write_state(&s)?;
        Ok(fin)
    }

    /// [`Store::cleanup_at`] à l'instant présent.
    pub fn cleanup(&self, keep: &[&str], keep_recent: usize) -> usize {
        self.cleanup_at(keep, keep_recent, SystemTime::now())
    }

    /// Supprime les versions hors de `keep` (active, précédente) et des
    /// `keep_recent` dernières installations non fautives ; les
    /// `<hash>.tmp` abandonnés (sans verrou `<hash>.lock` et sans écriture
    /// depuis [`STALE_AFTER`] à la date `now`) ; les `.part` de même âge ;
    /// les `<hash>.old` dont la version est en place (sinon `.old` est
    /// remis en place). Seul un dossier `<hash>` avec un manifeste de ce
    /// hash est une version : tout autre nom est laissé. Renvoie le nombre
    /// de dossiers supprimés.
    pub fn cleanup_at(&self, keep: &[&str], keep_recent: usize, now: SystemTime) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut state = self.read_state();
        let mut versions: Vec<(String, SystemTime)> = Vec::new();
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_file() {
                if name.ends_with(PART_SUFFIX)
                    && stale(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), now)
                {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            if let Some(base) = name.strip_suffix(TMP_SUFFIX) {
                let locked = self.root.join(format!("{base}{LOCK_SUFFIX}")).exists();
                if valid_hash(base)
                    && !locked
                    && stale(tree_mtime(&path), now)
                    && remove_entry(&path, &meta)
                {
                    removed += 1;
                }
                continue;
            }
            if let Some(base) = name.strip_suffix(OLD_SUFFIX) {
                if self.has_manifest(base, &path) {
                    if self.version_dir(base).exists() {
                        if remove_entry(&path, &meta) {
                            removed += 1;
                        }
                    } else {
                        self.restore_old(base);
                    }
                }
                continue;
            }
            if self.has_manifest(&name, &path) {
                let t = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                versions.push((name, t));
            }
        }
        // Les plus récentes d'abord : ordre de `installs`, puis date.
        let rank = |h: &str| state.installs.iter().position(|x| x == h);
        versions.sort_by(|(a, ta), (b, tb)| match (rank(a), rank(b)) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => tb.cmp(ta),
        });
        let mut recent = 0;
        let mut gone = Vec::new();
        for (h, _) in &versions {
            if keep.contains(&h.as_str()) {
                continue;
            }
            if !state.bad.contains(h) && recent < keep_recent {
                recent += 1;
                continue;
            }
            let path = self.version_dir(h);
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if remove_entry(&path, &meta) {
                    removed += 1;
                    gone.push(h.clone());
                }
            }
        }
        if !gone.is_empty() {
            state.installs.retain(|h| !gone.contains(h));
            state.activations.retain(|h| !gone.contains(h));
            let _ = self.write_state(&state);
        }
        removed
    }
}

fn remove_entry(path: &Path, meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        // Lien ou jonction : on retire le lien, jamais sa cible.
        std::fs::remove_dir(path)
            .or_else(|_| std::fs::remove_file(path))
            .is_ok()
    } else {
        std::fs::remove_dir_all(path).is_ok()
    }
}
