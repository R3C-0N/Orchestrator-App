//! Commandes Tauri (plan 1.5). Noms d'arguments en `snake_case` côté JS.
//!
//! J11 : `list_projects`, `list_sessions` et `load_history` lisent les
//! transcripts du CLI (module `sessions`, lecture seule).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use super::events::CoreEvent;
use super::types::{
    Answers, CoreError, Diagnostics, ExitInfo, HistoryLoaded, InterruptAck, LaunchInfo,
    ProjectSummary, SessionKey, SessionResync, SessionStarted, SessionSummary, SkipReason,
};
use crate::process::locate::{display_path, locate_claude, EXPECTED_CLAUDE_VERSION};
use crate::process::spawn::LaunchOptions;
use crate::process::supervisor::Session;

/// État partagé de l'app.
pub struct AppState {
    pub sessions: Mutex<HashMap<SessionKey, Arc<Session>>>,
    pub launch: LaunchInfo,
    /// Dernière session arrêtée ou morte, pour les diagnostics.
    pub last: Mutex<Option<Arc<Session>>>,
}

impl AppState {
    /// `cwd` réputé passé en argument (tests et outils).
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_launch(crate::sessions::LaunchDir {
            cwd,
            from_args: true,
            error: None,
        })
    }

    /// Dossier de lancement lu dans les arguments (`sessions::launch_dir_from_args`).
    pub fn with_launch(dir: crate::sessions::LaunchDir) -> Self {
        AppState {
            sessions: Mutex::new(HashMap::new()),
            launch: LaunchInfo {
                cwd: display_path(&dir.cwd),
                cwd_from_args: dir.from_args,
                launch_error: dir.error,
                claude_path: locate_claude().ok().map(|p| display_path(&p)),
                expected_version: EXPECTED_CLAUDE_VERSION.into(),
            },
            last: Mutex::new(None),
        }
    }

    fn get(&self, key: &SessionKey) -> Result<Arc<Session>, CoreError> {
        self.sessions
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(CoreError::NotRunning)
    }

    /// Arrête toutes les sessions (fermeture de l'app, plan 1.9).
    pub async fn stop_all(&self) {
        let all: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in all {
            s.stop().await;
            *self.last.lock().unwrap() = Some(s);
        }
    }

    /// Ferme une session et la retire de la table ; `None` si elle n'y est
    /// plus (déjà fermée). La session reste visible des diagnostics (`last`).
    pub async fn close_session(&self, key: &SessionKey) -> Option<ExitInfo> {
        let s = self.sessions.lock().unwrap().remove(key)?;
        let info = s.stop().await;
        *self.last.lock().unwrap() = Some(s);
        Some(info)
    }

    /// La webview (re)charge sa page : l'interface qui tenait les canaux des
    /// sessions n'existe plus, ces sessions sont orphelines (plan J6). Elles
    /// sont fermées ; l'interface relancée repart de `get_launch_info`.
    pub async fn close_orphans(&self) -> usize {
        let n = self.sessions.lock().unwrap().len();
        if n > 0 {
            tracing::warn!(
                sessions = n,
                "rechargement de l'interface : sessions orphelines fermées"
            );
            self.stop_all().await;
        }
        n
    }

    /// Retire de la table les sessions dont le processus est mort ; la plus
    /// récente reste dans `last` pour les diagnostics.
    fn forget_dead(&self) {
        let mut map = self.sessions.lock().unwrap();
        let dead: Vec<SessionKey> = map
            .iter()
            .filter(|(_, s)| !s.is_running())
            .map(|(k, _)| k.clone())
            .collect();
        for k in dead {
            if let Some(s) = map.remove(&k) {
                *self.last.lock().unwrap() = Some(s);
            }
        }
    }
}

/// Destination des événements d'une session : le canal de l'interface. Si le
/// canal refuse un lot (interface fermée ou rechargée), la session est
/// orpheline : elle est fermée et retirée de `AppState.sessions`, une fois.
fn channel_sink(
    events: Channel<Vec<CoreEvent>>,
    app: AppHandle,
    key: SessionKey,
) -> crate::state::batcher::Sink {
    let closed = Arc::new(AtomicBool::new(false));
    Arc::new(move |batch| {
        if events.send(batch).is_ok() {
            return true;
        }
        if !closed.swap(true, Ordering::SeqCst) {
            tracing::warn!(session = %key.0, "canal de l'interface fermé : session orpheline fermée");
            let app = app.clone();
            let key = key.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<AppState>().close_session(&key).await;
            });
        }
        false
    })
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_launch_info(state: State<'_, AppState>) -> LaunchInfo {
    state.launch.clone()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_session(
    app: AppHandle,
    state: State<'_, AppState>,
    cwd: String,
    resume: Option<String>,
    model: Option<String>,
    events: Channel<Vec<CoreEvent>>,
) -> Result<SessionStarted, CoreError> {
    let exe = locate_claude().map_err(|tried| CoreError::ClaudeNotFound(tried.join(" ; ")))?;
    let cwd = PathBuf::from(cwd);
    let key = SessionKey(uuid::Uuid::new_v4().to_string());
    state.forget_dead();
    let sink = channel_sink(events, app, key.clone());
    let session = Session::start(
        key.clone(),
        &exe,
        &cwd,
        LaunchOptions {
            resume,
            model,
            extra_args: vec![],
        },
        sink,
    )
    .await?;
    let started = SessionStarted {
        session: key.clone(),
        pid: session.pid,
        claude_path: display_path(&exe),
    };
    state.sessions.lock().unwrap().insert(key, session);
    Ok(started)
}

#[tauri::command(rename_all = "snake_case")]
pub fn send_message(
    state: State<'_, AppState>,
    session: SessionKey,
    text: String,
) -> Result<(), CoreError> {
    state.get(&session)?.send_message(&text)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn interrupt(
    state: State<'_, AppState>,
    session: SessionKey,
    cancel_queued: bool,
) -> Result<InterruptAck, CoreError> {
    let s = state.get(&session)?;
    s.interrupt(cancel_queued).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_model(
    state: State<'_, AppState>,
    session: SessionKey,
    model: String,
) -> Result<String, CoreError> {
    validate_model(&model)?;
    let s = state.get(&session)?;
    s.set_model(&model).await
}

/// Même règle que l'argument `--model` du lancement
/// (`process::spawn::validate_launch_options`) : non vide, pas de `-` en
/// tête, ni espace ni caractère de contrôle. Sinon `InvalidArgument`, sans
/// rien envoyer au CLI.
pub fn validate_model(model: &str) -> Result<(), CoreError> {
    crate::process::spawn::validate_launch_options(&LaunchOptions {
        model: Some(model.to_string()),
        ..LaunchOptions::default()
    })
    .map_err(CoreError::InvalidArgument)
}

#[tauri::command(rename_all = "snake_case")]
pub fn answer_question(
    state: State<'_, AppState>,
    session: SessionKey,
    request_id: String,
    answers: Answers,
    response: Option<String>,
) -> Result<(), CoreError> {
    state
        .get(&session)?
        .answer_question(&request_id, &answers, response.as_deref())
}

#[tauri::command(rename_all = "snake_case")]
pub fn skip_question(
    state: State<'_, AppState>,
    session: SessionKey,
    request_id: String,
    reason: SkipReason,
) -> Result<(), CoreError> {
    state.get(&session)?.skip_question(&request_id, reason)
}

fn projects_root() -> Result<PathBuf, CoreError> {
    crate::sessions::paths::projects_root()
        .ok_or_else(|| CoreError::Io("dossier personnel introuvable (USERPROFILE)".into()))
}

/// Projets récents de `~/.claude/projects` (J11, décision 11), du plus
/// récent au plus ancien. Lecture seule.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_projects() -> Result<Vec<ProjectSummary>, CoreError> {
    let root = projects_root()?;
    tauri::async_runtime::spawn_blocking(move || crate::sessions::list::list_projects(&root))
        .await
        .map_err(|e| CoreError::Io(e.to_string()))
}

/// Sessions du projet `cwd`, de la plus récente à la plus ancienne, 200 au
/// plus (plan 1.8). Projet jamais ouvert avec le CLI : liste vide.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_sessions(cwd: String) -> Result<Vec<SessionSummary>, CoreError> {
    let root = projects_root()?;
    tauri::async_runtime::spawn_blocking(move || crate::sessions::list::list_sessions(&root, &cwd))
        .await
        .map_err(|e| CoreError::Io(e.to_string()))
}

/// Historique de la session `session_id` (décision 9), relu dans le
/// transcript du dossier de travail de la session `session` (celle lancée
/// avec `resume`). Les événements sont **renvoyés** (pas poussés dans le
/// canal) : l'interface les applique avant les événements vivants. Erreur :
/// l'interface reprend sans historique, avec un avertissement.
#[tauri::command(rename_all = "snake_case")]
pub async fn load_history(
    state: State<'_, AppState>,
    session: SessionKey,
    session_id: String,
) -> Result<HistoryLoaded, CoreError> {
    let cwd = display_path(&state.get(&session)?.cwd);
    let root = projects_root()?;
    tauri::async_runtime::spawn_blocking(move || {
        let dir = crate::sessions::paths::find_project_dir(&root, &cwd)
            .ok_or_else(|| CoreError::Io(format!("aucun transcript pour le dossier {cwd}")))?;
        crate::sessions::transcript::load_history(&dir, &session_id, &session)
    })
    .await
    .map_err(|e| CoreError::Io(e.to_string()))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stop_session(
    state: State<'_, AppState>,
    session: SessionKey,
) -> Result<ExitInfo, CoreError> {
    state
        .close_session(&session)
        .await
        .ok_or(CoreError::NotRunning)
}

/// J14 (spec décision 15) : une interface (re)chargée se rattache aux
/// sessions vivantes. `events` devient le canal de toutes les sessions (les
/// `CoreEvent` portent leur `session`), y compris celles lancées par le cœur
/// (amélioration) ; la réponse donne, par session, de quoi reconstruire son
/// état (historique repris, journal des événements). Étape 0 : aucune
/// session n'est gardée au rechargement, la liste est vide.
#[tauri::command(rename_all = "snake_case")]
pub fn attach_sessions(events: Channel<Vec<CoreEvent>>) -> Result<Vec<SessionResync>, CoreError> {
    let _ = events;
    Ok(Vec::new())
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_diagnostics(state: State<'_, AppState>) -> Diagnostics {
    let current = state.sessions.lock().unwrap().values().next().cloned();
    let s = current.or_else(|| state.last.lock().unwrap().clone());
    let mut d = Diagnostics {
        expected_version: EXPECTED_CLAUDE_VERSION.into(),
        claude_path: state.launch.claude_path.clone(),
        log_path: crate::logging::log_dir().map(display_path),
        ..Default::default()
    };
    if let Some(s) = s {
        d.claude_version = s.version();
        d.capabilities = s.capabilities();
        d.unknown_count = s.unknown_count();
        let tail = s.stderr_tail();
        d.stderr_tail = tail.iter().rev().take(50).rev().cloned().collect();
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_model_value_is_validated_like_the_launch_argument() {
        for ok in ["haiku", "claude-sonnet-5-5", "opus[1m]"] {
            assert_eq!(validate_model(ok), Ok(()), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--dangerously-skip-permissions",
            "haiku sonnet",
            "a\tb",
            "a\nb",
            "a\u{7f}",
        ] {
            assert!(
                matches!(validate_model(bad), Err(CoreError::InvalidArgument(_))),
                "{bad:?}"
            );
        }
    }
}
