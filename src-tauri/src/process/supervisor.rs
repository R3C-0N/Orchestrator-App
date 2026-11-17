//! Une session `claude.exe` : lancement, écriture sur stdin, lecture de stdout
//! et stderr ligne à ligne, corrélation des requêtes de contrôle, arrêt.
//!
//! Indépendant de Tauri : les événements sortent par un [`Sink`] (Channel Tauri
//! dans l'app, impression JSON dans le binaire `probe` et les tests).
//!
//! Supervision (plan 1.9, J6) : la fin du processus est détectée par
//! `child.wait()` en concurrence avec la lecture de stdout (`select!`), et non
//! par la fin de stdout, qu'un petit-enfant peut garder ouverte. Après la
//! sortie, stdout est encore lu pendant [`SessionConfig::exit_flush`], puis le
//! Job Object est terminé : tout l'arbre meurt et les tuyaux se ferment.
//!
//! Sessions rattachables (J14, décision 15) : chaque événement est numéroté et
//! gardé dans le journal de la session ([`Outlet`]) avant de partir vers la
//! destination courante. La destination est remplaçable : un rechargement de
//! l'interface la détache (les événements ne vont plus qu'au journal), et la
//! nouvelle page s'y rattache en recevant le journal complet
//! ([`Outlet::attach`]), sans trou ni doublon de `seq`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::api::events::{CoreEvent, EventBody, QuestionCloseReason, SessionState};
use crate::api::types::{
    Answers, CoreError, ExitInfo, HistoryLoaded, InterruptAck, SessionKey, SessionResync,
    SessionRole, SkipReason,
};
use crate::errors::{self, NoticeLimiter};
use crate::logging::{self, RawLog};
use crate::process::locate::display_path;
use crate::process::reader::{LineReader, RawLine, MAX_LINE_BYTES};
use crate::process::spawn::{
    build_args, spawn_claude_env, validate_launch_options, Job, LaunchOptions,
};
use crate::process::writer;
use crate::protocol::inbound::Inbound;
use crate::protocol::outbound;
use crate::state::batcher::{run_batcher, Sink};
use crate::state::reducer::{Action, Reducer, CAP_UNANSWERED};

pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// Attente maximale de la fin de lecture de stderr après la fin du processus
/// ou un échec de démarrage.
pub const STDERR_DRAIN: Duration = Duration::from_secs(1);
/// Lecture de stdout encore permise après la sortie du processus, avant de
/// terminer le Job (les dernières lignes écrites juste avant la sortie).
pub const EXIT_FLUSH: Duration = Duration::from_millis(500);
/// Délai entre l'acceptation d'un `interrupt` et le `result` attendu.
pub const INTERRUPT_RESULT_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_KEEP: usize = 200;
/// Lignes de stderr jointes à `exited`.
const EXIT_STDERR_TAIL: usize = 20;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Délais et plafonds d'une session. `Default` = valeurs du plan ; les tests
/// les raccourcissent.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub initialize_timeout: Duration,
    pub control_timeout: Duration,
    pub interrupt_result_timeout: Duration,
    pub stop_grace: Duration,
    pub exit_flush: Duration,
    pub max_line_bytes: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            initialize_timeout: INITIALIZE_TIMEOUT,
            control_timeout: CONTROL_TIMEOUT,
            interrupt_result_timeout: INTERRUPT_RESULT_TIMEOUT,
            stop_grace: STOP_GRACE,
            exit_flush: EXIT_FLUSH,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }
}

/// Destination d'une session attachée à une page de l'interface : le `Sink`
/// et la génération de page (`AppState::page_gen`) qui l'a fournie.
#[derive(Clone)]
pub struct Attachment {
    pub sink: Sink,
    pub page_gen: u64,
}

/// Destination courante des événements d'une session.
struct Route {
    sink: Option<Sink>,
    /// Premier `seq` que `sink` doit recevoir : les précédents sont dans
    /// l'instantané rendu par [`Outlet::attach`] (pas de doublon).
    from_seq: u64,
    /// Génération de page de `sink`.
    page_gen: u64,
    /// Numéro de rattachement : un échec d'envoi ne détache que le `sink`
    /// qui a échoué, pas un `sink` posé depuis.
    epoch: u64,
    /// Depuis quand la session n'a plus de destination (détachement, ou
    /// création sans destination) ; `None` si attachée.
    detached_since: Option<Instant>,
}

struct Journal {
    events: Vec<CoreEvent>,
    tx: mpsc::UnboundedSender<CoreEvent>,
    turns: u64,
}

/// Journal des `CoreEvent` d'une session et destination remplaçable (J14).
///
/// Ordre garanti : un événement reçoit son `seq` (= sa place dans le
/// journal), entre au journal et part vers le regroupeur sous le même verrou ;
/// [`Outlet::attach`] prend l'instantané du journal et change de destination
/// sous ce même verrou. Tout événement absent de l'instantané passe donc
/// ensuite par le regroupeur vers la nouvelle destination, et tout événement
/// de l'instantané encore en route est écarté (`from_seq`). Le verrou unique
/// garantit aussi que les `seq` entrent dans le regroupeur dans l'ordre, même
/// émis par des tâches différentes (stderr, supervision, commandes).
pub struct Outlet {
    session: SessionKey,
    journal: Mutex<Journal>,
    route: Mutex<Route>,
    /// Nombre d'événements émis ; réveille les attentes (`Session::wait_*`).
    activity: watch::Sender<u64>,
    /// Appelé (hors verrou) quand un envoi refusé détache la session.
    on_detach: Mutex<Option<DetachHook>>,
}

/// Rappel de détachement (fermeture différée, `AppState`).
pub type DetachHook = Arc<dyn Fn() + Send + Sync>;

impl Outlet {
    /// Crée le journal et lance son regroupeur (runtime tokio requis).
    /// `attachment` : destination initiale ; `None` : journal seul.
    pub fn spawn(session: SessionKey, attachment: Option<Attachment>) -> Arc<Outlet> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Sans destination au départ, la session compte comme détachée dès
        // maintenant : fermée si aucune page ne s'y rattache dans le délai.
        let (page_gen, sink, detached_since) = match attachment {
            Some(a) => (a.page_gen, Some(a.sink), None),
            None => (0, None, Some(Instant::now())),
        };
        let outlet = Arc::new(Outlet {
            session,
            journal: Mutex::new(Journal {
                events: Vec::new(),
                tx,
                turns: 0,
            }),
            route: Mutex::new(Route {
                sink,
                from_seq: 0,
                page_gen,
                epoch: 0,
                detached_since,
            }),
            activity: watch::channel(0).0,
            on_detach: Mutex::new(None),
        });
        // Référence faible : le regroupeur s'arrête quand le journal (donc
        // l'émetteur) disparaît.
        let weak = Arc::downgrade(&outlet);
        let deliver: Sink = Arc::new(move |batch| {
            if let Some(o) = weak.upgrade() {
                o.deliver(batch);
            }
            // Toujours vrai : une destination fermée n'arrête pas le
            // regroupeur, la session peut être rattachée plus tard.
            true
        });
        tokio::spawn(run_batcher(rx, deliver));
        outlet
    }

    /// Numérote, horodate, journalise et envoie un événement ; renvoie son `seq`.
    pub fn emit(&self, body: EventBody, t: u64) -> u64 {
        let mut j = self.journal.lock().unwrap();
        let seq = j.events.len() as u64;
        if matches!(body, EventBody::TurnResult { .. }) {
            j.turns += 1;
        }
        let ev = CoreEvent {
            seq,
            t,
            session: self.session.clone(),
            body,
        };
        j.events.push(ev.clone());
        let _ = j.tx.send(ev);
        self.activity.send_replace(seq + 1);
        seq
    }

    /// Lot sorti du regroupeur : vers la destination courante, sans les
    /// événements déjà rendus par l'instantané du dernier rattachement. Le
    /// `sink` est appelé hors verrou : un lot lu avec l'ancienne destination
    /// ne contient que des événements émis avant le rattachement, donc
    /// présents dans son instantané.
    fn deliver(&self, batch: Vec<CoreEvent>) {
        let (sink, from, epoch) = {
            let r = self.route.lock().unwrap();
            match &r.sink {
                Some(s) => (s.clone(), r.from_seq, r.epoch),
                None => return,
            }
        };
        let batch: Vec<CoreEvent> = if batch.first().is_some_and(|e| e.seq < from) {
            batch.into_iter().filter(|e| e.seq >= from).collect()
        } else {
            batch
        };
        if batch.is_empty() || sink(batch) {
            return;
        }
        {
            let mut r = self.route.lock().unwrap();
            if r.epoch != epoch || r.sink.is_none() {
                return;
            }
            tracing::warn!(session = %self.session.0, "destination fermée : session détachée");
            r.sink = None;
            r.detached_since = Some(Instant::now());
        }
        let hook = self.on_detach.lock().unwrap().clone();
        if let Some(f) = hook {
            f();
        }
    }

    /// Rappel appelé quand un envoi refusé détache la session.
    pub fn set_on_detach(&self, f: DetachHook) {
        *self.on_detach.lock().unwrap() = Some(f);
    }

    /// Rattache la session à `sink` et renvoie le journal complet. Les
    /// événements suivants partent vers `sink`, à partir du `seq` qui suit
    /// l'instantané.
    pub fn attach(&self, sink: Sink, page_gen: u64) -> Vec<CoreEvent> {
        let j = self.journal.lock().unwrap();
        let snapshot = j.events.clone();
        let mut r = self.route.lock().unwrap();
        r.sink = Some(sink);
        r.from_seq = snapshot.len() as u64;
        r.page_gen = page_gen;
        r.epoch += 1;
        r.detached_since = None;
        snapshot
    }

    /// Détache la session si sa destination vient d'une génération de page
    /// antérieure à `page_gen`. Vrai si elle vient d'être détachée.
    pub fn detach_before(&self, page_gen: u64) -> bool {
        let mut r = self.route.lock().unwrap();
        if r.sink.is_none() || r.page_gen >= page_gen {
            return false;
        }
        r.sink = None;
        r.epoch += 1;
        r.detached_since = Some(Instant::now());
        true
    }

    pub fn is_attached(&self) -> bool {
        self.route.lock().unwrap().sink.is_some()
    }

    /// Durée depuis le détachement (ou depuis la création, si créée sans
    /// destination) ; `None` si attachée.
    pub fn detached_for(&self) -> Option<Duration> {
        self.route
            .lock()
            .unwrap()
            .detached_since
            .map(|t| t.elapsed())
    }

    /// Nombre d'événements émis (= `seq` du prochain).
    pub fn len(&self) -> u64 {
        self.journal.lock().unwrap().events.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copie du journal.
    pub fn events(&self) -> Vec<CoreEvent> {
        self.journal.lock().unwrap().events.clone()
    }

    /// Tours terminés (`turn_result` émis).
    pub fn turns(&self) -> u64 {
        self.journal.lock().unwrap().turns
    }

    /// Réveillé à chaque événement émis.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.activity.subscribe()
    }
}

/// Émetteur des événements d'une session : son journal.
#[derive(Clone)]
struct Emitter(Arc<Outlet>);

/// Issue de [`Session::wait_turn_end`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnWait {
    /// Tour terminé : nombre de tours terminés depuis le démarrage.
    Done(u64),
    /// Le processus s'est arrêté avant la fin du tour.
    Exited,
    Timeout,
}

impl Emitter {
    fn emit(&self, body: EventBody, t: u64) {
        self.0.emit(body, t);
    }

    fn emit_actions(&self, out: Vec<Action>, t: u64) {
        for a in out {
            if let Action::Emit(b) = a {
                self.emit(b, t);
            }
        }
    }
}

pub struct Session {
    pub key: SessionKey,
    pub pid: u32,
    pub claude_path: PathBuf,
    pub cwd: PathBuf,
    pub role: SessionRole,
    /// `session_id` passé à `--resume`.
    pub resumed_from: Option<String>,
    /// Heure de démarrage (ms) : ordre des sessions dans `attach_sessions`.
    pub started_at: u64,
    /// Historique repris, gardé au premier `load_history` (J14) : rendu tel
    /// quel au rattachement et aux appels suivants, même si le transcript a
    /// grossi depuis (sinon les tours de la session apparaîtraient deux fois).
    history: Mutex<Option<HistoryLoaded>>,
    config: SessionConfig,
    stdin_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    reducer: Arc<Mutex<Reducer>>,
    pending: Pending,
    emitter: Emitter,
    job: Arc<Job>,
    exit_rx: watch::Receiver<Option<Option<i32>>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Vrai dès que l'app demande l'arrêt : la sortie n'est plus une alerte.
    stopping: Arc<AtomicBool>,
    /// Numéro de la dernière interruption (garde de son délai de `result`).
    interrupt_gen: Arc<AtomicU64>,
    raw: Option<Arc<RawLog>>,
    next_id: AtomicU64,
}

/// Ce que la tâche de supervision partage avec la session.
struct Supervision {
    reducer: Arc<Mutex<Reducer>>,
    pending: Pending,
    em: Emitter,
    /// Référence faible : fermer stdin (stop) ne doit pas dépendre du lecteur.
    writer: mpsc::WeakUnboundedSender<String>,
    job: Arc<Job>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_task: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    raw: Option<Arc<RawLog>>,
    exit_tx: watch::Sender<Option<Option<i32>>>,
    exit_flush: Duration,
    max_line_bytes: usize,
    key: SessionKey,
    pid: u32,
}

impl Supervision {
    /// Boucle de supervision : lit stdout et attend la fin du processus en
    /// même temps. Émet `exited` dès que le processus est mort, même si un
    /// petit-enfant garde stdout ouvert.
    async fn run(mut self, mut child: Child, stdout: ChildStdout) {
        let mut r = LineReader::with_max(stdout, self.max_line_bytes);
        let mut limiter = NoticeLimiter::default();
        let mut stdout_open = true;
        let status = {
            let wait = child.wait();
            tokio::pin!(wait);
            loop {
                if !stdout_open {
                    break (&mut wait).await;
                }
                tokio::select! {
                    // Les lignes déjà écrites passent avant la fin du processus.
                    biased;
                    line = r.next_line() => match line {
                        Ok(Some(raw)) => self.on_line(raw, &mut limiter),
                        Ok(None) => {
                            tracing::debug!(session = %self.key.0, "fin de stdout");
                            stdout_open = false;
                        }
                        Err(e) => {
                            tracing::warn!(session = %self.key.0, "lecture de stdout : {e}");
                            stdout_open = false;
                        }
                    },
                    st = &mut wait => break st,
                }
            }
        };
        let code = match &status {
            Ok(s) => s.code(),
            Err(e) => {
                tracing::warn!(session = %self.key.0, "attente du processus : {e}");
                None
            }
        };

        // Dernières lignes écrites juste avant la sortie ; au-delà du délai,
        // stdout est tenu par un autre processus de l'arbre.
        if stdout_open {
            let deadline = tokio::time::Instant::now() + self.exit_flush;
            loop {
                match tokio::time::timeout_at(deadline, r.next_line()).await {
                    Ok(Ok(Some(raw))) => self.on_line(raw, &mut limiter),
                    Ok(Ok(None)) | Ok(Err(_)) => break,
                    Err(_) => {
                        tracing::warn!(
                            session = %self.key.0,
                            "stdout encore ouvert {} ms après la sortie : un processus de l'arbre le tient",
                            self.exit_flush.as_millis()
                        );
                        break;
                    }
                }
            }
        }
        // Finir l'arbre (bash, serveurs MCP, petits-enfants) : ferme les tuyaux.
        self.job.terminate();
        drop(r);

        // stderr complet avant `exited` (dernier message d'erreur du CLI).
        if let Some(mut task) = self.stderr_task.take() {
            if tokio::time::timeout(STDERR_DRAIN, &mut task).await.is_err() {
                // stderr tenu au-delà du délai : la lecture est abandonnée,
                // plus aucune ligne `stderr` après `exited`.
                task.abort();
            }
        }

        let during_turn = self.reducer.lock().unwrap().in_turn();
        // Lu avant de libérer les requêtes en attente : un échec d'`initialize`
        // causé par cette mort ne doit pas la faire passer pour un arrêt voulu.
        let stopping = self.stopping.load(Ordering::SeqCst);
        // Les requêtes en attente ne recevront jamais de réponse.
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err("claude.exe s'est arrêté".into()));
        }
        let tail: Vec<String> = {
            let t = self.stderr_tail.lock().unwrap();
            let skip = t.len().saturating_sub(EXIT_STDERR_TAIL);
            t.iter().skip(skip).cloned().collect()
        };
        tracing::info!(
            session = %self.key.0,
            pid = self.pid,
            ?code,
            during_turn,
            requested = stopping,
            "claude.exe arrêté"
        );
        let t = now_ms();
        {
            // Sous le verrou du réducteur : tout événement émis ailleurs passe
            // aussi par ce verrou (`Session::with_reducer`), donc rien ne peut
            // s'intercaler, et le réducteur refuse tout changement après.
            let mut r = self.reducer.lock().unwrap();
            // Code non nul sans arrêt demandé : alerte (plan 1.9), avant
            // `exited`. Code 0 après fermeture de stdin : arrêt normal.
            if code != Some(0) && !stopping {
                self.em.emit(errors::exit_nonzero(code, &tail), t);
            }
            // Agents non terminés -> `Stopped`, `exited`, puis `status Exited`.
            let out = r.exited_at(code, tail, t);
            self.em.emit_actions(out, t);
        }
        if let Some(raw) = &self.raw {
            raw.exit(code);
        }
        let _ = self.exit_tx.send(Some(code));
    }

    fn on_line(&self, raw: RawLine, limiter: &mut NoticeLimiter) {
        let t = now_ms();
        match raw {
            RawLine::Line(text) => {
                let line = text.trim();
                if line.is_empty() {
                    return;
                }
                if let Some(log) = &self.raw {
                    log.cli(line);
                }
                match Inbound::from_line(line) {
                    Ok(msg) => {
                        for k in msg.unknown_kinds() {
                            tracing::info!(session = %self.key.0, kind = %k, "type inconnu ignoré");
                        }
                        let mut r = self.reducer.lock().unwrap();
                        let actions = r.handle_inbound_at(&msg, t);
                        apply_actions(actions, &self.em, &self.writer, &self.pending, &self.raw, t);
                        drop(r);
                    }
                    Err(e) => {
                        tracing::warn!(
                            session = %self.key.0,
                            error = %e,
                            len = line.len(),
                            head = %errors::head(line),
                            "ligne illisible"
                        );
                        if limiter.allow(errors::BAD_JSON, Instant::now()) {
                            let n = limiter.take_suppressed(errors::BAD_JSON);
                            self.em.emit(errors::bad_json(&e.to_string(), line, n), t);
                        }
                    }
                }
            }
            RawLine::TooLong { len, head } => {
                tracing::warn!(
                    session = %self.key.0,
                    len,
                    max = self.max_line_bytes,
                    head = %head,
                    "ligne trop longue ignorée"
                );
                if limiter.allow(errors::LINE_TOO_LONG, Instant::now()) {
                    let n = limiter.take_suppressed(errors::LINE_TOO_LONG);
                    self.em
                        .emit(errors::line_too_long(len, self.max_line_bytes, &head, n), t);
                }
            }
        }
    }
}

/// Lit stderr ligne à ligne : 200 lignes en mémoire, toutes au journal, chacune
/// relayée (`stderr`).
fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    tail: Arc<Mutex<VecDeque<String>>>,
    em: Emitter,
    raw: Option<Arc<RawLog>>,
    key: SessionKey,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut r = LineReader::new(stderr);
        loop {
            match r.next_line().await {
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(session = %key.0, "lecture de stderr : {e}");
                    break;
                }
                Ok(Some(raw_line)) => {
                    let line = match raw_line {
                        RawLine::Line(l) => l.trim_end().to_string(),
                        RawLine::TooLong { len, head } => {
                            format!("{head}… [ligne de {len} octets tronquée]")
                        }
                    };
                    tracing::info!(target: "claude_stderr", session = %key.0, "{line}");
                    if let Some(log) = &raw {
                        log.stderr(&line);
                    }
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
}

/// Surveille la tâche d'écriture : un tuyau stdin cassé laisse une trace au
/// journal, et une `notice` si le processus tourne encore (sinon c'est la
/// conséquence de sa mort, déjà signalée par `exited`).
fn watch_writer(
    task: JoinHandle<std::io::Result<()>>,
    mut exit_rx: watch::Receiver<Option<Option<i32>>>,
    em: Emitter,
    reducer: Arc<Mutex<Reducer>>,
    stopping: Arc<AtomicBool>,
    key: SessionKey,
) {
    tokio::spawn(async move {
        match task.await {
            Ok(Ok(())) => tracing::debug!(session = %key.0, "stdin fermé"),
            Ok(Err(e)) => {
                tracing::warn!(session = %key.0, "écriture sur stdin en échec : {e}");
                // Laisser à la supervision le temps de voir une mort éventuelle.
                let exited = tokio::time::timeout(
                    Duration::from_millis(500),
                    exit_rx.wait_for(|x| x.is_some()),
                )
                .await
                .is_ok();
                let r = reducer.lock().unwrap();
                if !exited
                    && !stopping.load(Ordering::SeqCst)
                    && r.state != Some(SessionState::Exited)
                {
                    em.emit(errors::stdin_broken(&e.to_string()), now_ms());
                }
            }
            Err(e) => tracing::error!(session = %key.0, "tâche d'écriture interrompue : {e}"),
        }
    });
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
        Self::start_with(key, exe, cwd, opts, sink, SessionConfig::default()).await
    }

    /// Comme [`Session::start`], avec des délais et plafonds choisis.
    pub async fn start_with(
        key: SessionKey,
        exe: &Path,
        cwd: &Path,
        opts: LaunchOptions,
        sink: Sink,
        config: SessionConfig,
    ) -> Result<Arc<Session>, CoreError> {
        let attachment = Attachment { sink, page_gen: 0 };
        Self::start_role(
            key,
            exe,
            cwd,
            opts,
            SessionRole::Main,
            Some(attachment),
            config,
        )
        .await
    }

    /// Lancement complet (J14) : rôle de la session et destination initiale
    /// de ses événements (`None` : journal seul, jusqu'au premier
    /// rattachement).
    pub async fn start_role(
        key: SessionKey,
        exe: &Path,
        cwd: &Path,
        opts: LaunchOptions,
        role: SessionRole,
        attachment: Option<Attachment>,
        config: SessionConfig,
    ) -> Result<Arc<Session>, CoreError> {
        if !cwd.is_dir() {
            return Err(CoreError::BadCwd(display_path(cwd)));
        }
        validate_launch_options(&opts).map_err(CoreError::InvalidArgument)?;
        let args = build_args(&opts);
        let mut spawned = spawn_claude_env(exe, cwd, &args, &opts.env, opts.env_allow.as_deref())
            .map_err(|e| {
            tracing::error!(exe = %exe.display(), "échec du lancement : {e}");
            CoreError::SpawnFailed(e.to_string())
        })?;
        tracing::info!(
            session = %key.0,
            pid = spawned.pid,
            exe = %exe.display(),
            cwd = %cwd.display(),
            args = ?spawned.args,
            "claude.exe lancé"
        );

        let raw = logging::raw_dir().and_then(|dir| {
            RawLog::create(&dir, &key.0, now_ms())
                .map_err(|e| tracing::warn!("flux brut non copié : {e}"))
                .ok()
                .map(Arc::new)
        });

        let resumed_from = opts.resume.clone();
        let emitter = Emitter(Outlet::spawn(key.clone(), attachment));
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

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_task = Some(spawn_stderr_reader(
            stderr,
            stderr_tail.clone(),
            emitter.clone(),
            raw.clone(),
            key.clone(),
        ));

        let reducer = Arc::new(Mutex::new(Reducer::new()));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (exit_tx, exit_rx) = watch::channel(None);
        let stopping = Arc::new(AtomicBool::new(false));
        let job = Arc::new(spawned.job);

        // Écrivain : une ligne JSON par message ; fermer le canal ferme stdin.
        let (stdin_tx, writer_task) = writer::spawn_writer(stdin);
        watch_writer(
            writer_task,
            exit_rx.clone(),
            emitter.clone(),
            reducer.clone(),
            stopping.clone(),
            key.clone(),
        );

        let sup = Supervision {
            reducer: reducer.clone(),
            pending: pending.clone(),
            em: emitter.clone(),
            writer: stdin_tx.downgrade(),
            job: job.clone(),
            stderr_tail: stderr_tail.clone(),
            stderr_task,
            stopping: stopping.clone(),
            raw: raw.clone(),
            exit_tx,
            exit_flush: config.exit_flush,
            max_line_bytes: config.max_line_bytes,
            key: key.clone(),
            pid: spawned.pid,
        };
        tokio::spawn(sup.run(spawned.child, stdout));

        let session = Arc::new(Session {
            key,
            pid: spawned.pid,
            claude_path: exe.to_path_buf(),
            cwd: cwd.to_path_buf(),
            role,
            resumed_from,
            started_at: now_ms(),
            history: Mutex::new(None),
            config,
            stdin_tx: Mutex::new(Some(stdin_tx)),
            reducer,
            pending,
            emitter,
            job,
            exit_rx,
            stderr_tail,
            stopping,
            interrupt_gen: Arc::new(AtomicU64::new(0)),
            raw,
            next_id: AtomicU64::new(1),
        });

        // initialize (spec 4.7bis) : la réponse produit `ready`.
        let init_id = session.new_request_id("init");
        session.reducer.lock().unwrap().expect_initialize(&init_id);
        let timeout = session.config.initialize_timeout;
        match session
            .control_with_id(&init_id, "initialize", Map::new(), timeout)
            .await
        {
            Ok(_) => Ok(session),
            Err(e) => {
                // Arrêt voulu par l'app : pas d'alerte « code non nul » en plus
                // de l'erreur rendue ici, sauf si le processus était déjà mort.
                if session.is_running() {
                    session.stopping.store(true, Ordering::SeqCst);
                }
                // Tuer l'arbre ferme stdout et stderr ; on attend la fin de la
                // supervision (stderr vidé) avant de construire le message.
                session.job.terminate();
                let mut rx = session.exit_rx.clone();
                let _ = tokio::time::timeout(
                    STDERR_DRAIN + session.config.exit_flush,
                    rx.wait_for(|x| x.is_some()),
                )
                .await;
                let tail = session.stderr_tail();
                let detail = if tail.is_empty() {
                    String::new()
                } else {
                    let skip = tail.len().saturating_sub(EXIT_STDERR_TAIL);
                    format!(" ; stderr : {}", tail[skip..].join(" | "))
                };
                tracing::error!(session = %session.key.0, "échec du démarrage : {e}{detail}");
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
        if let Some(raw) = &self.raw {
            raw.sent(&line);
        }
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

    /// Lignes de type inconnu ignorées par le réducteur (plan 1.9).
    pub fn unknown_count(&self) -> u64 {
        self.reducer.lock().unwrap().unknown_count
    }

    /// État courant tel que le voit le réducteur.
    pub fn state(&self) -> Option<SessionState> {
        self.reducer.lock().unwrap().state
    }

    /// Code de sortie, une fois le processus arrêté : `Some(code)`.
    pub fn exit_code(&self) -> Option<Option<i32>> {
        *self.exit_rx.borrow()
    }

    /// Attend la fin du processus (et de sa supervision) ; `None` au-delà du délai.
    pub async fn wait_exit(&self, within: Duration) -> Option<Option<i32>> {
        let mut rx = self.exit_rx.clone();
        tokio::time::timeout(within, rx.wait_for(|x| x.is_some()))
            .await
            .ok()
            .and_then(|r| r.ok().map(|c| *c))
            .flatten()
    }

    /// Attend la fin du processus, sans délai ; renvoie son code.
    pub async fn exited(&self) -> Option<i32> {
        let mut rx = self.exit_rx.clone();
        let code = rx.wait_for(|x| x.is_some()).await.ok().and_then(|c| *c);
        code.flatten()
    }

    /// Fichier de copie du flux brut, si le journal est actif.
    pub fn raw_log_path(&self) -> Option<&Path> {
        self.raw.as_deref().map(RawLog::path)
    }

    /// Journal et destination des événements (J14).
    pub fn outlet(&self) -> &Arc<Outlet> {
        &self.emitter.0
    }

    /// Rattache la session à `sink` (page `page_gen`) et rend de quoi
    /// reconstruire son état : historique repris et journal complet.
    pub fn resync(&self, sink: Sink, page_gen: u64) -> SessionResync {
        let events = self.outlet().attach(sink, page_gen);
        SessionResync {
            session: self.key.clone(),
            role: self.role,
            cwd: display_path(&self.cwd),
            pid: self.pid,
            resumed_from: self.resumed_from.clone(),
            history: self.history.lock().unwrap().clone(),
            events,
            running: self.is_running(),
        }
    }

    /// Historique repris déjà chargé pour `session_id`, s'il y en a un.
    pub fn cached_history(&self, session_id: &str) -> Option<HistoryLoaded> {
        self.history
            .lock()
            .unwrap()
            .as_ref()
            .filter(|h| h.session_id == session_id)
            .cloned()
    }

    /// Garde le premier historique chargé ; renvoie celui qui est gardé (un
    /// chargement concurrent arrivé avant l'emporte).
    pub fn keep_history(&self, h: HistoryLoaded) -> HistoryLoaded {
        let mut slot = self.history.lock().unwrap();
        match slot.as_ref() {
            Some(kept) if kept.session_id == h.session_id => kept.clone(),
            Some(_) => h,
            None => {
                *slot = Some(h.clone());
                h
            }
        }
    }

    /// Au repos (décision 15) : pas de tour en cours et pas de question
    /// ouverte. Une session arrêtée est au repos.
    pub fn is_idle(&self) -> bool {
        let r = self.reducer.lock().unwrap();
        !r.in_turn() && r.questions.is_empty()
    }

    /// Attend le repos (ou la fin du processus) ; faux au-delà du délai.
    pub async fn wait_idle(&self, within: Duration) -> bool {
        let mut rx = self.outlet().subscribe();
        tokio::time::timeout(within, async {
            loop {
                if self.is_idle() || !self.is_running() {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    }

    /// Attend qu'au moins `after + 1` tours soient terminés (`turns()` lu
    /// avant d'envoyer le message, par exemple).
    pub async fn wait_turn_end(&self, after: u64, within: Duration) -> TurnWait {
        let mut rx = self.outlet().subscribe();
        let mut exit = self.exit_rx.clone();
        let wait = async {
            loop {
                let n = self.outlet().turns();
                if n > after {
                    return TurnWait::Done(n);
                }
                if !self.is_running() {
                    return TurnWait::Exited;
                }
                tokio::select! {
                    r = rx.changed() => if r.is_err() { return TurnWait::Exited },
                    _ = exit.changed() => {},
                }
            }
        };
        tokio::time::timeout(within, wait)
            .await
            .unwrap_or(TurnWait::Timeout)
    }

    /// Tours terminés depuis le démarrage.
    pub fn turns(&self) -> u64 {
        self.outlet().turns()
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
        let mut r = self.reducer.lock().unwrap();
        if r.state == Some(SessionState::Exited) {
            return Err(CoreError::NotRunning);
        }
        if !r.questions.is_empty() {
            return Err(CoreError::QuestionPending);
        }
        self.write(outbound::user_message(text))?;
        let out = r.user_sent(text);
        self.emitter.emit_actions(out, now_ms());
        Ok(())
    }

    /// Exécute `f` sous le verrou du réducteur et émet les événements produits
    /// **sous ce verrou** : l'ordre des `seq` suit celui du réducteur, et rien
    /// ne sort après `Exited` (émis lui aussi sous ce verrou).
    fn with_reducer<R>(&self, t: u64, f: impl FnOnce(&mut Reducer, &mut Vec<Action>) -> R) -> R {
        let mut out = Vec::new();
        let mut r = self.reducer.lock().unwrap();
        let res = f(&mut r, &mut out);
        self.emitter.emit_actions(out, t);
        res
    }

    pub async fn interrupt(&self, cancel_queued: bool) -> Result<InterruptAck, CoreError> {
        // `Interrupting` seulement pendant un tour ; rien après `Exited`. Le
        // numéro de garde n'avance que si l'état vient de passer en
        // `Interrupting` : un second Échap pendant `Interrupting` ne désarme
        // pas la garde de la première interruption (relecture J6, B1).
        let gens = self.interrupt_gen.clone();
        let (prev, gen) = self.with_reducer(now_ms(), |r, out| {
            let prev = r.begin_interrupt(out);
            let gen = if prev.is_some() {
                gens.fetch_add(1, Ordering::SeqCst) + 1
            } else {
                gens.load(Ordering::SeqCst)
            };
            (prev, gen)
        });
        let mut extra = Map::new();
        if cancel_queued {
            extra.insert("cancel_queued".into(), json!(true));
        }
        let id = self.new_request_id("int");
        let res = self
            .control_with_id(&id, "interrupt", extra, self.config.control_timeout)
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
        if let Some(prev) = prev {
            self.watch_interrupt(gen, prev);
        }
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

    /// Interruption acceptée : si aucun `result` ne suit dans le délai, l'état
    /// ne reste pas bloqué sur `Interrupting` ; il revient à l'état d'avant
    /// (le tour continue, on peut réessayer) avec une `notice`. Une nouvelle
    /// interruption entre-temps annule cette garde.
    fn watch_interrupt(&self, gen: u64, prev: SessionState) {
        let reducer = self.reducer.clone();
        let em = self.emitter.clone();
        let current = self.interrupt_gen.clone();
        let wait = self.config.interrupt_result_timeout;
        let key = self.key.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if current.load(Ordering::SeqCst) != gen {
                return;
            }
            let t = now_ms();
            let mut out = Vec::new();
            let mut r = reducer.lock().unwrap();
            if r.state == Some(SessionState::Interrupting) {
                tracing::warn!(session = %key.0, "interrupt accepté sans result après {wait:?}");
                em.emit(errors::interrupt_no_result(wait), t);
                r.abort_interrupt(prev, &mut out);
                em.emit_actions(out, t);
            }
        });
    }

    pub async fn set_model(&self, model: &str) -> Result<String, CoreError> {
        let mut extra = Map::new();
        extra.insert("model".into(), json!(model));
        let id = self.new_request_id("mdl");
        let v = self
            .control_with_id(&id, "set_model", extra, self.config.control_timeout)
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
        send_then_close_question(
            &self.reducer,
            request_id,
            QuestionCloseReason::Answered,
            |input| {
                self.write(outbound::answer_question(
                    request_id, input, answers, response,
                ))
            },
            |out| self.emitter.emit_actions(out, now_ms()),
        )
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
        send_then_close_question(
            &self.reducer,
            request_id,
            QuestionCloseReason::Skipped,
            |input| self.write(outbound::skip_question(request_id, input, reason)),
            |out| self.emitter.emit_actions(out, now_ms()),
        )
    }

    /// Ferme stdin, attend 3 s, puis tue l'arbre par le Job Object.
    pub async fn stop(&self) -> ExitInfo {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(code) = self.exit_code() {
            return ExitInfo {
                code,
                killed: false,
            };
        }
        tracing::info!(session = %self.key.0, "arrêt demandé : fermeture de stdin");
        self.stdin_tx.lock().unwrap().take();
        if let Some(code) = self.wait_exit(self.config.stop_grace).await {
            return ExitInfo {
                code,
                killed: false,
            };
        }
        tracing::warn!(
            session = %self.key.0,
            "pas de sortie {} s après la fermeture de stdin : Job Object terminé",
            self.config.stop_grace.as_secs()
        );
        self.job.terminate();
        let code = self.wait_exit(Duration::from_secs(2)).await.flatten();
        ExitInfo { code, killed: true }
    }

    /// Tue immédiatement l'arbre de processus (sans attente).
    pub fn kill(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.job.terminate();
    }
}

/// Répond à une question en attente : la réponse est écrite sur stdin sous le
/// verrou du réducteur (deux réponses simultanées sont impossibles), et la
/// question n'est retirée de la table qu'après un envoi réussi. En cas d'échec,
/// elle reste en attente et peut recevoir une nouvelle réponse. Le
/// `question_closed` est émis par `emit`, sous le même verrou.
pub(crate) fn send_then_close_question(
    reducer: &Mutex<Reducer>,
    request_id: &str,
    reason: QuestionCloseReason,
    send: impl FnOnce(&Value) -> Result<(), CoreError>,
    emit: impl FnOnce(Vec<Action>),
) -> Result<(), CoreError> {
    let mut r = reducer.lock().unwrap();
    if r.state == Some(SessionState::Exited) {
        return Err(CoreError::NotRunning);
    }
    let input = r
        .questions
        .get(request_id)
        .ok_or_else(|| CoreError::UnknownRequest(request_id.into()))?;
    send(input)?;
    emit(r.close_question(request_id, reason));
    Ok(())
}

fn apply_actions(
    actions: Vec<Action>,
    em: &Emitter,
    writer: &mpsc::WeakUnboundedSender<String>,
    pending: &Pending,
    raw: &Option<Arc<RawLog>>,
    t: u64,
) {
    for a in actions {
        match a {
            Action::Emit(body) => em.emit(body, t),
            Action::Write(line) => {
                if let Some(w) = writer.upgrade() {
                    if let Some(log) = raw {
                        log.sent(&line);
                    }
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
        let ans = QuestionCloseReason::Answered;
        let err = send_then_close_question(&r, "q1", ans, |_| Err(CoreError::NotRunning), |_| {});
        assert_eq!(err, Err(CoreError::NotRunning));
        assert!(
            r.lock().unwrap().questions.contains_key("q1"),
            "question gardée"
        );
        // Une nouvelle tentative réussie la retire.
        let mut seen = None;
        let mut emitted = Vec::new();
        send_then_close_question(
            &r,
            "q1",
            ans,
            |input| {
                seen = Some(input.clone());
                Ok(())
            },
            |out| emitted = out,
        )
        .unwrap();
        assert!(
            seen.unwrap()["questions"].is_array(),
            "input transmis à l'envoi"
        );
        assert!(!r.lock().unwrap().questions.contains_key("q1"));
        assert!(matches!(
            emitted.as_slice(),
            [Action::Emit(EventBody::QuestionClosed {
                reason: QuestionCloseReason::Answered,
                ..
            })]
        ));
        // Déjà répondue : requête inconnue, rien n'est envoyé.
        let again = send_then_close_question(
            &r,
            "q1",
            ans,
            |_| panic!("ne doit pas envoyer"),
            |_| panic!("rien à émettre"),
        );
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

    #[test]
    fn exit_stops_running_agents_then_nothing_more() {
        let mut r = Reducer::new();
        r.handle_at(
            &json!({"type": "assistant", "message": {"id": "msg_1", "content": [
                {"type": "tool_use", "id": "toolu_a", "name": "Agent",
                 "input": {"subagent_type": "agent-bleu", "description": "D"}}]}}),
            100,
        );
        r.user_sent("x");
        let out = r.exited_at(Some(1), vec![], 500);
        let kinds: Vec<&str> = out
            .iter()
            .map(|a| match a {
                Action::Emit(EventBody::Agent(a)) => {
                    assert_eq!(a.status, crate::api::events::AgentStatus::Stopped);
                    assert_eq!(a.ended_at, Some(500));
                    "agent"
                }
                Action::Emit(EventBody::Exited { during_turn, .. }) => {
                    assert!(during_turn);
                    "exited"
                }
                Action::Emit(EventBody::Status { state }) => {
                    assert_eq!(*state, SessionState::Exited);
                    "status"
                }
                other => panic!("inattendu : {other:?}"),
            })
            .collect();
        assert_eq!(kinds, vec!["agent", "exited", "status"]);
        assert!(
            r.exited_at(Some(1), vec![], 600).is_empty(),
            "une seule fois"
        );
        assert!(
            r.user_sent("y").is_empty(),
            "pas de user_message après Exited"
        );
        assert!(r
            .close_question("q", QuestionCloseReason::Answered)
            .is_empty());
    }
}
