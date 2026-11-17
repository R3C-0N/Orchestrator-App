//! J14, envoi réservé au propriétaire : la sonde et l'envoi dans un
//! environnement hostile, comme si une session avait préparé le terrain
//! avec le compte de l'utilisateur. Binaire de test à part : tout le
//! processus hérite de cet environnement (`GIT_CONFIG_COUNT` avec un
//! `pushInsteadOf` vers un dépôt nu « pirate », `GIT_SSH_COMMAND` vers un
//! faux ssh « pirate », `GIT_DIR` vers le pirate, `GIT_EXEC_PATH`,
//! `GIT_INDEX_FILE`, `GIT_TRACE`, `GIT_SSL_NO_VERIFY`, `GIT_TEMPLATE_DIR`),
//! et `empty.gitconfig` de l'app est déjà rempli avant l'appel. Dépôts nus
//! locaux, joints par un faux ssh ; aucun accès réseau.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use orchestrator_lib::selfupdate::push::{self, PushEnv};

/// Dossier temporaire du test, supprimé au drop.
struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `git -C dir args` hors de l'environnement hostile (identité fixée).
fn sh(dir: &Path, args: &[&str]) -> String {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args);
    for k in [
        "GIT_CONFIG_COUNT",
        "GIT_SSH_COMMAND",
        "GIT_TRACE",
        "GIT_TEMPLATE_DIR",
        "GIT_DIR",
        "GIT_EXEC_PATH",
        "GIT_INDEX_FILE",
    ] {
        c.env_remove(k);
    }
    let out = c.output().expect("git introuvable");
    assert!(
        out.status.success(),
        "git {args:?} : {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

const ROUTER: &str = r#"#!/bin/sh
# Faux ssh de l'app : `[options…] <hôte> <commande>` ; hôte `allow-…` =
# `git receive-pack` local.
[ "$1" = "-G" ] && exit 1
all="$*"
while [ $# -gt 2 ]; do shift; done
dir="${0%/*}"
printf '%s\n' "$all" >> "$dir/router.log"
unset GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_DIR
eval "git ${2#git-}"
"#;

const PIRATE: &str = r#"#!/bin/sh
# Faux ssh « pirate » hérité : ne doit jamais servir.
dir="${0%/*}"
printf '%s\n' "$*" >> "$dir/pirate-ssh.log"
exit 1
"#;

#[tokio::test]
async fn hostile_inherited_environment_does_not_redirect_the_push() {
    let pid = std::process::id();
    let root = TempRoot(std::env::temp_dir().join(format!("orch-c-push-env-{pid}")));
    let _ = std::fs::remove_dir_all(&root.0);
    std::fs::create_dir_all(&root.0).unwrap();
    let r = &root.0;

    // Dépôts : seed -> origin nu, pirate nu, clone.
    let seed = r.join("seed");
    std::fs::create_dir_all(seed.join("src")).unwrap();
    sh(&seed, &["init", "-q", "-b", "main"]);
    std::fs::write(seed.join("src/a.txt"), "a\n").unwrap();
    sh(&seed, &["add", "--", "src"]);
    sh(&seed, &["commit", "-q", "-m", "c1"]);
    let origin = r.join("origin.git");
    let pirate = r.join("pirate.git");
    sh(
        r,
        &["clone", "-q", "--bare", &slash(&seed), &slash(&origin)],
    );
    sh(r, &["init", "-q", "--bare", &slash(&pirate)]);
    let clone = r.join("clone");
    sh(r, &["clone", "-q", &slash(&origin), &slash(&clone)]);
    let url = format!("git@allow-env-{pid}:{}", slash(&origin));
    sh(&clone, &["remote", "set-url", "origin", &url]);
    sh(
        &clone,
        &["remote", "set-url", "--push", "origin", "no-push://x"],
    );
    let head = sh(&clone, &["rev-parse", "main"]);

    // Faux ssh de l'app (routeur) et faux ssh pirate hérité.
    let bin = r.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("router.sh"), ROUTER).unwrap();
    std::fs::write(bin.join("pirate.sh"), PIRATE).unwrap();

    // Environnement hostile hérité par tout le processus.
    let pirate_url = format!("git@allow-env-{pid}:{}", slash(&pirate));
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var(
        "GIT_CONFIG_KEY_0",
        format!("url.{pirate_url}.pushInsteadOf"),
    );
    std::env::set_var("GIT_CONFIG_VALUE_0", &url);
    std::env::set_var("GIT_SSH_COMMAND", slash(&bin.join("pirate.sh")));
    std::env::set_var("GIT_TRACE", slash(&r.join("trace.log")));
    std::env::set_var("GIT_SSL_NO_VERIFY", "1");
    std::env::set_var("GIT_TEMPLATE_DIR", slash(&r.join("modeles")));
    std::env::set_var("GIT_DIR", slash(&pirate));
    std::env::set_var("GIT_EXEC_PATH", slash(&r.join("exec-pirate")));
    std::env::set_var("GIT_INDEX_FILE", slash(&r.join("index-pirate")));

    // `empty.gitconfig` de l'app déjà rempli (redirection vers le pirate).
    let env_dir = r.join("push-env");
    std::fs::create_dir_all(&env_dir).unwrap();
    std::fs::write(
        env_dir.join("empty.gitconfig"),
        format!("[url \"{pirate_url}\"]\n\tpushInsteadOf = {url}\n"),
    )
    .unwrap();
    std::fs::write(
        env_dir.join("empty.ssh_config"),
        "Host *\n  ProxyCommand calc.exe\n",
    )
    .unwrap();

    let env = PushEnv {
        dir: env_dir.clone(),
        git_program: push::resolve_program(if cfg!(windows) { "git.exe" } else { "git" })
            .map(|p| slash(&p))
            .unwrap_or_else(|| "git".into()),
        ssh_program: slash(&bin.join("router.sh")),
        host: format!("allow-env-{pid}"),
        identities: Vec::new(),
    };
    let t = Duration::from_secs(60);
    push::check_write_access(&clone, &url, &url, &env, t)
        .await
        .expect("sonde");
    push::push_branch(&clone, &url, &url, &env, "auto/env-test", &head, t)
        .await
        .expect("envoi");

    assert_eq!(
        sh(&origin, &["rev-parse", "refs/heads/auto/env-test"]),
        head,
        "envoi au bon dépôt"
    );
    assert_eq!(
        sh(&pirate, &["for-each-ref", "--format=%(refname)"]),
        "",
        "pirate vide"
    );
    assert!(
        !bin.join("pirate-ssh.log").exists(),
        "ssh pirate jamais lancé"
    );
    assert!(!r.join("trace.log").exists(), "GIT_TRACE retiré");
    let routed = std::fs::read_to_string(bin.join("router.log")).unwrap();
    assert_eq!(routed.lines().count(), 2, "{routed}");
    assert_eq!(
        std::fs::read(env_dir.join("empty.gitconfig")).unwrap(),
        b"",
        "fichier vidé avant usage"
    );
    assert_eq!(
        std::fs::read(env_dir.join("empty.ssh_config")).unwrap(),
        b""
    );
}

/// Commande ssh de l'envoi passée par un shell puis par le vrai `ssh -G`
/// (aucune connexion) : une clé dont le chemin contient une espace reste
/// une seule valeur.
#[test]
fn identity_with_space_survives_shell_and_real_ssh() {
    let ssh = push::resolve_program(if cfg!(windows) { "ssh.exe" } else { "ssh" });
    let shell = push::resolve_program(if cfg!(windows) { "sh.exe" } else { "sh" });
    let (Some(ssh), Some(shell)) = (ssh, shell) else {
        eprintln!("ssh ou sh absent du PATH : test sauté");
        return;
    };
    let pid = std::process::id();
    let root = TempRoot(std::env::temp_dir().join(format!("orch-c-ssh-space-{pid}")));
    let key = root.0.join("Jean Dupont").join("id_test");
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, "").unwrap();
    let empty = root.0.join("empty.ssh_config");
    std::fs::write(&empty, "").unwrap();
    let env = PushEnv {
        dir: root.0.clone(),
        git_program: "git".into(),
        ssh_program: slash(&ssh),
        host: "github.com".into(),
        identities: vec![key.clone()],
    };
    let cmd = format!("{} -G github.com", env.ssh_command(&empty));
    let out = Command::new(shell).arg("-c").arg(&cmd).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{cmd}\n{err}");
    assert!(
        text.contains(&format!("identityfile {}", slash(&key).to_lowercase())),
        "{cmd}\n{text}\n{err}"
    );
    assert!(text.contains("hostname github.com"), "{text}");
    assert!(text.contains("stricthostkeychecking true"), "{text}");
}
