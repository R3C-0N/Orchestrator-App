//! Commandes Tauri (plan 1.5). Noms d'arguments en `snake_case` côté JS.
//!
//! J0 : `list_sessions` et `load_history` ont un corps provisoire (jalon de la
//! reprise de session).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use super::events::CoreEvent;
use super::types::{
    Answers, CoreError, Diagnostics, ExitInfo, InterruptAck, LaunchInfo, SessionKey,
    SessionStarted, SessionSummary, SkipReason,
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
    pub fn new(cwd: PathBuf) -> Self {
        AppState {
            sessions: Mutex::new(HashMap::new()),
            launch: LaunchInfo {
                cwd: display_path(&cwd),
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
    let s = state.get(&session)?;
    s.set_model(&model).await
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

#[tauri::command(rename_all = "snake_case")]
pub fn list_sessions(cwd: String) -> Result<Vec<SessionSummary>, CoreError> {
    // Provisoire (J0) : la lecture des transcripts arrive avec la reprise.
    let _ = cwd;
    Ok(vec![])
}

#[tauri::command(rename_all = "snake_case")]
pub fn load_history(session: SessionKey, session_id: String) -> Result<(), CoreError> {
    let _ = (session, session_id);
    Err(CoreError::Unsupported(
        "load_history (jalon de reprise)".into(),
    ))
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
