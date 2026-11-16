//! Commandes Tauri (plan 1.5). Noms d'arguments en `snake_case` côté JS.
//!
//! J0 : `list_sessions` et `load_history` ont un corps provisoire (jalon de la
//! reprise de session).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::State;

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
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_launch_info(state: State<'_, AppState>) -> LaunchInfo {
    state.launch.clone()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_session(
    state: State<'_, AppState>,
    cwd: String,
    resume: Option<String>,
    model: Option<String>,
    events: Channel<Vec<CoreEvent>>,
) -> Result<SessionStarted, CoreError> {
    let exe = locate_claude().map_err(|tried| CoreError::ClaudeNotFound(tried.join(" ; ")))?;
    let cwd = PathBuf::from(cwd);
    let key = SessionKey(uuid::Uuid::new_v4().to_string());
    let sink: crate::state::batcher::Sink = Arc::new(move |batch| events.send(batch).is_ok());
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
    let s = state
        .sessions
        .lock()
        .unwrap()
        .remove(&session)
        .ok_or(CoreError::NotRunning)?;
    let info = s.stop().await;
    *state.last.lock().unwrap() = Some(s);
    Ok(info)
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_diagnostics(state: State<'_, AppState>) -> Diagnostics {
    let current = state.sessions.lock().unwrap().values().next().cloned();
    let s = current.or_else(|| state.last.lock().unwrap().clone());
    let mut d = Diagnostics {
        expected_version: EXPECTED_CLAUDE_VERSION.into(),
        claude_path: state.launch.claude_path.clone(),
        ..Default::default()
    };
    if let Some(s) = s {
        d.claude_version = s.version();
        d.capabilities = s.capabilities();
        let tail = s.stderr_tail();
        d.stderr_tail = tail.iter().rev().take(50).rev().cloned().collect();
    }
    d
}
