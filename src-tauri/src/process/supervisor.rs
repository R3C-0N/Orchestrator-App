//! Une session `claude.exe` : lancement, écriture sur stdin, lecture de stdout
//! et stderr ligne à ligne, corrélation des requêtes de contrôle, arrêt.
//!
//! Indépendant de Tauri : les événements sortent par un [`Sink`] (Channel Tauri
//! dans l'app, impression JSON dans le binaire `probe` et les tests).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::api::events::{
    CoreEvent, EventBody, NoticeLevel, QuestionCloseReason, SessionState, ThreadId, UserOrigin,
};
use crate::api::types::{Answers, CoreError, ExitInfo, InterruptAck, SessionKey, SkipReason};
use crate::process::locate::display_path;
use crate::process::reader::{LineReader, RawLine};
use crate::process::spawn::{
    build_args, spawn_claude, validate_launch_options, Job, LaunchOptions,
};
use crate::process::writer;
use crate::protocol::inbound::Inbound;
use crate::protocol::outbound;
use crate::state::batcher::{run_batcher, Sink};
use crate::state::reducer::{Action, Reducer, CAP_UNANSWERED};

pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// Attente maximale de la fin de lecture de stderr après un échec de démarrage.
pub const STDERR_DRAIN: Duration = Duration::from_secs(1);
const STDERR_KEEP: usize = 200;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Numérote et horodate les événements, puis les passe au regroupeur.
#[derive(Clone)]
struct Emitter {
    session: SessionKey,
    seq: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<CoreEvent>,
}

impl Emitter {
    fn emit(&self, body: EventBody, t: u64) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(CoreEvent {
            seq,
            t,
            session: self.session.clone(),
            body,
        });
    }
}

pub struct Session {
    pub key: SessionKey,
    pub pid: u32,
    pub claude_path: PathBuf,
    pub cwd: PathBuf,
    stdin_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    reducer: Arc<Mutex<Reducer>>,
    pending: Pending,
    emitter: Emitter,
    job: Job,
    exit_rx: watch::Receiver<Option<Option<i32>>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    next_id: AtomicU64,
}

impl Session {
    /// Lance `claude.exe`, envoie `initialize` et attend sa réponse (20 s).
    pub async fn start(
        key: SessionKey,
        exe: &Path,
        cwd: &Path,
        opts: LaunchOptions,
        sink: Sink,
    ) -> Result<Arc<Session>, CoreError> {
        if !cwd.is_dir() {
            return Err(CoreError::BadCwd(display_path(cwd)));
        }
        validate_launch_options(&opts).map_err(CoreError::InvalidArgument)?;
        let args = build_args(&opts);
        let mut spawned =
            spawn_claude(exe, cwd, &args).map_err(|e| CoreError::SpawnFailed(e.to_string()))?;

        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_batcher(ev_rx, sink));
        let emitter = Emitter {
            session: key.clone(),
            seq: Arc::new(AtomicU64::new(0)),
            tx: ev_tx,
        };
        emitter.emit(
            EventBody::Starting {
                pid: spawned.pid,
                cwd: display_path(cwd),
                args: spawned.args.clone(),
            },
            now_ms(),
        );

        let stdin = spawned.child.stdin.take().expect("stdin piped");
        let stdout = spawned.child.stdout.take().expect("stdout piped");
        let stderr = spawned.child.stderr.take().expect("stderr piped");

        // Écrivain : une ligne JSON par message ; fermer le canal ferme stdin.
        let (stdin_tx, _writer_task) = writer::spawn_writer(stdin);

        // stderr : gardé en mémoire (200 lignes) et relayé.
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_task = {
            let tail = stderr_tail.clone();
            let em = emitter.clone();
            tokio::spawn(async move {
                let mut r = LineReader::new(stderr);
                loop {
                    match r.next_line().await {
                        Ok(None) | Err(_) => break,
                        Ok(Some(raw)) => {
                            let line = match raw {
                                RawLine::Line(l) => l.trim_end().to_string(),
                                RawLine::TooLong { len, head } => {
                                    format!("{head}… [ligne de {len} octets tronquée]")
                                }
                            };
                            {
                                let mut t = tail.lock().unwrap();
                                if t.len() == STDERR_KEEP {
                                    t.pop_front();
                                }
                                t.push_back(line.clone());
                            }
                            em.emit(EventBody::Stderr { line }, now_ms());
                        }
                    }
                }
            })
        };

        let reducer = Arc::new(Mutex::new(Reducer::new()));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (exit_tx, exit_rx) = watch::channel(None);

        // Lecteur de stdout, puis attente de la fin du processus.
        {
            let reducer = reducer.clone();
            let pending = pending.clone();
            let em = emitter.clone();
            // Référence faible : fermer stdin (stop) ne doit pas dépendre du lecteur.
            let writer = stdin_tx.downgrade();
            let tail = stderr_tail.clone();
            let mut child = spawned.child;
            tokio::spawn(async move {
                let mut r = LineReader::new(stdout);
                let mut last_bad_json: Option<Instant> = None;
                loop {
                    let raw = match r.next_line().await {
                        Ok(Some(raw)) => raw,
                        Ok(None) | Err(_) => break,
                    };
                    let t = now_ms();
                    // Ligne trop longue : traitée comme une ligne illisible
                    // (notice Warn limitée à 1 par minute, plan 1.9).
                    let parsed = match &raw {
                        RawLine::Line(text) => {
                            let line = text.trim();
                            if line.is_empty() {
                                continue;
                            }
                            Inbound::from_line(line)
                                .map_err(|e| (e.to_string(), line.chars().take(500).collect()))
                        }
                        RawLine::TooLong { len, head } => Err((
                            format!("ligne de {len} octets au-delà du plafond"),
                            head.clone(),
                        )),
                    };
                    let msg = match parsed {
                        Ok(m) => m,
                        Err((e, line)) => {
                            let now = Instant::now();
                            if last_bad_json.is_none_or(|l| now - l >= Duration::from_secs(60)) {
                                last_bad_json = Some(now);
                                em.emit(
                                    EventBody::Notice {
                                        level: NoticeLevel::Warn,
                                        code: "bad_json".into(),
                                        message: format!("Ligne illisible ignorée : {e}"),
                                        detail: Some(Value::String(line)),
                                    },
                                    t,
                                );
                            }
                            continue;
                        }
                    };
                    let actions = reducer.lock().unwrap().handle_inbound(&msg);
                    apply_actions(actions, &em, &writer, &pending, t);
                }
                let code = child.wait().await.ok().and_then(|s| s.code());
                let during_turn = reducer.lock().unwrap().in_turn();
                // Les requêtes en attente ne recevront jamais de réponse.
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("claude.exe s'est arrêté".into()));
                }
                let stderr_tail: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
                let t = now_ms();
                em.emit(
                    EventBody::Exited {
                        code,
                        during_turn,
                        stderr_tail: stderr_tail.iter().rev().take(20).rev().cloned().collect(),
                    },
                    t,
                );
                // Par le réducteur : `Exited` y devient terminal.
                let mut out = Vec::new();
                reducer
                    .lock()
                    .unwrap()
                    .set_state(SessionState::Exited, &mut out);
                for a in out {
                    if let Action::Emit(b) = a {
                        em.emit(b, t);
                    }
                }
                let _ = exit_tx.send(Some(code));
            });
        }

        let session = Arc::new(Session {
            key,
            pid: spawned.pid,
            claude_path: exe.to_path_buf(),
            cwd: cwd.to_path_buf(),
            stdin_tx: Mutex::new(Some(stdin_tx)),
            reducer,
            pending,
            emitter,
            job: spawned.job,
            exit_rx,
            stderr_tail,
            stderr_task: Mutex::new(Some(stderr_task)),
            next_id: AtomicU64::new(1),
        });

        // initialize (spec 4.7bis) : la réponse produit `ready`.
        let init_id = session.new_request_id("init");
        session.reducer.lock().unwrap().expect_initialize(&init_id);
        match session
            .control_with_id(&init_id, "initialize", Map::new(), INITIALIZE_TIMEOUT)
            .await
        {
            Ok(_) => Ok(session),
            Err(e) => {
                // Tuer l'arbre ferme stderr ; on laisse la tâche de lecture
                // vider ce qui reste avant de construire le message d'erreur.
                session.job.terminate();
                let task = session.stderr_task.lock().unwrap().take();
                if let Some(task) = task {
                    let _ = tokio::time::timeout(STDERR_DRAIN, task).await;
                }
                let tail = session.stderr_tail();
                let detail = if tail.is_empty() {
                    String::new()
                } else {
                    format!(" ; stderr : {}", tail.join(" | "))
                };
                Err(CoreError::SpawnFailed(format!(
                    "pas de réponse à initialize ({e}){detail}"
                )))
            }
        }
    }

    fn new_request_id(&self, prefix: &str) -> String {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{n}-{}", uuid::Uuid::new_v4().simple())
    }

    fn write(&self, line: String) -> Result<(), CoreError> {
        let guard = self.stdin_tx.lock().unwrap();
        let tx = guard.as_ref().ok_or(CoreError::NotRunning)?;
        tx.send(line).map_err(|_| CoreError::NotRunning)
    }

    pub fn is_running(&self) -> bool {
        self.exit_rx.borrow().is_none()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().iter().cloned().collect()
    }

    pub fn version(&self) -> Option<String> {
        self.reducer.lock().unwrap().version.clone()
    }

    pub fn capabilities(&self) -> Vec<String> {
        self.reducer.lock().unwrap().capabilities.clone()
    }

    async fn control_with_id(
        &self,
        request_id: &str,
        subtype: &str,
        extra: Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value, CoreError> {
        if !self.is_running() {
            return Err(CoreError::NotRunning);
        }
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(request_id.to_string(), tx);
        if let Err(e) = self.write(outbound::control_request(request_id, subtype, extra)) {
            self.pending.lock().unwrap().remove(request_id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(msg))) => Err(CoreError::Rejected(msg)),
            Ok(Err(_)) => Err(CoreError::NotRunning),
            Err(_) => {
                self.pending.lock().unwrap().remove(request_id);
                Err(CoreError::Timeout(format!(
                    "{subtype} ({} s)",
                    timeout.as_secs()
                )))
            }
        }
    }

    /// Envoie un message utilisateur (spec 4.2).
    pub fn send_message(&self, text: &str) -> Result<(), CoreError> {
        if !self.is_running() {
            return Err(CoreError::NotRunning);
        }
        if !self.reducer.lock().unwrap().questions.is_empty() {
            return Err(CoreError::QuestionPending);
        }
        self.write(outbound::user_message(text))?;
        let t = now_ms();
        self.emitter.emit(
            EventBody::UserMessage {
                thread: ThreadId::main(),
                uuid: None,
                text: text.to_string(),
                origin: UserOrigin::Typed,
            },
            t,
        );
        self.set_state(SessionState::Requesting, t);
        Ok(())
    }

    fn set_state(&self, st: SessionState, t: u64) {
        self.with_reducer(t, |r, out| r.set_state(st, out));
    }

    /// Exécute `f` sous le verrou du réducteur et émet les événements produits.
    fn with_reducer<R>(&self, t: u64, f: impl FnOnce(&mut Reducer, &mut Vec<Action>) -> R) -> R {
        let mut out = Vec::new();
        let res = f(&mut self.reducer.lock().unwrap(), &mut out);
        for a in out {
            if let Action::Emit(b) = a {
                self.emitter.emit(b, t);
            }
        }
        res
    }

    pub async fn interrupt(&self, cancel_queued: bool) -> Result<InterruptAck, CoreError> {
        // `Interrupting` seulement pendant un tour ; rien après `Exited`.
        let prev = self.with_reducer(now_ms(), |r, out| r.begin_interrupt(out));
        let mut extra = Map::new();
        if cancel_queued {
            extra.insert("cancel_queued".into(), json!(true));
        }
        let id = self.new_request_id("int");
        let res = self
            .control_with_id(&id, "interrupt", extra, CONTROL_TIMEOUT)
            .await;
        let v = match res {
            Ok(v) => v,
            Err(e) => {
                if let Some(prev) = prev {
                    self.with_reducer(now_ms(), |r, out| r.abort_interrupt(prev, out));
                }
                return Err(e);
            }
        };
        Ok(InterruptAck {
            still_queued: v
                .get("still_queued")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| x.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    pub async fn set_model(&self, model: &str) -> Result<String, CoreError> {
        let mut extra = Map::new();
        extra.insert("model".into(), json!(model));
        let id = self.new_request_id("mdl");
        let v = self
            .control_with_id(&id, "set_model", extra, CONTROL_TIMEOUT)
            .await?;
        Ok(v.get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_string())
    }

    pub fn answer_question(
        &self,
        request_id: &str,
        answers: &Answers,
        response: Option<&str>,
    ) -> Result<(), CoreError> {
        send_then_forget_question(&self.reducer, request_id, |input| {
            self.write(outbound::answer_question(
                request_id, input, answers, response,
            ))
        })?;
        self.emitter.emit(
            EventBody::QuestionClosed {
                request_id: request_id.into(),
                reason: QuestionCloseReason::Answered,
            },
            now_ms(),
        );
        Ok(())
    }

    pub fn skip_question(&self, request_id: &str, reason: SkipReason) -> Result<(), CoreError> {
        let supported = self
            .reducer
            .lock()
            .unwrap()
            .capabilities
            .iter()
            .any(|c| c == CAP_UNANSWERED);
        if !supported {
            return Err(CoreError::Unsupported(CAP_UNANSWERED.into()));
        }
        send_then_forget_question(&self.reducer, request_id, |input| {
            self.write(outbound::skip_question(request_id, input, reason))
        })?;
        self.emitter.emit(
            EventBody::QuestionClosed {
                request_id: request_id.into(),
                reason: QuestionCloseReason::Answered,
            },
            now_ms(),
        );
        Ok(())
    }

    /// Ferme stdin, attend 3 s, puis tue l'arbre par le Job Object.
    pub async fn stop(&self) -> ExitInfo {
        self.stdin_tx.lock().unwrap().take();
        let mut rx = self.exit_rx.clone();
        let waited = tokio::time::timeout(STOP_GRACE, rx.wait_for(|x| x.is_some()))
            .await
            .ok()
            .and_then(|r| r.ok().map(|c| c.flatten()));
        if let Some(code) = waited {
            return ExitInfo {
                code,
                killed: false,
            };
        }
        self.job.terminate();
        let code = tokio::time::timeout(Duration::from_secs(2), rx.wait_for(|x| x.is_some()))
            .await
            .ok()
            .and_then(|r| r.ok().map(|c| c.flatten()))
            .flatten();
        ExitInfo { code, killed: true }
    }

    /// Tue immédiatement l'arbre de processus (sans attente).
    pub fn kill(&self) {
        self.job.terminate();
    }
}

/// Répond à une question en attente : la réponse est écrite sur stdin sous le
/// verrou du réducteur (deux réponses simultanées sont impossibles), et la
/// question n'est retirée de la table qu'après un envoi réussi. En cas d'échec,
/// elle reste en attente et peut recevoir une nouvelle réponse.
pub(crate) fn send_then_forget_question(
    reducer: &Mutex<Reducer>,
    request_id: &str,
    send: impl FnOnce(&Value) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mut r = reducer.lock().unwrap();
    let input = r
        .questions
        .get(request_id)
        .ok_or_else(|| CoreError::UnknownRequest(request_id.into()))?;
    send(input)?;
    r.questions.remove(request_id);
    Ok(())
}

fn apply_actions(
    actions: Vec<Action>,
    em: &Emitter,
    writer: &mpsc::WeakUnboundedSender<String>,
    pending: &Pending,
    t: u64,
) {
    for a in actions {
        match a {
            Action::Emit(body) => em.emit(body, t),
            Action::Write(line) => {
                if let Some(w) = writer.upgrade() {
                    let _ = w.send(line);
                }
            }
            Action::Resolve { request_id, result } => {
                if let Some(tx) = pending.lock().unwrap().remove(&request_id) {
                    let _ = tx.send(result);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reducer_with_question(id: &str) -> Mutex<Reducer> {
        let mut r = Reducer::new();
        r.handle(&json!({
            "type": "control_request",
            "request_id": id,
            "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion",
                        "input": {"questions": [{"question": "Q ?", "header": "H",
                                  "options": [], "multiSelect": false}]}}
        }));
        assert!(r.questions.contains_key(id));
        Mutex::new(r)
    }

    #[test]
    fn failed_send_keeps_the_question_pending() {
        let r = reducer_with_question("q1");
        let err = send_then_forget_question(&r, "q1", |_| Err(CoreError::NotRunning));
        assert_eq!(err, Err(CoreError::NotRunning));
        assert!(
            r.lock().unwrap().questions.contains_key("q1"),
            "question gardée"
        );
        // Une nouvelle tentative réussie la retire.
        let mut seen = None;
        send_then_forget_question(&r, "q1", |input| {
            seen = Some(input.clone());
            Ok(())
        })
        .unwrap();
        assert!(
            seen.unwrap()["questions"].is_array(),
            "input transmis à l'envoi"
        );
        assert!(!r.lock().unwrap().questions.contains_key("q1"));
        // Déjà répondue : requête inconnue, rien n'est envoyé.
        let again = send_then_forget_question(&r, "q1", |_| panic!("ne doit pas envoyer"));
        assert_eq!(again, Err(CoreError::UnknownRequest("q1".into())));
    }

    fn status_events(out: &[Action]) -> Vec<SessionState> {
        out.iter()
            .filter_map(|a| match a {
                Action::Emit(EventBody::Status { state }) => Some(*state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn interrupt_only_during_a_turn_and_restores_on_failure() {
        let mut r = Reducer::new();
        let mut out = Vec::new();
        // Hors tour (Idle) : pas de changement d'état.
        r.set_state(SessionState::Idle, &mut out);
        out.clear();
        assert_eq!(r.begin_interrupt(&mut out), None);
        assert!(status_events(&out).is_empty());

        // Pendant un tour : Interrupting, puis retour à Streaming si échec.
        r.set_state(SessionState::Streaming, &mut out);
        out.clear();
        let prev = r.begin_interrupt(&mut out);
        assert_eq!(prev, Some(SessionState::Streaming));
        assert_eq!(status_events(&out), vec![SessionState::Interrupting]);
        out.clear();
        r.abort_interrupt(prev.unwrap(), &mut out);
        assert_eq!(status_events(&out), vec![SessionState::Streaming]);

        // Un `result` arrivé avant l'échec l'emporte : pas de retour arrière.
        let prev = r.begin_interrupt(&mut out).unwrap();
        r.handle(&json!({"type": "result", "subtype": "error_during_execution"}));
        out.clear();
        r.abort_interrupt(prev, &mut out);
        assert!(status_events(&out).is_empty());
        assert_eq!(r.state, Some(SessionState::Idle));
    }

    #[test]
    fn nothing_after_exited() {
        let mut r = Reducer::new();
        let mut out = Vec::new();
        r.set_state(SessionState::Streaming, &mut out);
        r.set_state(SessionState::Exited, &mut out);
        out.clear();
        assert_eq!(r.begin_interrupt(&mut out), None);
        r.set_state(SessionState::Interrupting, &mut out);
        r.set_state(SessionState::Idle, &mut out);
        assert!(status_events(&out).is_empty(), "aucun status après Exited");
        assert_eq!(r.state, Some(SessionState::Exited));
    }
}
