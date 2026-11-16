//! Listes des projets et des sessions (plan 1.8, décisions 9 et 11).
//!
//! - `list_projects` : un sous-dossier de `~/.claude/projects` par projet,
//!   trié par date du transcript le plus récent ; le vrai chemin vient du
//!   champ `cwd` des premières lignes de ce transcript (l'encodage du nom
//!   n'est pas réversible) ; `exists` dit si ce dossier existe encore.
//! - `list_sessions` : date et taille lues dans le dossier sans ouvrir les
//!   fichiers, puis seulement la **fin** (64 ko) des 200 plus récents pour
//!   le titre (`custom-title`, `ai-title`) et le dernier prompt
//!   (`last-prompt`).
//!
//! Tolérant : un fichier ou une ligne illisible est ignoré, jamais bloquant.
//! Lecture seule.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use super::paths::{encode_project_dir, find_project_dir, is_session_id};
use crate::api::types::{ProjectSummary, SessionSummary};

/// Nombre maximal de sessions renvoyées par `list_sessions`.
pub const MAX_SESSIONS: usize = 200;
/// Taille lue à la fin de chaque transcript pour le titre et le dernier prompt.
pub const TAIL_BYTES: u64 = 64 * 1024;
/// Taille lue au début d'un transcript pour son `cwd`, ou pour un prompt de
/// repli quand la fin n'a ni titre ni `last-prompt`.
pub const HEAD_BYTES: u64 = 256 * 1024;
/// Transcripts essayés, du plus récent au plus ancien, pour trouver un `cwd`.
const CWD_TRIES: usize = 3;
/// Longueur maximale (caractères) d'un titre ou d'un prompt renvoyé.
const MAX_TEXT_CHARS: usize = 300;

/// Un transcript vu dans le dossier, sans l'ouvrir.
#[derive(Debug, Clone)]
struct TranscriptFile {
    id: String,
    path: PathBuf,
    modified: u64,
    size: u64,
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Transcripts `<uuid>.jsonl` d'un dossier de projet, du plus récent au plus
/// ancien. Les autres fichiers et les sous-dossiers sont ignorés.
fn transcripts(dir: &Path) -> Vec<TranscriptFile> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<TranscriptFile> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let id = name.to_str()?.strip_suffix(".jsonl")?;
            if !is_session_id(id) {
                return None;
            }
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some(TranscriptFile {
                id: id.to_string(),
                path: e.path(),
                modified: mtime_ms(&meta),
                size: meta.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.id.cmp(&b.id)));
    out
}

/// Lit au plus `max` octets à partir de `from`. Lecture partielle tolérée.
fn read_range(path: &Path, from: u64, max: u64) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    if from > 0 {
        f.seek(SeekFrom::Start(from)).ok()?;
    }
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Lignes complètes d'un extrait. `skip_first` : l'extrait commence au milieu
/// d'une ligne (lecture de la fin), la première est jetée. La dernière ligne,
/// si elle n'est pas terminée, est gardée : le parseur JSON la rejettera.
fn lines_of(buf: &[u8], skip_first: bool) -> Vec<String> {
    let text = String::from_utf8_lossy(buf);
    let mut it = text.split('\n');
    if skip_first {
        it.next();
    }
    it.map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Premier `cwd` du début d'un transcript ; celui dont l'encodage redonne le
/// nom du dossier est préféré (un `cd` en cours de session peut changer le
/// champ).
fn cwd_from_head(path: &Path, dir_name: &str) -> Option<String> {
    let buf = read_range(path, 0, HEAD_BYTES)?;
    let mut first = None;
    for line in lines_of(&buf, false) {
        if !line.contains("\"cwd\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(cwd) = v
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        if encode_project_dir(cwd.trim_end_matches(['\\', '/'])).eq_ignore_ascii_case(dir_name) {
            return Some(cwd.to_string());
        }
        first.get_or_insert_with(|| cwd.to_string());
    }
    first
}

/// Vrai chemin d'un dossier de projet : `cwd` lu dans ses transcripts les plus
/// récents (plan 1.8). `None` si aucun n'en contient.
pub fn read_project_cwd(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?.to_string();
    transcripts(dir)
        .iter()
        .take(CWD_TRIES)
        .find_map(|t| cwd_from_head(&t.path, &name))
}

/// Projets récents de `root` (`~/.claude/projects`), du plus récent au plus
/// ancien. Un dossier sans transcript est ignoré. `root` absent : liste vide.
pub fn list_projects(root: &Path) -> Vec<ProjectSummary> {
    let Ok(rd) = fs::read_dir(root) else {
        return vec![];
    };
    let mut out: Vec<ProjectSummary> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let dir = e.path();
            let encoded = e.file_name().to_str()?.to_string();
            let files = transcripts(&dir);
            let last_used = files.first()?.modified;
            let path = files
                .iter()
                .take(CWD_TRIES)
                .find_map(|t| cwd_from_head(&t.path, &encoded));
            let exists = path.as_deref().is_some_and(|p| Path::new(p).is_dir());
            Some(ProjectSummary {
                encoded,
                path,
                exists,
                last_used,
                session_count: files.len() as u32,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_used
            .cmp(&a.last_used)
            .then(a.encoded.cmp(&b.encoded))
    });
    out
}

/// Ce que la fin d'un transcript apprend sur sa session.
#[derive(Debug, Default)]
struct TailInfo {
    custom_title: Option<String>,
    ai_title: Option<String>,
    last_prompt: Option<String>,
    entrypoint: Option<String>,
}

impl TailInfo {
    fn complete(&self) -> bool {
        (self.custom_title.is_some() || self.ai_title.is_some())
            && self.last_prompt.is_some()
            && self.entrypoint.is_some()
    }
}

/// Première étape de la lecture de la fin ; elle double jusqu'à [`TAIL_BYTES`].
const TAIL_STEP: u64 = 16 * 1024;

/// Vrai si `hay` contient `needle` (octets).
fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Lit la fin du transcript **à rebours** : la ligne la plus proche de la fin
/// l'emporte, et la lecture s'arrête dès que titre, dernier prompt et
/// `entrypoint` sont connus (en général dans les 16 derniers ko).
fn tail_info(t: &TranscriptFile) -> TailInfo {
    let mut info = TailInfo::default();
    let mut span = TAIL_STEP.min(t.size);
    loop {
        let from = t.size - span;
        let Some(buf) = read_range(&t.path, from, span) else {
            return info;
        };
        info = TailInfo::default();
        let mut lines: Vec<&[u8]> = buf.split(|b| *b == b'\n').collect();
        if from > 0 && !lines.is_empty() {
            // Ligne coupée au début de l'extrait.
            lines.remove(0);
        }
        for line in lines.into_iter().rev() {
            scan_tail_line(line, &mut info);
            if info.complete() {
                return info;
            }
        }
        if span >= TAIL_BYTES.min(t.size) {
            return info;
        }
        span = (span * 2).min(TAIL_BYTES).min(t.size);
    }
}

/// Une ligne de la fin, lue de la plus récente à la plus ancienne : seule la
/// première valeur trouvée de chaque champ est gardée.
fn scan_tail_line(line: &[u8], info: &mut TailInfo) {
    // Les lignes de métadonnées du CLI commencent par leur type : test
    // rapide sur le début de la ligne avant toute analyse JSON.
    let head = &line[..line.len().min(48)];
    let meta = has(head, b"\"ai-title\"")
        || has(head, b"\"custom-title\"")
        || has(head, b"\"last-prompt\"");
    // `entrypoint` est vers la fin des lignes `user` / `assistant` (avant
    // `cwd`, `sessionId`, `version`, `gitBranch`) : on ne cherche que dans
    // les derniers octets, pour un coût fixe par ligne.
    let end = &line[line.len().saturating_sub(512)..];
    let want_entry = info.entrypoint.is_none() && !meta && has(end, b"\"entrypoint\"");
    if !meta && !want_entry {
        return;
    }
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|x| !x.trim().is_empty())
            .map(clip)
    };
    match v.get("type").and_then(Value::as_str) {
        Some("ai-title") if info.ai_title.is_none() => info.ai_title = s("aiTitle"),
        Some("custom-title") if info.custom_title.is_none() => info.custom_title = s("customTitle"),
        Some("last-prompt") if info.last_prompt.is_none() => info.last_prompt = s("lastPrompt"),
        _ => {}
    }
    if info.entrypoint.is_none() {
        info.entrypoint = s("entrypoint");
    }
}

/// Texte d'un message utilisateur tapé (chaîne ou blocs `text`), hors
/// messages internes (`isMeta`, balises `<…>` du CLI).
fn typed_prompt(v: &Value) -> Option<String> {
    if v.get("type").and_then(Value::as_str) != Some("user")
        || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted"))
        .then(|| clip(t))
}

/// Repli quand la fin n'a ni titre ni `last-prompt` : premier prompt tapé et
/// `entrypoint` du début du fichier.
fn head_info(t: &TranscriptFile) -> (Option<String>, Option<String>) {
    let Some(buf) = read_range(&t.path, 0, HEAD_BYTES) else {
        return (None, None);
    };
    let mut entrypoint = None;
    for line in lines_of(&buf, false) {
        if !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entrypoint.is_none() {
            entrypoint = v
                .get("entrypoint")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if let Some(p) = typed_prompt(&v) {
            return (Some(p), entrypoint);
        }
    }
    (None, entrypoint)
}

/// Sessions du projet `cwd` sous `root`, de la plus récente à la plus
/// ancienne, au plus [`MAX_SESSIONS`]. Aucun dossier pour ce projet : liste
/// vide (projet jamais ouvert avec le CLI).
pub fn list_sessions(root: &Path, cwd: &str) -> Vec<SessionSummary> {
    let Some(dir) = find_project_dir(root, cwd) else {
        return vec![];
    };
    let mut files = transcripts(&dir);
    files.truncate(MAX_SESSIONS);
    files
        .iter()
        .map(|t| {
            let mut info = tail_info(t);
            if info.ai_title.is_none() && info.custom_title.is_none() && info.last_prompt.is_none()
            {
                let (prompt, entry) = head_info(t);
                info.last_prompt = prompt;
                info.entrypoint = info.entrypoint.or(entry);
            }
            SessionSummary {
                id: t.id.clone(),
                // Un titre donné par `/rename` passe avant celui du modèle.
                title: info.custom_title.or(info.ai_title),
                last_prompt: info.last_prompt,
                modified: t.modified,
                size_bytes: t.size,
                entrypoint: info.entrypoint,
            }
        })
        .collect()
}
