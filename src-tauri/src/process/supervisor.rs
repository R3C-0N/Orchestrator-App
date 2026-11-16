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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use crate::api::events::{
    CoreEvent, EventBody, NoticeLevel, QuestionCloseReason, SessionState, ThreadId, UserOrigin,
};
use crate::api::types::{Answers, CoreError, ExitInfo, InterruptAck, SessionKey, SkipReason};
use crate::process::locate::display_path;
use crate::process::spawn::{build_args, spawn_claude, Job, LaunchOptions};
use crate::protocol::outbound;
use crate::state::batcher::{run_batcher, Sink};
use crate::state::reducer::{Action, Reducer, CAP_UNANSWERED};

pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
pub const STOP_GRACE: Duration = Duration::from_secs(3);
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
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
            drop(stdin);
        });

        // stderr : gardé en mémoire (200 lignes) et relayé.
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        {
            let tail = stderr_tail.clone();
            let em = emitter.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(stderr);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match r.read_until(b'\n', &mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
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
            });
        }

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
                let mut r = BufReader::new(stdout);
                let mut buf = Vec::new();
                let mut last_bad_json: Option<Instant> = None;
                loop {
                    buf.clear();
                    match r.read_until(b'\n', &mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let t = now_ms();
                    let text = String::from_utf8_lossy(&buf);
                    let line = text.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let v: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(e) => {
                            let now = Instant::now();
                            if last_bad_json.is_none_or(|l| now - l >= Duration::from_secs(60)) {
                                last_bad_json = Some(now);
                                em.emit(
                                    EventBody::Notice {
                                        level: NoticeLevel::Warn,
                                        code: "bad_json".into(),
                                        message: format!("Ligne illisible ignorée : {e}"),
                                        detail: Some(Value::String(
                                            line.chars().take(500).collect(),
                                        )),
                                    },
                                    t,
                                );
                            }
                            continue;
                        }
                    };
                    let actions = reducer.lock().unwrap().handle(&v);
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
                em.emit(
                    EventBody::Status {
                        state: SessionState::Exited,
                    },
                    t,
                );
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
                let tail = session.stderr_tail();
                session.job.terminate();
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
        let mut out = Vec::new();
        self.reducer.lock().unwrap().set_state(st, &mut out);
        for a in out {
            if let Action::Emit(b) = a {
                self.emitter.emit(b, t);
            }
        }
    }

    pub async fn interrupt(&self, cancel_queued: bool) -> Result<InterruptAck, CoreError> {
        self.set_state(SessionState::Interrupting, now_ms());
        let mut extra = Map::new();
        if cancel_queued {
            extra.insert("cancel_queued".into(), json!(true));
        }
        let id = self.new_request_id("int");
        let v = self
            .control_with_id(&id, "interrupt", extra, CONTROL_TIMEOUT)
            .await?;
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
        let input = self
            .reducer
            .lock()
            .unwrap()
            .questions
            .remove(request_id)
            .ok_or_else(|| CoreError::UnknownRequest(request_id.into()))?;
        self.write(outbound::answer_question(
            request_id, &input, answers, response,
        ))?;
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
        let mut r = self.reducer.lock().unwrap();
        if !r.capabilities.iter().any(|c| c == CAP_UNANSWERED) {
            return Err(CoreError::Unsupported(CAP_UNANSWERED.into()));
        }
        let input = r
            .questions
            .remove(request_id)
            .ok_or_else(|| CoreError::UnknownRequest(request_id.into()))?;
        drop(r);
        self.write(outbound::skip_question(request_id, &input, reason))?;
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
