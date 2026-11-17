//! J14, étape 2 : hébergement de l'interface (`ui::UiHost`), sur des
//! dossiers temporaires (jamais le vrai `%LOCALAPPDATA%`). Critères du plan
//! J14 (7) et (8) : refus de `..`, `%2e%2e` et d'un chemin absolu, repli
//! embarqué si le dossier manque ou si le manifeste est faux, installation
//! atomique, watchdog, au plus 7 dossiers après 8 installations.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use orchestrator_lib::api::types::UiSource;
use orchestrator_lib::ui::store::{
    clean_path, valid_hash, InstallStage, Pointer, MANIFEST_FILE, STALE_AFTER,
};
use orchestrator_lib::ui::{ActiveUi, UiHost, KEEP_RECENT};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Wry};

/// Préfixe des dossiers temporaires de ces tests (`%TEMP%`).
const TEMP_PREFIX: &str = "orch-j14-ui-";

/// Dossier temporaire propre à un test, supprimé à la fin (même en cas
/// d'échec : `Drop` passe pendant le déroulement de la panique).
///
/// Sous Windows, `remove_dir_all` échoue parfois juste après les écritures
/// (« répertoire non vide », os error 145 ; fichier tenu par l'antivirus ou
/// l'indexeur, suppression en attente) : la suppression est réessayée
/// pendant 5 s au plus, et un échec final est signalé sur stderr au lieu
/// d'être ignoré en silence. Un minuteur du watchdog (fil à part) peut aussi
/// réécrire `state.json` après la fin d'un test qui a échoué : nouvelle
/// suppression après 1 s dans ce cas, et les dossiers d'exécutions
/// précédentes sont balayés par [`temp_root`].
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let mut res = remove_dir_retry(&self.0, Duration::from_secs(5));
        if std::thread::panicking() {
            // Test en échec : une chaîne de repli du watchdog peut encore
            // écrire ; on la laisse finir puis on supprime à nouveau.
            std::thread::sleep(Duration::from_secs(1));
            res = remove_dir_retry(&self.0, Duration::from_secs(5));
        }
        if let Err(e) = res {
            eprintln!("dossier de test non supprimé {} : {e}", self.0.display());
        }
    }
}

/// `remove_dir_all` réessayé jusqu'à `budget` ; `Ok` si le dossier n'existe
/// plus.
fn remove_dir_retry(d: &Path, budget: Duration) -> std::io::Result<()> {
    let end = Instant::now() + budget;
    loop {
        match fs::remove_dir_all(d) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if Instant::now() >= end => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Supprime les dossiers `orch-j14-ui-*` laissés par une exécution
/// précédente (autre processus, dossier modifié il y a plus de 10 min).
fn sweep_stale_temp_roots() {
    let me = format!("-{}", std::process::id());
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(TEMP_PREFIX) || name.ends_with(&me) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(600));
        if old {
            let _ = remove_dir_retry(&e.path(), Duration::from_secs(1));
        }
    }
}

fn temp_root(name: &str) -> TempDir {
    static SWEEP: std::sync::Once = std::sync::Once::new();
    SWEEP.call_once(sweep_stale_temp_roots);
    let d = std::env::temp_dir().join(format!("{TEMP_PREFIX}{name}-{}", std::process::id()));
    let _ = remove_dir_retry(&d, Duration::from_secs(5));
    fs::create_dir_all(&d).unwrap();
    TempDir(d)
}

/// Une date au-delà du délai d'abandon des `.tmp` et `.part`.
fn later() -> SystemTime {
    SystemTime::now() + STALE_AFTER + Duration::from_secs(60)
}

/// Assets embarqués simulés : `EMBARQUÉ:<clé>` pour toute clé.
struct Embedded;

impl Assets<Wry> for Embedded {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        Some(Cow::Owned(
            format!("EMBARQUÉ:{}", key.as_ref()).into_bytes(),
        ))
    }
    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }
    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

fn provider(host: &UiHost) -> Box<dyn Assets<Wry>> {
    host.provider::<Wry>(Box::new(Embedded))
}

fn get(a: &dyn Assets<Wry>, key: &str) -> Option<String> {
    a.get(&AssetKey::from(key))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Écrit un build simulé `v<hash>` dans `<base>\build-<hash>`.
fn build(base: &Path, hash: &str) -> PathBuf {
    let d = base.join(format!("build-{hash}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("assets")).unwrap();
    fs::write(d.join("index.html"), format!("<html>{hash}</html>")).unwrap();
    fs::write(d.join("assets/app.js"), format!("console.log('{hash}')")).unwrap();
    d
}

/// `root` pour les versions, `work` pour les builds, hors de `root`.
fn setup(name: &str) -> (TempDir, PathBuf, PathBuf, UiHost) {
    let base = temp_root(name);
    let root = base.0.join("ui");
    let work = base.0.join("work");
    fs::create_dir_all(&work).unwrap();
    (base, root.clone(), work, UiHost::new(root))
}

fn install(host: &UiHost, work: &Path, hash: &str) -> PathBuf {
    host.install(hash, &build(work, hash)).unwrap()
}

fn dirs(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < limit {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

// --- Chemins -------------------------------------------------------------

#[test]
fn clean_path_refuse_les_chemins_dangereux() {
    for bad in [
        "",
        "/",
        "..",
        "/../secret.txt",
        "a/../b",
        "./index.html",
        "%2e%2e/secret.txt",
        "%2e%2e%2fsecret.txt",
        "..%2fsecret.txt",
        "C:/Windows/win.ini",
        "C:\\Windows\\win.ini",
        "/C:/x",
        "\\\\serveur\\partage\\x",
        "//serveur/partage",
        "assets\\app.js",
        "a//b",
        "index.html:flux",
        "a\0b",
        // Point ou espace final : Windows les retire en silence.
        "index.html.",
        "index.html ",
        "assets./app.js",
        "assets /app.js",
        // Noms réservés, avec ou sans extension, toute casse.
        "CON",
        "con.txt",
        "Nul.js",
        "assets/aux.tar.gz",
        "prn",
        "COM1.js",
        "com0",
        "LPT9",
        "lpt3.html",
    ] {
        assert!(clean_path(bad).is_none(), "accepté à tort : {bad:?}");
    }
    assert_eq!(clean_path("/index.html").unwrap(), vec!["index.html"]);
    assert_eq!(
        clean_path("assets/app-1.js").unwrap(),
        vec!["assets", "app-1.js"]
    );
    for ok in [
        "console.js",
        "auxiliary.js",
        "com10.js",
        "nul_x.css",
        ".well-known/a",
    ] {
        assert!(clean_path(ok).is_some(), "refusé à tort : {ok:?}");
    }
}

#[test]
fn valid_hash_hexa_minuscule_de_longueur_bornee() {
    for ok in ["abc1234", "0123456789abcdef", &"a".repeat(64)] {
        assert!(valid_hash(ok), "{ok:?}");
    }
    for bad in [
        "",
        "abc123",
        "ABC1234",
        "Abc1234",
        "logs",
        "cache",
        "abcdefg",
        "con1234",
        "abc1234.tmp",
        &"a".repeat(65),
    ] {
        assert!(!valid_hash(bad), "{bad:?}");
    }
}

#[test]
fn le_fournisseur_sert_la_version_active_et_refuse_de_sortir_du_dossier() {
    let (_g, root, work, host) = setup("serve");
    install(&host, &work, "abc1234");
    host.switch_to("abc1234").unwrap();
    // Fichiers voisins qu'une traversée pourrait atteindre.
    fs::write(root.join("secret.txt"), "SECRET").unwrap();
    fs::write(root.join("abc1234").join("extra.txt"), "HORS MANIFESTE").unwrap();
    let secret_abs = root.join("secret.txt");

    let a = provider(&host);
    assert_eq!(host.active().source, UiSource::Disk);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1234</html>");
    assert_eq!(
        get(&*a, "/assets/app.js").unwrap(),
        "console.log('abc1234')"
    );
    for key in [
        "../secret.txt",
        "assets/../../secret.txt",
        "%2e%2e/secret.txt",
        "%2e%2e%2fsecret.txt",
        "..\\secret.txt",
        secret_abs.to_str().unwrap(),
        "extra.txt",
        MANIFEST_FILE,
        "absent.js",
    ] {
        // Refus net : ni le fichier visé, ni l'interface embarquée (pas de
        // repli fichier par fichier).
        assert_eq!(get(&*a, key), None, "servi à tort : {key:?}");
    }
    // `iter` ne liste que les fichiers du manifeste.
    let mut keys: Vec<String> = a.iter().map(|(k, _)| k.into_owned()).collect();
    keys.sort();
    assert_eq!(keys, vec!["/assets/app.js", "/index.html"]);
}

#[test]
fn un_fichier_modifie_apres_installation_n_est_pas_servi() {
    let (_g, root, work, host) = setup("tamper");
    install(&host, &work, "abc1234");
    host.switch_to("abc1234").unwrap();
    let a = provider(&host);
    fs::write(root.join("abc1234/assets/app.js"), "alert(1)").unwrap();
    assert_eq!(get(&*a, "assets/app.js"), None);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1234</html>");
}

// --- Repli en bloc sur l'interface embarquée -----------------------------

fn embedded_after(name: &str, damage: impl FnOnce(&Path)) {
    let (_g, root, work, host) = setup(name);
    install(&host, &work, "abc1234");
    host.switch_to("abc1234").unwrap();
    damage(&root);
    let fresh = UiHost::new(root.clone());
    let a = provider(&fresh);
    assert_eq!(
        fresh.active(),
        ActiveUi {
            source: UiSource::Embedded,
            hash: None,
            previous: None
        },
        "{name}"
    );
    // Tout vient de l'embarqué, y compris un fichier encore sur disque.
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
    assert_eq!(
        get(&*a, "assets/app.js").unwrap(),
        "EMBARQUÉ:/assets/app.js"
    );
}

#[test]
fn repli_embarque_si_dossier_absent() {
    embedded_after("absent", |root| {
        fs::remove_dir_all(root.join("abc1234")).unwrap()
    });
}

#[test]
fn repli_embarque_si_manifeste_corrompu() {
    embedded_after("corrupt", |root| {
        fs::write(root.join("abc1234").join(MANIFEST_FILE), "{ pas du json").unwrap()
    });
}

#[test]
fn repli_embarque_si_manifeste_d_un_autre_hash() {
    embedded_after("otherhash", |root| {
        let p = root.join("abc1234").join(MANIFEST_FILE);
        let s = fs::read_to_string(&p)
            .unwrap()
            .replace("\"abc1234\"", "\"def2345\"");
        fs::write(p, s).unwrap();
    });
}

#[test]
fn repli_embarque_si_manifeste_avec_traversee() {
    embedded_after("manifest-dotdot", |root| {
        let p = root.join("abc1234").join(MANIFEST_FILE);
        let s = fs::read_to_string(&p)
            .unwrap()
            .replace("\"assets/app.js\"", "\"../secret.txt\"");
        fs::write(p, s).unwrap();
    });
}

#[test]
fn repli_embarque_si_un_fichier_manque_ou_change() {
    embedded_after("file-missing", |root| {
        fs::remove_file(root.join("abc1234/assets/app.js")).unwrap()
    });
    embedded_after("file-changed", |root| {
        fs::write(root.join("abc1234/index.html"), "<html>autre</html>").unwrap()
    });
}

#[test]
fn repli_embarque_si_version_marquee_fautive() {
    embedded_after("bad", |root| {
        UiHost::new(root.to_path_buf())
            .store()
            .mark_bad("abc1234")
            .unwrap()
    });
}

#[test]
fn repli_embarque_si_pointeur_illisible_ou_absent() {
    embedded_after("pointer", |root| {
        fs::write(root.join("active.json"), "nul").unwrap()
    });
    let (_g, _root, _work, host) = setup("empty");
    let a = provider(&host);
    assert_eq!(host.active().source, UiSource::Embedded);
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
}

#[test]
fn au_demarrage_la_version_active_est_relue() {
    let (_g, root, work, host) = setup("restart");
    install(&host, &work, "abc1234");
    install(&host, &work, "def2345");
    host.switch_to("abc1234").unwrap();
    host.switch_to("def2345").unwrap();
    let fresh = UiHost::new(root);
    let a = provider(&fresh);
    assert_eq!(
        fresh.active(),
        ActiveUi {
            source: UiSource::Disk,
            hash: Some("def2345".into()),
            previous: Some("abc1234".into())
        }
    );
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>def2345</html>");
}

// --- Installation atomique -----------------------------------------------

#[test]
fn installation_interrompue_laisse_le_pointeur_inchange() {
    for stage in [
        InstallStage::Copied,
        InstallStage::ManifestWritten,
        InstallStage::Verified,
        InstallStage::Renamed,
    ] {
        let (_g, root, work, host) = setup(&format!("crash-{stage:?}"));
        install(&host, &work, "abc1234");
        host.switch_to("abc1234").unwrap();
        let before = host.store().read_pointer();
        let src = build(&work, "def2345");
        let r = host.install_with("def2345", &src, &mut |s| {
            if s == stage {
                Err(std::io::Error::other("arrêt brutal simulé"))
            } else {
                Ok(())
            }
        });
        assert!(r.is_err(), "{stage:?}");
        // Pointeur et version servie inchangés, y compris après relance.
        assert_eq!(host.store().read_pointer(), before, "{stage:?}");
        let fresh = UiHost::new(root.clone());
        let a = provider(&fresh);
        assert_eq!(fresh.active().hash.as_deref(), Some("abc1234"), "{stage:?}");
        assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1234</html>");
        if stage != InstallStage::Renamed {
            assert!(!root.join("def2345").exists(), "{stage:?}");
            assert!(root.join("def2345.tmp").exists(), "{stage:?}");
            // Une version inachevée ne s'active pas.
            assert!(fresh.switch_to("def2345").is_err(), "{stage:?}");
            // Le nettoyage supprime le `.tmp` abandonné.
            assert_eq!(fresh.cleanup_at(later()), 1, "{stage:?}");
            assert!(!root.join("def2345.tmp").exists(), "{stage:?}");
        }
        // Une nouvelle installation complète réussit.
        fresh.install("def2345", &src).unwrap();
        fresh.switch_to("def2345").unwrap();
        assert_eq!(
            fresh.store().read_pointer().active.as_deref(),
            Some("def2345")
        );
    }
}

#[test]
fn installation_en_place_depuis_le_tmp() {
    let (_g, root, _work, host) = setup("inplace");
    let tmp = root.join("abc1234.tmp");
    fs::create_dir_all(&root).unwrap();
    fs::rename(build(&root, "abc1234"), &tmp).unwrap();
    let fin = host.install("abc1234", &tmp).unwrap();
    assert_eq!(fin, root.join("abc1234"));
    assert!(!tmp.exists());
    assert!(fin.join(MANIFEST_FILE).is_file());
    // Installer n'active pas.
    assert_eq!(host.store().read_pointer(), Pointer::default());
}

#[test]
fn installation_refusee_sans_index_ou_avec_hash_invalide() {
    let (_g, _root, work, host) = setup("refuse");
    let d = work.join("vide");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("app.js"), "x").unwrap();
    assert!(host.install("abc1234", &d).is_err());
    let ok = build(&work, "abc1234");
    for h in ["", "..", "a/b", "abc1234.tmp", "C:x"] {
        assert!(host.install(h, &ok).is_err(), "{h:?}");
    }
}

// --- Watchdog -------------------------------------------------------------

type Seen = Arc<Mutex<Vec<ActiveUi>>>;

fn recorder() -> (Seen, orchestrator_lib::ui::OnFallback) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    (
        seen,
        Arc::new(move |ui: &ActiveUi| s.lock().unwrap().push(ui.clone())),
    )
}

#[test]
fn watchdog_sans_pret_marque_fautive_et_revient_a_la_precedente_puis_a_l_embarquee() {
    let (_g, _root, work, host) = setup("watchdog");
    install(&host, &work, "abc1234");
    install(&host, &work, "def2345");
    host.switch_to("abc1234").unwrap();
    host.switch_to("def2345").unwrap();
    let a = provider(&host);
    let (seen, cb) = recorder();
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(Some("def2345".into()), t, cb);
    // Un « prêt » d'une autre version ne désarme pas.
    assert!(!host.ready(Some("abc1234")));
    assert!(!host.ready(None));

    assert!(wait_until(Duration::from_secs(5), || seen
        .lock()
        .unwrap()
        .len()
        >= 2));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].hash.as_deref(), Some("abc1234"));
    assert_eq!(seen[1].source, UiSource::Embedded);
    let state = host.store().read_state();
    assert!(state.bad.contains("def2345") && state.bad.contains("abc1234"));
    assert_eq!(host.store().read_pointer().active, None);
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
    // Pas de boucle : rien n'est plus attendu, les versions fautives ne
    // se réactivent pas.
    std::thread::sleep(t * 3);
    assert!(host.switch_to("def2345").is_err());
    assert!(host.switch_to("abc1234").is_err());
}

#[test]
fn watchdog_pret_de_la_precedente_arrete_la_chaine() {
    let (_g, _root, work, host) = setup("watchdog-prev");
    let host = Arc::new(host);
    install(&host, &work, "abc1234");
    install(&host, &work, "def2345");
    host.switch_to("abc1234").unwrap();
    host.switch_to("def2345").unwrap();
    let seen: Seen = Arc::default();
    let (s, h) = (seen.clone(), host.clone());
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(
        Some("def2345".into()),
        t,
        Arc::new(move |ui: &ActiveUi| {
            s.lock().unwrap().push(ui.clone());
            // La page rechargée (version précédente) répond.
            assert!(h.ready(ui.hash.as_deref()));
        }),
    );
    assert!(wait_until(Duration::from_secs(5), || !seen
        .lock()
        .unwrap()
        .is_empty()));
    std::thread::sleep(t * 3);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(host.active().hash.as_deref(), Some("abc1234"));
    let state = host.store().read_state();
    assert!(state.bad.contains("def2345") && !state.bad.contains("abc1234"));
}

#[test]
fn watchdog_pret_a_temps_ne_change_rien() {
    let (_g, _root, work, host) = setup("watchdog-ok");
    install(&host, &work, "abc1234");
    host.switch_to("abc1234").unwrap();
    let (seen, cb) = recorder();
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(Some("abc1234".into()), t, cb);
    assert!(host.ready(Some("abc1234")));
    assert!(!host.ready(Some("abc1234")), "un seul « prêt » attendu");
    std::thread::sleep(t * 3);
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(host.active().hash.as_deref(), Some("abc1234"));
    assert!(host.store().read_state().bad.is_empty());
}

#[test]
fn watchdog_rearme_remplace_l_ancien_minuteur() {
    let (_g, _root, work, host) = setup("watchdog-rearm");
    install(&host, &work, "abc1234");
    install(&host, &work, "def2345");
    host.switch_to("abc1234").unwrap();
    let (seen, cb) = recorder();
    host.arm_watchdog_with(
        Some("abc1234".into()),
        Duration::from_millis(150),
        cb.clone(),
    );
    // Bascule avant l'échéance : le minuteur de abc1234 est périmé.
    host.switch_to("def2345").unwrap();
    host.arm_watchdog_with(Some("def2345".into()), Duration::from_secs(60), cb);
    std::thread::sleep(Duration::from_millis(450));
    assert!(seen.lock().unwrap().is_empty());
    assert!(host.store().read_state().bad.is_empty());
    assert!(host.ready(Some("def2345")));
}

// --- Retour arrière --------------------------------------------------------

#[test]
fn retour_arriere_vers_la_precedente_puis_l_embarquee() {
    let (_g, _root, work, host) = setup("rollback");
    install(&host, &work, "abc1234");
    install(&host, &work, "def2345");
    host.switch_to("abc1234").unwrap();
    let ui = host.switch_to("def2345").unwrap();
    assert_eq!(ui.previous.as_deref(), Some("abc1234"));
    let ui = host.step_back().unwrap();
    assert_eq!(ui.hash.as_deref(), Some("abc1234"));
    assert_eq!(ui.previous, None);
    // Le retour arrière ne marque pas la version fautive.
    assert!(host.store().read_state().bad.is_empty());
    let ui = host.step_back().unwrap();
    assert_eq!(ui.source, UiSource::Embedded);
    assert_eq!(host.store().read_pointer().active, None);
}

#[test]
fn retour_arriere_saute_une_precedente_invalide() {
    let (_g, root, work, host) = setup("rollback-skip");
    for h in ["aaa1111", "bbb2222", "ccc3333"] {
        install(&host, &work, h);
        host.switch_to(h).unwrap();
    }
    fs::remove_dir_all(root.join("bbb2222")).unwrap();
    let ui = host.step_back().unwrap();
    assert_eq!(ui.hash.as_deref(), Some("aaa1111"));
}

// --- Nettoyage ----------------------------------------------------------

#[test]
fn nettoyage_apres_huit_installations_au_plus_sept_dossiers() {
    let (_g, root, work, host) = setup("cleanup");
    let hashes: Vec<String> = (1..=8).map(|i| format!("{i:02}aa000")).collect();
    for h in &hashes {
        install(&host, &work, h);
        host.switch_to(h).unwrap();
    }
    // Active : la plus ancienne ; précédente : la plus récente.
    host.switch_to("01aa000").unwrap();
    fs::create_dir_all(root.join("09aa000.tmp")).unwrap();
    // Un dossier inconnu n'est pas touché (et ne compte pas comme version).
    fs::create_dir_all(root.join("notes-perso")).unwrap();

    let removed = host.cleanup_at(later());
    let left = dirs(&root);
    let versions: Vec<&String> = left.iter().filter(|d| *d != "notes-perso").collect();
    assert!(versions.len() <= 2 + KEEP_RECENT, "{left:?}");
    assert_eq!(versions.len(), 7, "{left:?}");
    assert_eq!(removed, 2, "{left:?}"); // 02aa000 et 09aa000.tmp
    assert!(left.contains(&"01aa000".to_string()) && left.contains(&"08aa000".to_string()));
    assert!(!left.contains(&"02aa000".to_string()) && !left.contains(&"09aa000.tmp".to_string()));
    assert!(left.contains(&"notes-perso".to_string()));
    // L'active est toujours servie.
    let a = provider(&host);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>01aa000</html>");
}

#[test]
fn nettoyage_supprime_les_versions_fautives_hors_active() {
    let (_g, root, work, host) = setup("cleanup-bad");
    for h in ["aaa1111", "bbb2222", "ccc3333"] {
        install(&host, &work, h);
    }
    host.switch_to("aaa1111").unwrap();
    host.store().mark_bad("bbb2222").unwrap();
    assert_eq!(host.cleanup(), 1);
    assert_eq!(dirs(&root), vec!["aaa1111", "ccc3333"]);
}

#[test]
fn nettoyage_respecte_le_verrou_de_build_et_l_age_du_tmp() {
    let (_g, root, work, host) = setup("cleanup-lock");
    install(&host, &work, "aaa1111");
    // Build en cours : verrou tenu, `.tmp` en train d'être écrit.
    let lock = host.build_lock("bbb2222").unwrap();
    assert!(host.build_lock("bbb2222").is_err(), "verrou exclusif");
    let tmp = root.join("bbb2222.tmp");
    fs::create_dir_all(tmp.join("assets")).unwrap();
    fs::write(tmp.join("assets/app.js"), "x").unwrap();
    // `.tmp` récent sans verrou : laissé aussi.
    fs::create_dir_all(root.join("ccc3333.tmp")).unwrap();

    assert_eq!(host.cleanup(), 0);
    assert!(tmp.exists() && root.join("ccc3333.tmp").exists());
    // Même vieux, un `.tmp` verrouillé n'est pas touché ; l'autre part.
    assert_eq!(host.cleanup_at(later()), 1);
    assert!(tmp.exists() && !root.join("ccc3333.tmp").exists());
    // Verrou rendu : le `.tmp` abandonné part au nettoyage suivant.
    drop(lock);
    assert!(!root.join("bbb2222.lock").exists());
    assert_eq!(host.cleanup_at(later()), 1);
    assert!(!tmp.exists());
    assert!(root.join("aaa1111").exists());
    assert!(host.build_lock("../x").is_err());
}

#[test]
fn nettoyage_ne_touche_que_nos_dossiers() {
    let (_g, root, work, host) = setup("cleanup-foreign");
    let hashes: Vec<String> = (1..=8).map(|i| format!("{i:07x}")).collect();
    for h in &hashes {
        install(&host, &work, h);
    }
    // Dossiers étrangers : nom ordinaire, hash sans manifeste, manifeste
    // d'un autre hash, casse non conforme.
    for d in ["logs", "cache", "deadbee", "ABCDEF0"] {
        fs::create_dir_all(root.join(d)).unwrap();
        fs::write(root.join(d).join("x.txt"), "à moi").unwrap();
    }
    fs::create_dir_all(root.join("cafe123")).unwrap();
    fs::copy(
        root.join("0000001").join(MANIFEST_FILE),
        root.join("cafe123").join(MANIFEST_FILE),
    )
    .unwrap();
    // Aucune active : 5 récentes gardées, 3 supprimées, étrangers intacts.
    assert_eq!(host.cleanup_at(later()), 3);
    let left = dirs(&root);
    for d in ["logs", "cache", "deadbee", "ABCDEF0", "cafe123"] {
        assert!(left.contains(&d.to_string()), "{d} supprimé : {left:?}");
    }
    for h in &hashes[..3] {
        assert!(!left.contains(h), "{left:?}");
    }
}

#[test]
fn nettoyage_supprime_les_part_orphelins_anciens() {
    let (_g, root, work, host) = setup("cleanup-part");
    install(&host, &work, "aaa1111");
    let part = root.join("state.json.999-0.part");
    fs::write(&part, "{").unwrap();
    host.cleanup();
    assert!(part.exists(), "récent : gardé");
    host.cleanup_at(later());
    assert!(!part.exists(), "ancien : supprimé");
    assert!(root.join("state.json").exists());
}

// --- Remplacement d'une version existante ----------------------------------

#[test]
fn remplacement_interrompu_garde_l_ancienne_version() {
    let (_g, root, work, host) = setup("replace");
    install(&host, &work, "aaa1111");
    install(&host, &work, "bbb2222");
    host.switch_to("aaa1111").unwrap();
    // Arrêt brutal pendant un remplacement de bbb2222 : l'ancienne est de
    // côté (`.old`), la nouvelle n'est pas encore en place.
    fs::rename(root.join("bbb2222"), root.join("bbb2222.old")).unwrap();
    // Elle est remise en place à la lecture.
    let ui = host.switch_to("bbb2222").unwrap();
    assert_eq!(ui.hash.as_deref(), Some("bbb2222"));
    assert!(!root.join("bbb2222.old").exists());

    // Réinstallation d'une version non affichée : remplacée proprement.
    install(&host, &work, "ccc3333");
    install(&host, &work, "ccc3333");
    assert!(!root.join("ccc3333.old").exists());
    // Un `.old` resté à côté d'une version en place part au nettoyage.
    fs::create_dir_all(root.join("ddd4444.old")).unwrap();
    copy_dir(&root.join("ccc3333"), &root.join("ccc3333.old"));
    host.cleanup();
    assert!(!root.join("ccc3333.old").exists());
    assert!(root.join("ccc3333").exists());
    // Un `.old` sans manifeste à nous n'est pas touché.
    assert!(root.join("ddd4444.old").exists());
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

#[test]
fn reinstallation_refusee_pour_l_active_et_la_precedente() {
    let (_g, _root, work, host) = setup("reinstall");
    install(&host, &work, "aaa1111");
    install(&host, &work, "bbb2222");
    host.switch_to("aaa1111").unwrap();
    host.switch_to("bbb2222").unwrap();
    for h in ["aaa1111", "bbb2222"] {
        assert!(host.install(h, &build(&work, h)).is_err(), "{h}");
    }
}

// --- Watchdog de démarrage ---------------------------------------------------

#[test]
fn demarrage_premier_echec_embarquee_pour_cette_execution_seulement() {
    let (_g, root, work, host) = setup("startup");
    install(&host, &work, "aaa1111");
    install(&host, &work, "bbb2222");
    host.switch_to("aaa1111").unwrap();
    host.switch_to("bbb2222").unwrap();
    let t = Duration::from_millis(100);

    // Lancement 1 : pas de « prêt ».
    let run1 = UiHost::new(root.clone());
    let a = provider(&run1);
    let (seen, cb) = recorder();
    run1.arm_startup_watchdog_with(run1.active().hash, t, cb);
    assert!(wait_until(Duration::from_secs(5), || !seen
        .lock()
        .unwrap()
        .is_empty()));
    std::thread::sleep(t * 3);
    assert_eq!(seen.lock().unwrap().len(), 1, "pas de réarmement");
    assert_eq!(run1.active().source, UiSource::Embedded);
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
    let st = run1.store().read_state();
    assert!(st.bad.is_empty(), "pas fautive au premier échec");
    assert_eq!(st.startup_failures.get("bbb2222"), Some(&1));
    assert_eq!(
        run1.store().read_pointer().active.as_deref(),
        Some("bbb2222")
    );
    // Lancement 2 : la version est retentée ; deuxième échec consécutif →
    // fautive et retour à la précédente.
    let run2 = UiHost::new(root.clone());
    provider(&run2);
    assert_eq!(run2.active().hash.as_deref(), Some("bbb2222"));
    let (seen, cb) = recorder();
    run2.arm_startup_watchdog_with(run2.active().hash, t, cb);
    assert!(wait_until(Duration::from_secs(5), || !seen
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(seen.lock().unwrap()[0].hash.as_deref(), Some("aaa1111"));
    assert!(run2.store().read_state().bad.contains("bbb2222"));
    assert_eq!(
        run2.store().read_pointer().active.as_deref(),
        Some("aaa1111")
    );
    // La précédente a son propre compteur : premier échec → embarquée.
    assert!(wait_until(Duration::from_secs(5), || seen
        .lock()
        .unwrap()
        .len()
        >= 2));
    assert_eq!(seen.lock().unwrap()[1].source, UiSource::Embedded);
    assert!(!run2.store().read_state().bad.contains("aaa1111"));
}

#[test]
fn demarrage_watchdog_arme_au_premier_index_html() {
    let (_g, root, work, host) = setup("startup-first-load");
    install(&host, &work, "aaa1111");
    host.switch_to("aaa1111").unwrap();
    let run = UiHost::new(root);
    let (_seen, cb) = recorder();
    let a = run.provider_with_reload::<Wry>(Box::new(Embedded), cb);
    // Rien d'armé avant le chargement de la page, ni par un autre fichier.
    assert_eq!(run.watchdog_expected(), None);
    get(&*a, "assets/app.js");
    assert_eq!(run.watchdog_expected(), None);
    get(&*a, "index.html");
    assert_eq!(run.watchdog_expected().as_deref(), Some("aaa1111"));
    assert!(run.ready(Some("aaa1111")));
    // Un rechargement ultérieur ne réarme pas le watchdog de démarrage.
    get(&*a, "index.html");
    assert_eq!(run.watchdog_expected(), None);
}

#[test]
fn demarrage_pret_remet_le_compteur_a_zero() {
    let (_g, root, work, host) = setup("startup-reset");
    install(&host, &work, "aaa1111");
    host.switch_to("aaa1111").unwrap();
    let t = Duration::from_millis(100);
    for run in 0..3 {
        let h = UiHost::new(root.clone());
        provider(&h);
        let (seen, cb) = recorder();
        h.arm_startup_watchdog_with(h.active().hash, t, cb);
        if run == 1 {
            // Un lancement réussi entre deux échecs.
            assert!(h.ready(Some("aaa1111")));
            std::thread::sleep(t * 3);
            assert!(seen.lock().unwrap().is_empty());
        } else {
            assert!(wait_until(Duration::from_secs(5), || !seen
                .lock()
                .unwrap()
                .is_empty()));
        }
    }
    let st = host.store().read_state();
    assert!(st.bad.is_empty(), "jamais deux échecs consécutifs");
    assert_eq!(st.startup_failures.get("aaa1111"), Some(&1));
}

#[test]
fn retour_arriere_depuis_le_repli_de_demarrage() {
    let (_g, root, work, host) = setup("startup-rollback");
    install(&host, &work, "aaa1111");
    install(&host, &work, "bbb2222");
    host.switch_to("aaa1111").unwrap();
    host.switch_to("bbb2222").unwrap();
    let run = UiHost::new(root.clone());
    provider(&run);
    let (seen, cb) = recorder();
    run.arm_startup_watchdog_with(run.active().hash, Duration::from_millis(50), cb);
    assert!(wait_until(Duration::from_secs(5), || !seen
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(run.active().source, UiSource::Embedded);
    // Depuis l'embarquée : la version précédente de celle qui a échoué.
    let ui = run.step_back().unwrap();
    assert_eq!(ui.hash.as_deref(), Some("aaa1111"));
    // Et sans version précédente, on reste sur l'embarquée.
    let ui = run.step_back().unwrap();
    assert_eq!(ui.source, UiSource::Embedded);
    let ui = run.step_back().unwrap();
    assert_eq!(ui.source, UiSource::Embedded);
}

#[test]
fn course_entre_pret_et_echeance_un_seul_gagnant() {
    let (_g, root, work, host) = setup("race");
    install(&host, &work, "bbb2222");
    let host = Arc::new(host);
    let (mut ready_won, mut expire_won) = (0, 0);
    for i in 0..60 {
        // Remise en état : bbb2222 seule active (pas de précédente, donc
        // pas de chaîne de repli), rien de fautif.
        let _ = fs::remove_file(root.join("state.json"));
        let _ = fs::remove_file(root.join("active.json"));
        host.switch_to("bbb2222").unwrap();
        let (seen, cb) = recorder();
        let t = Duration::from_millis(i % 3);
        let h = host.clone();
        let racer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1));
            h.ready(Some("bbb2222"))
        });
        host.arm_watchdog_with(Some("bbb2222".into()), t, cb);
        let ready = racer.join().unwrap();
        if ready {
            // Le minuteur, périmé, ne doit rien faire.
            std::thread::sleep(t + Duration::from_millis(30));
        } else {
            assert!(
                wait_until(Duration::from_secs(5), || !seen.lock().unwrap().is_empty()),
                "tour {i} : ni prêt ni repli"
            );
        }
        let fell_back = !seen.lock().unwrap().is_empty();
        let bad = host.store().read_state().bad.contains("bbb2222");
        assert!(
            ready != fell_back,
            "tour {i} : prêt={ready} repli={fell_back}"
        );
        assert_eq!(bad, fell_back, "tour {i}");
        if ready {
            ready_won += 1;
            assert_eq!(host.active().hash.as_deref(), Some("bbb2222"));
        } else {
            expire_won += 1;
            assert_eq!(host.active().source, UiSource::Embedded);
        }
    }
    eprintln!("prêt gagnant : {ready_won}, échéance gagnante : {expire_won}");
}
