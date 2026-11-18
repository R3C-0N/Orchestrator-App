//! Résolution des outils de l'auto-amélioration : git, Node, npm, cargo
//! (J14, décision 19). Le nom nu (`Command::new("node")`) ne suffit pas sous
//! Windows : PATH hérité périmé, Node derrière le lien de nvm pour Windows
//! (`C:\nvm4w\nodejs`, que Windows peut refuser de traverser : « point de
//! montage non approuvé »), cargo hors du PATH de l'app. Chaque outil est donc
//! cherché, dans l'ordre : réglage `node_path` / `cargo_path` de
//! `config.json`, PATH du processus, PATH utilisateur puis machine du
//! registre (`%VAR%` développées), puis les emplacements connus (lien et
//! versions de nvm, `CARGO_HOME`, `~/.cargo/bin`, dossier de Git). Un
//! candidat n'est retenu que s'il se lance réellement (`--version`) ; sinon
//! l'erreur réelle (message Windows, code de sortie) est gardée pour la
//! raison affichée.
//!
//! Le registre, les variables d'environnement et le lancement sont injectés
//! ([`ToolEnv`]) : les tests n'ont besoin ni de Node ni de cargo.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Délai d'un `--version`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Lance un candidat avec `--version` : première ligne de sortie, ou le
/// message d'erreur réel.
pub type Probe = Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync>;

/// Environnement de la résolution (injectable).
#[derive(Clone)]
pub struct ToolEnv {
    /// Variables d'environnement, noms en MAJUSCULES.
    pub vars: HashMap<String, String>,
    /// `Path` brut (non développé) de `HKCU\Environment`.
    pub user_path: Option<String>,
    /// `Path` brut de `HKLM\…\Session Manager\Environment`.
    pub machine_path: Option<String>,
    /// `node_path` de `config.json`.
    pub node_path: Option<PathBuf>,
    /// `cargo_path` de `config.json`.
    pub cargo_path: Option<PathBuf>,
    pub probe: Probe,
}

impl fmt::Debug for ToolEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolEnv")
            .field("node_path", &self.node_path)
            .field("cargo_path", &self.cargo_path)
            .finish_non_exhaustive()
    }
}

impl ToolEnv {
    /// Environnement réel : processus, registre (lecture seule), `config.json`.
    pub fn real() -> ToolEnv {
        let vars = std::env::vars()
            .map(|(k, v)| (k.to_ascii_uppercase(), v))
            .collect();
        let (node_path, cargo_path) = overrides_from(super::config_path().as_deref());
        ToolEnv {
            vars,
            user_path: read_registry_path(r"HKCU\Environment"),
            machine_path: read_registry_path(
                r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            ),
            node_path,
            cargo_path,
            probe: Arc::new(real_probe),
        }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(&name.to_ascii_uppercase())
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Dossiers du PATH du processus, puis du registre (utilisateur, machine),
    /// sans doublon.
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let sources = [
            self.var("PATH").map(str::to_string),
            self.user_path.clone(),
            self.machine_path.clone(),
        ];
        for raw in sources.into_iter().flatten() {
            for part in raw.split(PATH_SEP) {
                let part = expand_vars(part.trim().trim_matches('"'), &self.vars);
                if part.is_empty() {
                    continue;
                }
                let p = PathBuf::from(part);
                if !out.iter().any(|q| same_path(q, &p)) {
                    out.push(p);
                }
            }
        }
        out
    }
}

#[cfg(windows)]
const PATH_SEP: char = ';';
#[cfg(not(windows))]
const PATH_SEP: char = ':';

fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    n(a) == n(b)
}

/// `%NOM%` remplacé par la variable (sans casse) ; inconnue : laissée telle
/// quelle.
pub fn expand_vars(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => {
                let name = &after[..j];
                match vars.get(&name.to_ascii_uppercase()) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Valeur `Path` d'une sortie de `reg query <clé> /v Path`.
pub fn parse_reg_query(out: &str) -> Option<String> {
    for line in out.lines() {
        let t = line.trim();
        if !t.to_ascii_lowercase().starts_with("path") {
            continue;
        }
        for ty in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(i) = t.find(ty) {
                let v = t[i + ty.len()..].trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

#[cfg(windows)]
fn read_registry_path(key: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", "Path"])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_reg_query(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

#[cfg(not(windows))]
fn read_registry_path(_key: &str) -> Option<String> {
    None
}

/// `node_path` et `cargo_path` de `config.json` (fichier, ou dossier qui
/// contient l'exécutable).
pub fn overrides_from(config_file: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
    let v = config_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let get = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k)?.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    (get("node_path"), get("cargo_path"))
}

/// `chemin --version` : première ligne de sortie, sinon le message d'erreur
/// réel (erreur Windows au lancement, ou code de sortie et début de stderr).
pub fn real_probe(path: &Path) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("pas de réponse en {} s", PROBE_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let first = |b: &[u8]| {
        String::from_utf8_lossy(b)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if status.success() {
        Ok(first(&out.stdout))
    } else {
        let code = status
            .code()
            .map_or("sans code".to_string(), |c| c.to_string());
        Err(format!("code de sortie {code} : {}", first(&out.stderr)))
    }
}

/// Outil résolu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub version: String,
}

/// Pourquoi un outil n'a pas pu être résolu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// Aucun candidat n'existe.
    NotFound,
    /// Un candidat existe (ou est annoncé) mais ne se lance pas : message réel.
    Refused(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::NotFound => f.write_str("introuvable"),
            ToolError::Refused(m) => write!(f, "lancement refusé ({m})"),
        }
    }
}

#[cfg(windows)]
const EXTS: &[&str] = &[".exe", ".cmd", ".bat"];
#[cfg(not(windows))]
const EXTS: &[&str] = &[""];

#[cfg(windows)]
fn exe_name(stem: &str) -> String {
    format!("{stem}.exe")
}
#[cfg(not(windows))]
fn exe_name(stem: &str) -> String {
    stem.to_string()
}

/// Candidat : chemin, et si l'existence du fichier est exigée (faux pour un
/// lien qui peut exister sans être traversable : on veut alors l'erreur
/// réelle).
struct Candidate {
    path: PathBuf,
    must_exist: bool,
}

fn cand(path: PathBuf, must_exist: bool) -> Candidate {
    Candidate { path, must_exist }
}

/// Versions de nvm (`<NVM_HOME>\v*`) de la plus récente à la plus ancienne.
fn nvm_versions(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix('v')?;
            let nums: Vec<u64> = rest.split('.').map_while(|p| p.parse().ok()).collect();
            (!nums.is_empty()).then(|| (nums, e.path()))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

fn candidates(env: &ToolEnv, tool: &str, extra_dirs: &[PathBuf]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let exe = exe_name(tool);
    let over = match tool {
        "node" => env.node_path.as_ref(),
        "cargo" => env.cargo_path.as_ref(),
        _ => None,
    };
    if let Some(p) = over {
        out.push(cand(
            if p.is_dir() { p.join(&exe) } else { p.clone() },
            false,
        ));
    }
    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
    dirs.extend(env.path_dirs());
    for d in &dirs {
        for ext in EXTS {
            out.push(cand(d.join(format!("{tool}{ext}")), true));
        }
    }
    match tool {
        "node" => {
            if let Some(link) = env.var("NVM_SYMLINK").map(PathBuf::from) {
                out.push(cand(link.join(&exe), false));
                if let Ok(target) = std::fs::read_link(&link) {
                    out.push(cand(target.join(&exe), true));
                }
            }
            if let Some(home) = env.var("NVM_HOME") {
                for v in nvm_versions(Path::new(home)) {
                    out.push(cand(v.join(&exe), true));
                }
            }
        }
        "cargo" => {
            if let Some(h) = env.var("CARGO_HOME") {
                out.push(cand(Path::new(h).join("bin").join(&exe), true));
            }
            if let Some(h) = env.var("USERPROFILE") {
                out.push(cand(
                    Path::new(h).join(".cargo").join("bin").join(&exe),
                    true,
                ));
            }
        }
        "git" => {
            for k in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
                if let Some(h) = env.var(k) {
                    let base = Path::new(h);
                    out.push(cand(base.join("Git").join("cmd").join(&exe), true));
                    out.push(cand(
                        base.join("Programs").join("Git").join("cmd").join(&exe),
                        true,
                    ));
                }
            }
        }
        _ => {}
    }
    out
}

/// Résout `tool` (`node`, `npm`, `git`, `cargo`). `extra_dirs` : dossiers à
/// essayer d'abord (celui de Node pour `npm`). Chaque candidat est lancé.
pub fn resolve_tool(
    env: &ToolEnv,
    tool: &str,
    extra_dirs: &[PathBuf],
) -> Result<Resolved, ToolError> {
    let mut refused: Option<String> = None;
    let mut tried: Vec<PathBuf> = Vec::new();
    for c in candidates(env, tool, extra_dirs) {
        if tried.iter().any(|p| same_path(p, &c.path)) {
            continue;
        }
        tried.push(c.path.clone());
        if c.must_exist && !c.path.is_file() {
            continue;
        }
        match (env.probe)(&c.path) {
            Ok(version) => {
                if let Some(m) = &refused {
                    tracing::info!("{tool} : repli sur {} après « {m} »", c.path.display());
                }
                return Ok(Resolved {
                    path: c.path,
                    version,
                });
            }
            Err(e) => {
                let msg = format!("{} : {e}", c.path.display());
                tracing::warn!("{tool} : candidat refusé ({msg})");
                refused.get_or_insert(msg);
            }
        }
    }
    Err(refused.map_or(ToolError::NotFound, ToolError::Refused))
}

/// Outils résolus et PATH à donner aux sous-processus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Active {
    pub paths: HashMap<String, PathBuf>,
    pub path_env: Option<String>,
}

/// Résultat de [`check_tools`].
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub resolved: Vec<(String, Resolved)>,
    /// `(libellé, erreur)` de tous les outils en échec.
    pub failed: Vec<(String, ToolError)>,
    pub active: Active,
}

impl Report {
    /// « Node : lancement refusé (…) ; cargo : introuvable ».
    pub fn reason(&self) -> Option<String> {
        if self.failed.is_empty() {
            return None;
        }
        let list: Vec<String> = self
            .failed
            .iter()
            .map(|(l, e)| format!("{l} : {e}"))
            .collect();
        Some(format!(
            "{} ; /ameliorer demande git, Node, npm et cargo",
            list.join(" ; ")
        ))
    }
}

/// PATH des sous-processus : dossiers des outils résolus, puis PATH du
/// processus et du registre.
pub fn build_path(env: &ToolEnv, resolved: &HashMap<String, PathBuf>) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for key in ["node", "npm", "cargo", "git"] {
        if let Some(d) = resolved.get(key).and_then(|p| p.parent()) {
            if !dirs.iter().any(|q| same_path(q, d)) {
                dirs.push(d.to_path_buf());
            }
        }
    }
    for d in env.path_dirs() {
        if !dirs.iter().any(|q| same_path(q, &d)) {
            dirs.push(d);
        }
    }
    dirs.iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(&PATH_SEP.to_string())
}

/// Vérifie tous les outils (pas seulement le premier en échec). `checks` :
/// `(libellé affiché, nom d'outil)`. `npm` est cherché aussi à côté de Node.
pub fn check_tools(env: &ToolEnv, checks: &[(String, String)]) -> Report {
    let mut rep = Report::default();
    let mut node_dir: Vec<PathBuf> = Vec::new();
    for (label, tool) in checks {
        let extra = if tool == "npm" {
            node_dir.clone()
        } else {
            vec![]
        };
        match resolve_tool(env, tool, &extra) {
            Ok(r) => {
                if tool == "node" {
                    node_dir = r
                        .path
                        .parent()
                        .map(|d| vec![d.to_path_buf()])
                        .unwrap_or_default();
                }
                tracing::info!("{label} : {} ({})", r.path.display(), r.version);
                rep.active.paths.insert(tool.clone(), r.path.clone());
                rep.resolved.push((label.clone(), r));
            }
            Err(e) => rep.failed.push((label.clone(), e)),
        }
    }
    rep.active.path_env = Some(build_path(env, &rep.active.paths));
    rep
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// Retient les outils résolus pour les lancements suivants.
pub fn set_active(a: Active) {
    if let Ok(mut g) = ACTIVE.lock() {
        *g = Some(a);
    }
}

fn active() -> Option<Active> {
    ACTIVE.lock().ok().and_then(|g| g.clone())
}

/// Programme à lancer pour `name` (`node`, `npm`, `npm.cmd`, `cargo`) :
/// chemin résolu, sinon `name`.
pub fn program(name: &str) -> String {
    let key = name.strip_suffix(".cmd").unwrap_or(name);
    active()
        .and_then(|a| a.paths.get(key).cloned())
        .map_or_else(|| name.to_string(), |p| p.to_string_lossy().into_owned())
}

/// PATH enrichi pour les sous-processus (Node et cargo en tête), s'il a été
/// calculé.
pub fn subprocess_path() -> Option<String> {
    active().and_then(|a| a.path_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orch-tools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, stem: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(exe_name(stem));
        std::fs::write(&p, b"").unwrap();
        p
    }

    /// Sonde simulée : réussit pour les chemins listés, sinon l'erreur donnée.
    fn probe(ok: Vec<PathBuf>, refuse: Vec<(PathBuf, &'static str)>) -> Probe {
        Arc::new(move |p: &Path| {
            if ok.iter().any(|o| same_path(o, p)) {
                return Ok("v1.0.0".into());
            }
            for (r, msg) in &refuse {
                if same_path(r, p) {
                    return Err((*msg).to_string());
                }
            }
            Err("introuvable".into())
        })
    }

    fn env_with(vars: &[(&str, String)], probe: Probe) -> ToolEnv {
        ToolEnv {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_ascii_uppercase(), v.clone()))
                .collect(),
            user_path: None,
            machine_path: None,
            node_path: None,
            cargo_path: None,
            probe,
        }
    }

    #[test]
    fn expands_percent_variables() {
        let mut v = HashMap::new();
        v.insert("USERPROFILE".to_string(), r"C:\Users\x".to_string());
        assert_eq!(
            expand_vars(r"%userprofile%\.cargo\bin", &v),
            r"C:\Users\x\.cargo\bin"
        );
        assert_eq!(expand_vars("%INCONNUE%\\a", &v), "%INCONNUE%\\a");
        assert_eq!(expand_vars("100%", &v), "100%");
    }

    #[test]
    fn parses_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    %USERPROFILE%\\bin;C:\\x y\r\n\r\n";
        assert_eq!(
            parse_reg_query(out).as_deref(),
            Some(r"%USERPROFILE%\bin;C:\x y")
        );
        assert_eq!(parse_reg_query("rien"), None);
    }

    /// PATH du processus périmé : Node n'est trouvé que par le PATH du
    /// registre (variables développées).
    #[test]
    fn stale_path_resolved_through_registry() {
        let root = tmp("registry");
        let node = touch(&root.join("apps").join("node"), "node");
        let mut env = env_with(
            &[
                ("PATH", r"C:\inexistant".into()),
                ("MESAPPS", root.join("apps").to_string_lossy().into_owned()),
            ],
            probe(vec![node.clone()], vec![]),
        );
        assert_eq!(resolve_tool(&env, "node", &[]), Err(ToolError::NotFound));
        env.user_path = Some(format!(
            "{};%MESAPPS%{}node",
            PATH_SEP,
            std::path::MAIN_SEPARATOR
        ));
        let r = resolve_tool(&env, "node", &[]).unwrap();
        assert!(same_path(&r.path, &node), "{:?}", r.path);
        assert_eq!(r.version, "v1.0.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Le lien de nvm existe mais n'est pas traversable : repli sur la
    /// version de nvm la plus récente, sans passer par le lien.
    #[test]
    fn untraversable_nvm_link_falls_back_to_versions() {
        let root = tmp("nvm");
        let link = root.join("nodejs");
        let home = root.join("nvm");
        let old = touch(&home.join("v18.9.0"), "node");
        let new = touch(&home.join("v24.4.1"), "node");
        let _ = touch(&home.join("v9.0.0"), "node");
        let via_link = link.join(exe_name("node"));
        let env = env_with(
            &[
                ("PATH", link.to_string_lossy().into_owned()),
                ("NVM_SYMLINK", link.to_string_lossy().into_owned()),
                ("NVM_HOME", home.to_string_lossy().into_owned()),
            ],
            probe(
                vec![old, new.clone()],
                vec![(via_link, "point de montage non approuvé")],
            ),
        );
        let r = resolve_tool(&env, "node", &[]).unwrap();
        assert!(same_path(&r.path, &new), "v24 attendue : {:?}", r.path);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Si tout est refusé, l'erreur réelle remonte (et pas « introuvable »).
    #[test]
    fn refusal_keeps_real_message() {
        let root = tmp("refus");
        let link = root.join("nodejs");
        let env = env_with(
            &[("NVM_SYMLINK", link.to_string_lossy().into_owned())],
            probe(vec![], vec![(link.join(exe_name("node")), "os error 448")]),
        );
        match resolve_tool(&env, "node", &[]) {
            Err(ToolError::Refused(m)) => assert!(m.contains("os error 448"), "{m}"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `node_path` passe avant le PATH ; un dossier est accepté.
    #[test]
    fn node_path_override_wins() {
        let root = tmp("override");
        let in_path = touch(&root.join("path"), "node");
        let custom = touch(&root.join("custom"), "node");
        let mut env = env_with(
            &[("PATH", root.join("path").to_string_lossy().into_owned())],
            probe(vec![in_path.clone(), custom.clone()], vec![]),
        );
        env.node_path = Some(custom.clone());
        assert!(same_path(
            &resolve_tool(&env, "node", &[]).unwrap().path,
            &custom
        ));
        env.node_path = Some(root.join("custom"));
        assert!(same_path(
            &resolve_tool(&env, "node", &[]).unwrap().path,
            &custom
        ));
        // Réglage inutilisable : repli sur le PATH.
        env.node_path = Some(root.join("nulle-part").join("node.exe"));
        assert!(same_path(
            &resolve_tool(&env, "node", &[]).unwrap().path,
            &in_path
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overrides_read_from_config_json() {
        let root = tmp("config");
        let f = root.join("config.json");
        std::fs::write(
            &f,
            r#"{"repo_url":"x","node_path":" D:\\n\\node.exe ","cargo_path":""}"#,
        )
        .unwrap();
        let (n, c) = overrides_from(Some(&f));
        assert_eq!(n, Some(PathBuf::from(r"D:\n\node.exe")));
        assert_eq!(c, None);
        assert_eq!(overrides_from(None), (None, None));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cargo_found_in_cargo_home_and_profile() {
        let root = tmp("cargo");
        let a = touch(&root.join("home").join("bin"), "cargo");
        let b = touch(&root.join("profile").join(".cargo").join("bin"), "cargo");
        let env = env_with(
            &[
                (
                    "CARGO_HOME",
                    root.join("home").to_string_lossy().into_owned(),
                ),
                (
                    "USERPROFILE",
                    root.join("profile").to_string_lossy().into_owned(),
                ),
            ],
            probe(vec![a.clone(), b.clone()], vec![]),
        );
        assert!(same_path(
            &resolve_tool(&env, "cargo", &[]).unwrap().path,
            &a
        ));
        let env = env_with(
            &[(
                "USERPROFILE",
                root.join("profile").to_string_lossy().into_owned(),
            )],
            probe(vec![b.clone()], vec![]),
        );
        assert!(same_path(
            &resolve_tool(&env, "cargo", &[]).unwrap().path,
            &b
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Tous les outils sont vérifiés et cités ; npm est cherché à côté de
    /// Node ; Node et cargo passent en tête du PATH des sous-processus.
    #[test]
    fn all_tools_reported_and_path_prefixed() {
        let root = tmp("report");
        let link = root.join("nodejs");
        let v = root.join("nvm").join("v24.4.1");
        let node = touch(&v, "node");
        let npm = v.join("npm.cmd");
        std::fs::write(&npm, b"").unwrap();
        let env = env_with(
            &[
                ("PATH", r"C:\autre".into()),
                ("NVM_SYMLINK", link.to_string_lossy().into_owned()),
                ("NVM_HOME", root.join("nvm").to_string_lossy().into_owned()),
            ],
            probe(
                vec![node.clone(), npm.clone()],
                vec![(link.join(exe_name("node")), "point de montage non approuvé")],
            ),
        );
        let checks: Vec<(String, String)> = [
            ("git", "git"),
            ("Node", "node"),
            ("npm", "npm"),
            ("cargo", "cargo"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let rep = check_tools(&env, &checks);
        assert_eq!(rep.resolved.len(), 2, "{rep:?}");
        let labels: Vec<_> = rep.failed.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["git", "cargo"]);
        let reason = rep.reason().unwrap();
        assert!(reason.contains("git : introuvable"), "{reason}");
        assert!(reason.contains("cargo : introuvable"), "{reason}");
        let p = rep.active.path_env.clone().unwrap();
        assert!(p.starts_with(&v.to_string_lossy().into_owned()), "{p}");
        assert!(p.contains(r"C:\autre"));

        // Node refusé et cargo absent, cités ensemble avec le message réel.
        let env2 = env_with(
            &[("NVM_SYMLINK", link.to_string_lossy().into_owned())],
            probe(vec![], vec![(link.join(exe_name("node")), "os error 448")]),
        );
        let rep = check_tools(&env2, &checks[1..2]);
        let reason = rep.reason().unwrap();
        assert!(reason.starts_with("Node : lancement refusé ("), "{reason}");
        assert!(reason.contains("os error 448"), "{reason}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Diagnostic sur la machine réelle :
    /// `cargo test --lib real_machine -- --ignored --nocapture`.
    #[test]
    #[ignore = "diagnostic de la machine réelle"]
    fn real_machine() {
        let env = ToolEnv::real();
        println!("NVM_SYMLINK défini : {}", env.var("NVM_SYMLINK").is_some());
        println!("NVM_HOME défini : {}", env.var("NVM_HOME").is_some());
        println!(
            "dossiers du PATH (process + registre) : {}",
            env.path_dirs().len()
        );
        let checks: Vec<(String, String)> = [
            ("git", "git"),
            ("Node", "node"),
            ("npm", "npm"),
            ("cargo", "cargo"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let rep = check_tools(&env, &checks);
        for (l, r) in &rep.resolved {
            println!("OK  {l} : {} -> {}", r.path.display(), r.version);
        }
        for (l, e) in &rep.failed {
            println!("KO  {l} : {e}");
        }
        println!("raison : {:?}", rep.reason());
        let first = rep.active.path_env.unwrap_or_default();
        println!(
            "PATH sous-processus, 3 premiers dossiers : {:?}",
            first.split(PATH_SEP).take(3).collect::<Vec<_>>()
        );
    }
}
