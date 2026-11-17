//! J14, étape 2 : hébergement de l'interface (`ui::UiHost`), sur des
//! dossiers temporaires (jamais le vrai `%LOCALAPPDATA%`). Critères du plan
//! J14 (7) et (8) : refus de `..`, `%2e%2e` et d'un chemin absolu, repli
//! embarqué si le dossier manque ou si le manifeste est faux, installation
//! atomique, watchdog, au plus 7 dossiers après 8 installations.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orchestrator_lib::api::types::UiSource;
use orchestrator_lib::ui::store::{clean_path, InstallStage, Pointer, MANIFEST_FILE};
use orchestrator_lib::ui::{ActiveUi, UiHost, KEEP_RECENT};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Wry};

/// Dossier temporaire propre à un test.
fn temp_root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("orch-j14-ui-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
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
fn setup(name: &str) -> (PathBuf, PathBuf, UiHost) {
    let base = temp_root(name);
    let root = base.join("ui");
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    (root.clone(), work, UiHost::new(root))
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
    ] {
        assert!(clean_path(bad).is_none(), "accepté à tort : {bad:?}");
    }
    assert_eq!(clean_path("/index.html").unwrap(), vec!["index.html"]);
    assert_eq!(
        clean_path("assets/app-1.js").unwrap(),
        vec!["assets", "app-1.js"]
    );
}

#[test]
fn le_fournisseur_sert_la_version_active_et_refuse_de_sortir_du_dossier() {
    let (root, work, host) = setup("serve");
    install(&host, &work, "abc1");
    host.switch_to("abc1").unwrap();
    // Fichiers voisins qu'une traversée pourrait atteindre.
    fs::write(root.join("secret.txt"), "SECRET").unwrap();
    fs::write(root.join("abc1").join("extra.txt"), "HORS MANIFESTE").unwrap();
    let secret_abs = root.join("secret.txt");

    let a = provider(&host);
    assert_eq!(host.active().source, UiSource::Disk);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1</html>");
    assert_eq!(get(&*a, "/assets/app.js").unwrap(), "console.log('abc1')");
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
    let (root, work, host) = setup("tamper");
    install(&host, &work, "abc1");
    host.switch_to("abc1").unwrap();
    let a = provider(&host);
    fs::write(root.join("abc1/assets/app.js"), "alert(1)").unwrap();
    assert_eq!(get(&*a, "assets/app.js"), None);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1</html>");
}

// --- Repli en bloc sur l'interface embarquée -----------------------------

fn embedded_after(name: &str, damage: impl FnOnce(&Path)) {
    let (root, work, host) = setup(name);
    install(&host, &work, "abc1");
    host.switch_to("abc1").unwrap();
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
        fs::remove_dir_all(root.join("abc1")).unwrap()
    });
}

#[test]
fn repli_embarque_si_manifeste_corrompu() {
    embedded_after("corrupt", |root| {
        fs::write(root.join("abc1").join(MANIFEST_FILE), "{ pas du json").unwrap()
    });
}

#[test]
fn repli_embarque_si_manifeste_d_un_autre_hash() {
    embedded_after("otherhash", |root| {
        let p = root.join("abc1").join(MANIFEST_FILE);
        let s = fs::read_to_string(&p)
            .unwrap()
            .replace("\"abc1\"", "\"def2\"");
        fs::write(p, s).unwrap();
    });
}

#[test]
fn repli_embarque_si_manifeste_avec_traversee() {
    embedded_after("manifest-dotdot", |root| {
        let p = root.join("abc1").join(MANIFEST_FILE);
        let s = fs::read_to_string(&p)
            .unwrap()
            .replace("\"assets/app.js\"", "\"../secret.txt\"");
        fs::write(p, s).unwrap();
    });
}

#[test]
fn repli_embarque_si_un_fichier_manque_ou_change() {
    embedded_after("file-missing", |root| {
        fs::remove_file(root.join("abc1/assets/app.js")).unwrap()
    });
    embedded_after("file-changed", |root| {
        fs::write(root.join("abc1/index.html"), "<html>autre</html>").unwrap()
    });
}

#[test]
fn repli_embarque_si_version_marquee_fautive() {
    embedded_after("bad", |root| {
        UiHost::new(root.to_path_buf())
            .store()
            .mark_bad("abc1")
            .unwrap()
    });
}

#[test]
fn repli_embarque_si_pointeur_illisible_ou_absent() {
    embedded_after("pointer", |root| {
        fs::write(root.join("active.json"), "nul").unwrap()
    });
    let (_root, _work, host) = setup("empty");
    let a = provider(&host);
    assert_eq!(host.active().source, UiSource::Embedded);
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
}

#[test]
fn au_demarrage_la_version_active_est_relue() {
    let (root, work, host) = setup("restart");
    install(&host, &work, "abc1");
    install(&host, &work, "def2");
    host.switch_to("abc1").unwrap();
    host.switch_to("def2").unwrap();
    let fresh = UiHost::new(root);
    let a = provider(&fresh);
    assert_eq!(
        fresh.active(),
        ActiveUi {
            source: UiSource::Disk,
            hash: Some("def2".into()),
            previous: Some("abc1".into())
        }
    );
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>def2</html>");
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
        let (root, work, host) = setup(&format!("crash-{stage:?}"));
        install(&host, &work, "abc1");
        host.switch_to("abc1").unwrap();
        let before = host.store().read_pointer();
        let src = build(&work, "def2");
        let r = host.install_with("def2", &src, &mut |s| {
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
        assert_eq!(fresh.active().hash.as_deref(), Some("abc1"), "{stage:?}");
        assert_eq!(get(&*a, "index.html").unwrap(), "<html>abc1</html>");
        if stage != InstallStage::Renamed {
            assert!(!root.join("def2").exists(), "{stage:?}");
            assert!(root.join("def2.tmp").exists(), "{stage:?}");
            // Une version inachevée ne s'active pas.
            assert!(fresh.switch_to("def2").is_err(), "{stage:?}");
            // Le nettoyage supprime le `.tmp` abandonné.
            assert_eq!(fresh.cleanup(), 1, "{stage:?}");
            assert!(!root.join("def2.tmp").exists(), "{stage:?}");
        }
        // Une nouvelle installation complète réussit.
        fresh.install("def2", &src).unwrap();
        fresh.switch_to("def2").unwrap();
        assert_eq!(fresh.store().read_pointer().active.as_deref(), Some("def2"));
    }
}

#[test]
fn installation_en_place_depuis_le_tmp() {
    let (root, _work, host) = setup("inplace");
    let tmp = root.join("abc1.tmp");
    fs::create_dir_all(&root).unwrap();
    fs::rename(build(&root, "abc1"), &tmp).unwrap();
    let fin = host.install("abc1", &tmp).unwrap();
    assert_eq!(fin, root.join("abc1"));
    assert!(!tmp.exists());
    assert!(fin.join(MANIFEST_FILE).is_file());
    // Installer n'active pas.
    assert_eq!(host.store().read_pointer(), Pointer::default());
}

#[test]
fn installation_refusee_sans_index_ou_avec_hash_invalide() {
    let (_root, work, host) = setup("refuse");
    let d = work.join("vide");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("app.js"), "x").unwrap();
    assert!(host.install("abc1", &d).is_err());
    let ok = build(&work, "abc1");
    for h in ["", "..", "a/b", "abc1.tmp", "C:x"] {
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
    let (_root, work, host) = setup("watchdog");
    install(&host, &work, "abc1");
    install(&host, &work, "def2");
    host.switch_to("abc1").unwrap();
    host.switch_to("def2").unwrap();
    let a = provider(&host);
    let (seen, cb) = recorder();
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(Some("def2".into()), t, cb);
    // Un « prêt » d'une autre version ne désarme pas.
    assert!(!host.ready(Some("abc1")));
    assert!(!host.ready(None));

    assert!(wait_until(Duration::from_secs(5), || seen
        .lock()
        .unwrap()
        .len()
        >= 2));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].hash.as_deref(), Some("abc1"));
    assert_eq!(seen[1].source, UiSource::Embedded);
    let state = host.store().read_state();
    assert!(state.bad.contains("def2") && state.bad.contains("abc1"));
    assert_eq!(host.store().read_pointer().active, None);
    assert_eq!(get(&*a, "index.html").unwrap(), "EMBARQUÉ:/index.html");
    // Pas de boucle : rien n'est plus attendu, les versions fautives ne
    // se réactivent pas.
    std::thread::sleep(t * 3);
    assert!(host.switch_to("def2").is_err());
    assert!(host.switch_to("abc1").is_err());
}

#[test]
fn watchdog_pret_de_la_precedente_arrete_la_chaine() {
    let (_root, work, host) = setup("watchdog-prev");
    let host = Arc::new(host);
    install(&host, &work, "abc1");
    install(&host, &work, "def2");
    host.switch_to("abc1").unwrap();
    host.switch_to("def2").unwrap();
    let seen: Seen = Arc::default();
    let (s, h) = (seen.clone(), host.clone());
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(
        Some("def2".into()),
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
    assert_eq!(host.active().hash.as_deref(), Some("abc1"));
    let state = host.store().read_state();
    assert!(state.bad.contains("def2") && !state.bad.contains("abc1"));
}

#[test]
fn watchdog_pret_a_temps_ne_change_rien() {
    let (_root, work, host) = setup("watchdog-ok");
    install(&host, &work, "abc1");
    host.switch_to("abc1").unwrap();
    let (seen, cb) = recorder();
    let t = Duration::from_millis(150);
    host.arm_watchdog_with(Some("abc1".into()), t, cb);
    assert!(host.ready(Some("abc1")));
    assert!(!host.ready(Some("abc1")), "un seul « prêt » attendu");
    std::thread::sleep(t * 3);
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(host.active().hash.as_deref(), Some("abc1"));
    assert!(host.store().read_state().bad.is_empty());
}

#[test]
fn watchdog_rearme_remplace_l_ancien_minuteur() {
    let (_root, work, host) = setup("watchdog-rearm");
    install(&host, &work, "abc1");
    install(&host, &work, "def2");
    host.switch_to("abc1").unwrap();
    let (seen, cb) = recorder();
    host.arm_watchdog_with(Some("abc1".into()), Duration::from_millis(150), cb.clone());
    // Bascule avant l'échéance : le minuteur de abc1 est périmé.
    host.switch_to("def2").unwrap();
    host.arm_watchdog_with(Some("def2".into()), Duration::from_secs(60), cb);
    std::thread::sleep(Duration::from_millis(450));
    assert!(seen.lock().unwrap().is_empty());
    assert!(host.store().read_state().bad.is_empty());
    assert!(host.ready(Some("def2")));
}

// --- Retour arrière --------------------------------------------------------

#[test]
fn retour_arriere_vers_la_precedente_puis_l_embarquee() {
    let (_root, work, host) = setup("rollback");
    install(&host, &work, "abc1");
    install(&host, &work, "def2");
    host.switch_to("abc1").unwrap();
    let ui = host.switch_to("def2").unwrap();
    assert_eq!(ui.previous.as_deref(), Some("abc1"));
    let ui = host.step_back().unwrap();
    assert_eq!(ui.hash.as_deref(), Some("abc1"));
    assert_eq!(ui.previous, None);
    // Le retour arrière ne marque pas la version fautive.
    assert!(host.store().read_state().bad.is_empty());
    let ui = host.step_back().unwrap();
    assert_eq!(ui.source, UiSource::Embedded);
    assert_eq!(host.store().read_pointer().active, None);
}

#[test]
fn retour_arriere_saute_une_precedente_invalide() {
    let (root, work, host) = setup("rollback-skip");
    for h in ["aaa1", "bbb2", "ccc3"] {
        install(&host, &work, h);
        host.switch_to(h).unwrap();
    }
    fs::remove_dir_all(root.join("bbb2")).unwrap();
    let ui = host.step_back().unwrap();
    assert_eq!(ui.hash.as_deref(), Some("aaa1"));
}

// --- Nettoyage ----------------------------------------------------------

#[test]
fn nettoyage_apres_huit_installations_au_plus_sept_dossiers() {
    let (root, work, host) = setup("cleanup");
    let hashes: Vec<String> = (1..=8).map(|i| format!("{i:02}aa")).collect();
    for h in &hashes {
        install(&host, &work, h);
        host.switch_to(h).unwrap();
    }
    // Active : la plus ancienne ; précédente : la plus récente.
    host.switch_to("01aa").unwrap();
    fs::create_dir_all(root.join("09aa.tmp")).unwrap();
    // Un dossier inconnu n'est pas touché (et ne compte pas comme version).
    fs::create_dir_all(root.join("notes-perso")).unwrap();

    let removed = host.cleanup();
    let left = dirs(&root);
    let versions: Vec<&String> = left.iter().filter(|d| *d != "notes-perso").collect();
    assert!(versions.len() <= 2 + KEEP_RECENT, "{left:?}");
    assert_eq!(versions.len(), 7, "{left:?}");
    assert_eq!(removed, 2, "{left:?}"); // 02aa et 09aa.tmp
    assert!(left.contains(&"01aa".to_string()) && left.contains(&"08aa".to_string()));
    assert!(!left.contains(&"02aa".to_string()) && !left.contains(&"09aa.tmp".to_string()));
    assert!(left.contains(&"notes-perso".to_string()));
    // L'active est toujours servie.
    let a = provider(&host);
    assert_eq!(get(&*a, "index.html").unwrap(), "<html>01aa</html>");
}

#[test]
fn nettoyage_supprime_les_versions_fautives_hors_active() {
    let (root, work, host) = setup("cleanup-bad");
    for h in ["aaa1", "bbb2", "ccc3"] {
        install(&host, &work, h);
    }
    host.switch_to("aaa1").unwrap();
    host.store().mark_bad("bbb2").unwrap();
    assert_eq!(host.cleanup(), 1);
    assert_eq!(dirs(&root), vec!["aaa1", "ccc3"]);
}
