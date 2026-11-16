//! J11 : projets récents et liste des sessions (plan 1.8, décision 11), sur
//! les extraits synthétiques de `fixtures/transcripts/projects/` et sur des
//! dossiers temporaires (dates de modification maîtrisées).

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use orchestrator_lib::sessions::list::{list_projects, list_sessions, MAX_SESSIONS};
use orchestrator_lib::sessions::paths::encode_project_dir;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/transcripts/projects")
}

/// Dossier temporaire propre à un test.
fn temp_root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("orch-j11-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn set_mtime(p: &Path, secs_ago: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs_ago);
    File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_times(FileTimes::new().set_modified(t))
        .unwrap();
}

fn uuid_n(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn line_user(cwd: &str, text: &str) -> String {
    serde_json::json!({
        "type": "user", "isSidechain": false, "cwd": cwd, "entrypoint": "cli",
        "message": {"role": "user", "content": text}, "timestamp": "2026-11-16T09:00:00Z"
    })
    .to_string()
}

#[test]
fn projects_from_fixtures_use_the_real_cwd() {
    let p = list_projects(&fixtures());
    // `C--vide` n'a pas de transcript : absent.
    assert_eq!(p.len(), 2, "{p:?}");
    let demo = p
        .iter()
        .find(|x| x.encoded == "C--Users--user--Code-demo")
        .unwrap();
    assert_eq!(demo.path.as_deref(), Some(r"C:\Users\<user>\Code\demo"));
    assert_eq!(demo.session_count, 4);
    assert!(!demo.exists);
    // Nom ambigu (`D:\disparu\a-b` ou `D:\disparu\a\b`) : `cwd` tranche.
    let gone = p.iter().find(|x| x.encoded == "D--disparu-a-b").unwrap();
    assert_eq!(gone.path.as_deref(), Some(r"D:\disparu\a-b"));
    assert!(!gone.exists, "dossier disparu : grisé");
}

#[test]
fn projects_are_sorted_by_latest_transcript_and_existing_dirs_detected() {
    let root = temp_root("projects");
    let real = temp_root("projects-real-cwd");
    let real_s = real.display().to_string();
    let mk = |cwd: &str, n: usize, ago: u64| {
        let d = root.join(encode_project_dir(cwd));
        fs::create_dir_all(&d).unwrap();
        let f = d.join(format!("{}.jsonl", uuid_n(n)));
        fs::write(&f, line_user(cwd, "bonjour") + "\n").unwrap();
        set_mtime(&f, ago);
    };
    mk(r"D:\ancien", 1, 3000);
    mk(&real_s, 2, 100);
    mk(&real_s, 3, 5000);
    mk(r"D:\moyen", 4, 1000);
    // Fichier qui n'est pas un transcript de session : ignoré.
    fs::write(
        root.join(encode_project_dir(r"D:\moyen"))
            .join("notes.jsonl"),
        "x",
    )
    .unwrap();

    let p = list_projects(&root);
    let order: Vec<_> = p.iter().map(|x| x.path.clone().unwrap()).collect();
    assert_eq!(
        order,
        vec![real_s.clone(), r"D:\moyen".into(), r"D:\ancien".into()]
    );
    assert!(p[0].exists);
    assert_eq!(p[0].session_count, 2);
    assert_eq!(p[1].session_count, 1);
    assert!(p[0].last_used > p[1].last_used);
    // Racine absente : liste vide, pas d'erreur.
    assert!(list_projects(&root.join("absent")).is_empty());
}

#[test]
fn sessions_from_fixtures_have_titles_and_prompts() {
    let s = list_sessions(&fixtures(), r"C:\Users\<user>\Code\demo");
    assert_eq!(s.len(), 4);
    let get = |suffix: &str| s.iter().find(|x| x.id.ends_with(suffix)).unwrap();
    let full = get("0001");
    assert_eq!(full.title.as_deref(), Some("Résumé du README de démo"));
    assert_eq!(full.last_prompt.as_deref(), Some("Encore une chose"));
    assert_eq!(full.entrypoint.as_deref(), Some("cli"));
    // Session `-p` : pas d'`ai-title`, le dernier prompt sert de titre.
    let sdk = get("0004");
    assert_eq!(sdk.title, None);
    assert_eq!(sdk.last_prompt.as_deref(), Some("Réponds juste OK."));
    assert_eq!(sdk.entrypoint.as_deref(), Some("sdk-cli"));
    // Tronquée : ni titre ni `last-prompt` à la fin, repli sur le premier prompt.
    let tr = get("0002");
    assert_eq!(
        tr.last_prompt.as_deref(),
        Some("Lis le fichier README et résume-le.")
    );
    // Illisible : listée quand même, sans titre.
    let bad = get("0003");
    assert!(bad.title.is_none() && bad.last_prompt.is_none());
    assert!(bad.size_bytes > 0);
    // Casse différente de la lettre de lecteur : même dossier.
    assert_eq!(
        list_sessions(&fixtures(), r"c:\Users\<user>\Code\demo").len(),
        4
    );
    // Projet inconnu : liste vide.
    assert!(list_sessions(&fixtures(), r"Z:\jamais\ouvert").is_empty());
}

#[test]
fn sessions_list_200_of_250_sorted_under_300ms() {
    let root = temp_root("sessions-perf");
    let cwd = r"D:\perf\projet";
    let dir = root.join(encode_project_dir(cwd));
    fs::create_dir_all(&dir).unwrap();
    // ~100 ko par transcript : le titre est à la fin, après des lignes
    // volumineuses.
    let filler = serde_json::json!({
        "type": "attachment", "attachment": {"type": "x", "text": "a".repeat(4000)}
    })
    .to_string();
    for n in 0..250 {
        let mut body = String::new();
        body.push_str(&line_user(cwd, &format!("prompt {n}")));
        body.push('\n');
        for _ in 0..25 {
            body.push_str(&filler);
            body.push('\n');
        }
        // Comme dans le CLI : une ligne de conversation (avec `entrypoint`)
        // juste avant les métadonnées de fin.
        body.push_str(&line_user(cwd, &format!("suite {n}")));
        body.push('\n');
        body.push_str(&format!(
            "{{\"type\":\"ai-title\",\"aiTitle\":\"Titre {n}\"}}\n{{\"type\":\"last-prompt\",\"lastPrompt\":\"dernier {n}\"}}\n"
        ));
        let f = dir.join(format!("{}.jsonl", uuid_n(n)));
        fs::write(&f, body).unwrap();
        set_mtime(&f, 10 * (n as u64 + 1));
    }
    let t0 = Instant::now();
    let s = list_sessions(&root, cwd);
    let ms = t0.elapsed().as_millis();
    eprintln!("list_sessions : {} sessions en {ms} ms", s.len());
    assert_eq!(s.len(), MAX_SESSIONS);
    // Tri par date décroissante : n = 0 est la plus récente.
    assert_eq!(s[0].title.as_deref(), Some("Titre 0"));
    assert_eq!(s[0].last_prompt.as_deref(), Some("dernier 0"));
    assert_eq!(s[199].title.as_deref(), Some("Titre 199"));
    assert!(s.windows(2).all(|w| w[0].modified >= w[1].modified));
    assert!(ms < 300, "{ms} ms pour 200 sessions");
    let _ = fs::remove_dir_all(&root);
}

/// Mesure sur les vrais transcripts de la machine (lecture seule, n'affiche
/// que des comptes et des durées) : `cargo test --test sessions_list --
/// --ignored --nocapture`.
#[test]
#[ignore]
fn local_projects_and_sessions_timing() {
    let Some(root) = orchestrator_lib::sessions::paths::projects_root() else {
        return;
    };
    let t0 = Instant::now();
    let projects = list_projects(&root);
    eprintln!(
        "list_projects : {} projets en {} ms",
        projects.len(),
        t0.elapsed().as_millis()
    );
    let mut worst = (0u128, 0usize);
    for p in projects.iter().filter(|p| p.path.is_some()) {
        let t0 = Instant::now();
        let s = list_sessions(&root, p.path.as_deref().unwrap());
        let ms = t0.elapsed().as_millis();
        if ms > worst.0 {
            worst = (ms, s.len());
        }
        assert!(s.len() <= MAX_SESSIONS);
    }
    eprintln!(
        "list_sessions : pire cas {} ms pour {} sessions",
        worst.0, worst.1
    );
}
