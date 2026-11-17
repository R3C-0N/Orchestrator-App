//! Commandes Tauri (plan 1.5). Noms d'arguments en `snake_case` côté JS.
//!
//! J11 : `list_projects`, `list_sessions` et `load_history` lisent les
//! transcripts du CLI (module `sessions`, lecture seule).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::ipc::Channel;
use tauri::State;

use super::events::CoreEvent;
use super::types::{
    Answers, CoreError, Diagnostics, ExitInfo, HistoryLoaded, InterruptAck, LaunchInfo,
    ProjectSummary, SessionKey, SessionResync, SessionRole, SessionStarted, SessionSummary,
    SkipReason,
};
use crate::process::locate::{display_path, locate_claude, EXPECTED_CLAUDE_VERSION};
use crate::process::spawn::LaunchOptions;
use crate::process::supervisor::{Attachment, Session, SessionConfig};
use crate::state::batcher::Sink;

/// Délai après lequel une session détachée (interface rechargée ou fermée)
/// et pas rattachée est fermée (J14, décision 15 ; esprit de J6).
pub const DETACH_GRACE: Duration = Duration::from_secs(60);

type SessionMap = Arc<Mutex<HashMap<SessionKey, Arc<Session>>>>;
/// Sessions que la fermeture différée ne touche pas (session d'amélioration
/// tenue par le cycle, même sans page rattachée).
type Protected = Arc<Mutex<HashSet<SessionKey>>>;

/// État partagé de l'app.
pub struct AppState {
    pub sessions: SessionMap,
    pub launch: LaunchInfo,
    /// Dernière session arrêtée ou morte, pour les diagnostics.
    pub last: Arc<Mutex<Option<Arc<Session>>>>,
    /// Délai de fermeture d'une session détachée ([`DETACH_GRACE`] ; les
    /// tests le raccourcissent).
    pub detach_grace: Duration,
    /// Génération de page : incrémentée à chaque (re)chargement de la
    /// webview (`close_orphans`). Un canal d'une génération passée n'est plus
    /// lu par personne : l'essai J14 montre que `Channel::send` y réussit
    /// quand même, la génération est donc la seule détection fiable.
    page_gen: Arc<AtomicU64>,
    /// Canal de la page courante, donné par `attach_sessions` : destination
    /// des sessions lancées par le cœur (rôle `Improve`).
    ui: Mutex<Option<Attachment>>,
    /// Sessions protégées de la fermeture différée ([`AppState::protect`]).
    protected: Protected,
    /// Sérialise `start_session` (lancement puis fermeture des autres
    /// sessions principales) : deux démarrages entrelacés ne se ferment pas
    /// l'un l'autre.
    pub main_start: tokio::sync::Mutex<()>,
}

/// Attente du repos d'une session principale remplacée avant de la fermer.
pub const MAIN_REPLACE_WAIT: Duration = Duration::from_secs(3);

/// Protection d'une session contre la fermeture différée, levée à la
/// destruction (voir [`AppState::protect`]).
pub struct Protection {
    key: SessionKey,
    protected: Protected,
}

impl Drop for Protection {
    fn drop(&mut self) {
        self.protected.lock().unwrap().remove(&self.key);
    }
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
            sessions: Arc::default(),
            launch: LaunchInfo {
                cwd: display_path(&dir.cwd),
                cwd_from_args: dir.from_args,
                launch_error: dir.error,
                claude_path: locate_claude().ok().map(|p| display_path(&p)),
                expected_version: EXPECTED_CLAUDE_VERSION.into(),
            },
            last: Arc::default(),
            detach_grace: DETACH_GRACE,
            page_gen: Arc::new(AtomicU64::new(0)),
            ui: Mutex::new(None),
            protected: Arc::default(),
            main_start: tokio::sync::Mutex::new(()),
        }
    }

    /// Session de la table.
    pub fn get(&self, key: &SessionKey) -> Result<Arc<Session>, CoreError> {
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

    /// Version `async` de [`AppState::detach_all`] (nom gardé de J6), pour
    /// les tests et les outils : `on_page_load` appelle `detach_all`
    /// directement depuis l'étape 5 de J14. Sens changé par J14
    /// (décision 15) : les sessions ne sont plus fermées mais **détachées** ;
    /// leurs événements ne vont plus qu'à leur journal, et la nouvelle page
    /// s'y rattache par `attach_sessions`. Une session pas rattachée après
    /// [`AppState::detach_grace`] est fermée. Renvoie le nombre de sessions
    /// détachées.
    pub async fn close_orphans(&self) -> usize {
        self.detach_all()
    }

    /// La webview (re)charge sa page : appelé directement par `on_page_load`
    /// (pas de tâche), pour que la génération avance avant que la nouvelle
    /// page puisse appeler `attach_sessions`. Contexte du runtime tokio
    /// requis (fermeture différée).
    pub fn detach_all(&self) -> usize {
        let gen = self.page_gen.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut ui = self.ui.lock().unwrap();
            if ui.as_ref().is_some_and(|a| a.page_gen < gen) {
                *ui = None;
            }
        }
        let n = self.reaper().detach_before(gen);
        if n > 0 {
            tracing::info!(
                sessions = n,
                page_gen = gen,
                grace_s = self.detach_grace.as_secs(),
                "rechargement de l'interface : sessions détachées, en attente de rattachement"
            );
        }
        n
    }

    /// Génération de page courante.
    pub fn page_gen(&self) -> u64 {
        self.page_gen.load(Ordering::SeqCst)
    }

    /// Destination `sink` pour la génération de page courante (tests, outils).
    pub fn attachment(&self, sink: Sink) -> Attachment {
        Attachment {
            sink,
            page_gen: self.page_gen(),
        }
    }

    fn reaper(&self) -> Reaper {
        Reaper {
            sessions: self.sessions.clone(),
            last: self.last.clone(),
            grace: self.detach_grace,
            protected: self.protected.clone(),
        }
    }

    /// Rattache toutes les sessions de la table à `page` et rend, pour
    /// chacune, de quoi la reconstruire (`attach_sessions`). Les sessions
    /// lancées ensuite par le cœur (`Improve`) partent aussi vers `page`.
    /// Ordre : du plus ancien démarrage au plus récent.
    ///
    /// Une session morte est rendue une fois (son journal finit par
    /// `exited`), puis retirée de la table : un rechargement suivant ne
    /// re-sérialise pas son journal. Une page d'une génération déjà passée
    /// (rechargée entre-temps) ne rattache rien.
    pub fn attach_all(&self, page: Attachment) -> Vec<SessionResync> {
        if page.page_gen < self.page_gen() {
            tracing::warn!(
                page_gen = page.page_gen,
                "rattachement d'une page déjà rechargée : ignoré"
            );
            return Vec::new();
        }
        *self.ui.lock().unwrap() = Some(page.clone());
        let mut all: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().cloned().collect();
        all.sort_by_key(|s| s.started_at);
        let out = all
            .iter()
            .map(|s| s.resync(page.sink.clone(), page.page_gen))
            .collect();
        self.forget_dead();
        out
    }

    /// Lance une session et l'inscrit dans la table. `own` : destination
    /// propre (canal de `start_session`, avec sa génération de page) ;
    /// `None` : canal de la page courante (`attach_sessions`), ou journal
    /// seul si aucune page n'est rattachée. Une session sans destination
    /// (jamais rattachée, ou canal déjà refusé) est fermée après le délai si
    /// aucune page ne s'y rattache.
    pub async fn spawn_session(
        &self,
        role: SessionRole,
        exe: &Path,
        cwd: &Path,
        opts: LaunchOptions,
        own: Option<Attachment>,
        config: SessionConfig,
    ) -> Result<Arc<Session>, CoreError> {
        self.forget_dead();
        let key = SessionKey(uuid::Uuid::new_v4().to_string());
        let attachment = own.or_else(|| self.ui.lock().unwrap().clone());
        let session =
            Session::start_role(key.clone(), exe, cwd, opts, role, attachment, config).await?;
        // Destination refusée plus tard (fenêtre fermée) : fermeture différée.
        let reaper = self.reaper();
        session
            .outlet()
            .set_on_detach(Arc::new(move || reaper.schedule()));
        self.sessions.lock().unwrap().insert(key, session.clone());
        // Déjà sans destination (jamais rattachée, ou refusée pendant le
        // démarrage, avant l'inscription dans la table).
        if !session.outlet().is_attached() {
            self.reaper().schedule();
        }
        if role == SessionRole::Improve {
            // Fermeture sûre même si l'appelant oublie `close_session` : la
            // session sort de la table à la fin de son processus.
            let reaper = self.reaper();
            let s = session.clone();
            tokio::spawn(async move {
                s.exited().await;
                reaper.forget(&s.key);
            });
        }
        Ok(session)
    }

    /// Étape 5 (J14, décision 17) : lance la session `claude.exe` dédiée à
    /// une amélioration, dans `cwd` (le worktree), et lui envoie `prompt`.
    /// Pas de canal propre : ses événements vont à son journal et au canal
    /// de la page rattachée. Fin du tour : `Session::wait_turn_end(0, …)` ;
    /// fin du processus : `Session::wait_exit` ; arrêt :
    /// `AppState::close_session(&s.key)`.
    pub async fn start_improve(
        &self,
        cwd: &Path,
        prompt: &str,
        opts: LaunchOptions,
    ) -> Result<Arc<Session>, CoreError> {
        let exe = locate_claude().map_err(|tried| CoreError::ClaudeNotFound(tried.join(" ; ")))?;
        self.start_improve_with(&exe, cwd, prompt, opts, SessionConfig::default())
            .await
    }

    /// Comme [`AppState::start_improve`], avec l'exécutable et les délais
    /// choisis (tests avec le faux CLI).
    pub async fn start_improve_with(
        &self,
        exe: &Path,
        cwd: &Path,
        prompt: &str,
        opts: LaunchOptions,
        config: SessionConfig,
    ) -> Result<Arc<Session>, CoreError> {
        let s = self
            .spawn_session(SessionRole::Improve, exe, cwd, opts, None, config)
            .await?;
        if let Err(e) = s.send_message(prompt) {
            self.close_session(&s.key).await;
            return Err(e);
        }
        Ok(s)
    }

    /// Protège la session `key` de la fermeture différée tant que la garde
    /// rendue vit (J14, étape 5) : la session d'amélioration n'a souvent
    /// aucune page rattachée, et serait sinon fermée après
    /// [`DETACH_GRACE`] alors que le cycle l'attend. À la levée de la
    /// protection, une session toujours détachée suit la règle commune.
    pub fn protect(&self, key: &SessionKey) -> Protection {
        self.protected.lock().unwrap().insert(key.clone());
        Protection {
            key: key.clone(),
            protected: self.protected.clone(),
        }
    }

    /// Ferme les sessions principales démarrées avant `keep` (J14, étape 5) :
    /// le cœur garde au plus une session `Main`. L'interface n'en affiche
    /// qu'une (`mainOf` au rattachement) ; une deuxième (nouvelle session
    /// lancée à côté de la courante) continuerait sinon invisible jusqu'à la
    /// fermeture de l'app. Une session plus récente que `keep` n'est jamais
    /// fermée (démarrages entrelacés). Une session en plein tour est attendue
    /// [`MAIN_REPLACE_WAIT`] au plus ; au-delà, le tour abandonné est noté
    /// dans son journal (notice `turn_abandoned`) et au journal de l'app,
    /// puis elle est fermée. Renvoie le nombre de sessions fermées.
    pub async fn close_other_mains(&self, keep: &SessionKey) -> usize {
        let older: Vec<Arc<Session>> = {
            let map = self.sessions.lock().unwrap();
            let Some(newest) = map.get(keep).map(|s| s.started_at) else {
                return 0;
            };
            map.iter()
                .filter(|(k, s)| {
                    *k != keep && s.role == SessionRole::Main && s.started_at <= newest
                })
                .map(|(_, s)| s.clone())
                .collect()
        };
        let mut n = 0;
        for s in older {
            if s.is_running() && !s.is_idle() && !s.wait_idle(MAIN_REPLACE_WAIT).await {
                tracing::warn!(
                    session = %s.key.0,
                    "session principale remplacée en plein tour : tour abandonné"
                );
                s.outlet().emit(
                    crate::api::events::EventBody::Notice {
                        level: crate::api::events::NoticeLevel::Warn,
                        code: "turn_abandoned".into(),
                        message: "session remplacée par une nouvelle session principale : tour en cours abandonné"
                            .into(),
                        detail: None,
                    },
                    crate::process::supervisor::now_ms(),
                );
            }
            if self.close_session(&s.key).await.is_some() {
                tracing::info!(session = %s.key.0, "session principale remplacée : fermée");
                n += 1;
            }
        }
        n
    }

    /// Attend que toutes les sessions principales soient au repos
    /// ([`AppState::main_idle`]) ; vérifié toutes les 250 ms. Sans fin
    /// propre : l'appelant borne ou annule (cycle d'amélioration).
    pub async fn wait_main_idle(&self) {
        while !self.main_idle() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Vrai si toutes les sessions principales vivantes sont au repos (pas de
    /// tour en cours, pas de question ouverte) : condition de la bascule
    /// automatique d'interface (décision 15). Vrai s'il n'y en a aucune.
    pub fn main_idle(&self) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.role == SessionRole::Main && s.is_running())
            .all(|s| s.is_idle())
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

/// Fermeture différée des sessions détachées : une session détachée depuis
/// au moins `grace` est arrêtée et retirée de la table. L'échéance se compte
/// depuis le détachement de chaque session (`Outlet::detached_for`) : une
/// passe trop tôt se reprogramme tant qu'il reste des sessions détachées.
#[derive(Clone)]
struct Reaper {
    sessions: SessionMap,
    last: Arc<Mutex<Option<Arc<Session>>>>,
    grace: Duration,
    protected: Protected,
}

impl Reaper {
    /// Détache les sessions dont la destination vient d'une génération de
    /// page antérieure à `gen`, puis programme leur fermeture.
    fn detach_before(&self, gen: u64) -> usize {
        let all: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().cloned().collect();
        let n = all.iter().filter(|s| s.outlet().detach_before(gen)).count();
        if n > 0 {
            self.schedule();
        }
        n
    }

    /// Programme des passes jusqu'à ce qu'aucune session ne soit détachée
    /// (runtime tokio requis : commandes, regroupeur).
    fn schedule(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut wait = me.grace;
            loop {
                tokio::time::sleep(wait).await;
                match me.sweep().await {
                    Some(next) => wait = next,
                    None => break,
                }
            }
        });
    }

    /// Ferme les sessions détachées depuis `grace` ; renvoie l'attente avant
    /// l'échéance de la plus proche des autres sessions détachées.
    async fn sweep(&self) -> Option<Duration> {
        let mut next: Option<Duration> = None;
        let expired: Vec<Arc<Session>> = {
            let protected = self.protected.lock().unwrap().clone();
            let mut map = self.sessions.lock().unwrap();
            let mut keys = Vec::new();
            for (k, s) in map.iter() {
                if protected.contains(k) {
                    // Tenue par le cycle d'amélioration : repasser plus tard.
                    if s.outlet().detached_for().is_some() {
                        next = Some(next.map_or(self.grace, |n| n.min(self.grace)));
                    }
                    continue;
                }
                match s.outlet().detached_for() {
                    Some(d) if d >= self.grace => keys.push(k.clone()),
                    Some(d) => {
                        // Marge de 10 ms : l'échéance est passée au réveil.
                        let left = self.grace - d + Duration::from_millis(10);
                        next = Some(next.map_or(left, |n| n.min(left)));
                    }
                    None => {}
                }
            }
            keys.iter().filter_map(|k| map.remove(k)).collect()
        };
        for s in expired {
            tracing::warn!(
                session = %s.key.0,
                pid = s.pid,
                "aucune interface rattachée depuis {} s : session fermée",
                self.grace.as_secs()
            );
            s.stop().await;
            *self.last.lock().unwrap() = Some(s);
        }
        next
    }

    /// Retire une session de la table (gardée pour les diagnostics).
    fn forget(&self, key: &SessionKey) {
        if let Some(s) = self.sessions.lock().unwrap().remove(key) {
            *self.last.lock().unwrap() = Some(s);
        }
    }
}

/// Destination des événements vers un canal de l'interface, valable pour la
/// génération de page courante seulement (renvoyée avec le `Sink`) : une
/// page rechargée garde un canal où `send` réussit sans que personne ne
/// lise (essai J14), donc un canal d'une génération passée est tenu pour
/// fermé. Un envoi refusé (fenêtre fermée sans rechargement) vaut perte de
/// la page : toutes les sessions de cette génération sont détachées, y
/// compris celles qui n'émettent rien, et fermées sans rattachement dans le
/// délai.
fn channel_sink(
    events: Channel<Vec<CoreEvent>>,
    page_gen: Arc<AtomicU64>,
    reaper: Reaper,
) -> Attachment {
    let gen = page_gen.load(Ordering::SeqCst);
    let failed = Arc::new(AtomicBool::new(false));
    let sink: Sink = Arc::new(move |batch| {
        if page_gen.load(Ordering::SeqCst) != gen {
            return false;
        }
        if events.send(batch).is_ok() {
            return true;
        }
        if !failed.swap(true, Ordering::SeqCst) {
            tracing::warn!(
                page_gen = gen,
                "canal de l'interface fermé : sessions détachées"
            );
            // Les sessions de la page qui n'émettent rien sont détachées
            // aussi ; celle dont le lot vient d'échouer se détache elle-même
            // (`Outlet::deliver`, rappel `on_detach`).
            reaper.detach_before(gen + 1);
        }
        false
    });
    Attachment {
        sink,
        page_gen: gen,
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
    let page = channel_sink(events, state.page_gen.clone(), state.reaper());
    // Un démarrage à la fois (lancement puis fermeture des anciennes).
    let _one = state.main_start.lock().await;
    let session = state
        .spawn_session(
            SessionRole::Main,
            &exe,
            &cwd,
            LaunchOptions {
                resume,
                model,
                extra_args: vec![],
                ..LaunchOptions::default()
            },
            Some(page),
            SessionConfig::default(),
        )
        .await?;
    // Au plus une session principale (J14, étape 5).
    state.close_other_mains(&session.key).await;
    Ok(SessionStarted {
        session: session.key.clone(),
        pid: session.pid,
        claude_path: display_path(&exe),
    })
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
///
/// J14 : le premier historique chargé est gardé par la session et rendu
/// tel quel ensuite (et par `attach_sessions`) : relu plus tard, le
/// transcript contiendrait aussi les tours de la session, déjà dans son
/// journal.
#[tauri::command(rename_all = "snake_case")]
pub async fn load_history(
    state: State<'_, AppState>,
    session: SessionKey,
    session_id: String,
) -> Result<HistoryLoaded, CoreError> {
    let s = state.get(&session)?;
    if let Some(h) = s.cached_history(&session_id) {
        return Ok(h);
    }
    let cwd = display_path(&s.cwd);
    let root = projects_root()?;
    let h = tauri::async_runtime::spawn_blocking(move || {
        let dir = crate::sessions::paths::find_project_dir(&root, &cwd)
            .ok_or_else(|| CoreError::Io(format!("aucun transcript pour le dossier {cwd}")))?;
        crate::sessions::transcript::load_history(&dir, &session_id, &session)
    })
    .await
    .map_err(|e| CoreError::Io(e.to_string()))??;
    Ok(s.keep_history(h))
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
/// état (historique repris, journal des événements). Les événements émis
/// après l'instantané arrivent ensuite sur `events`, sans trou ni doublon de
/// `seq` (voir `process::supervisor::Outlet`).
///
/// `async` : une commande synchrone tourne sur le thread principal de la
/// webview ; ici la copie des journaux (≈ 5 ms pour 10 000 événements) et la
/// sérialisation de la réponse (≈ 0,1 à 0,4 s en debug) se font sur un fil
/// du runtime, l'interface reste fluide.
#[tauri::command(rename_all = "snake_case")]
pub async fn attach_sessions(
    state: State<'_, AppState>,
    events: Channel<Vec<CoreEvent>>,
) -> Result<Vec<SessionResync>, CoreError> {
    let page = channel_sink(events, state.page_gen.clone(), state.reaper());
    Ok(state.attach_all(page))
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
