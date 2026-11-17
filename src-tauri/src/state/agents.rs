//! Registre des sous-agents (plan 1.6 : `agents`, `task_to_tool`).
//!
//! Clé : `tool_use_id` de l'appel `Agent` (ou `Task`, ancien nom, spec 4.4).
//! Chaque changement renvoie l'état **complet** de l'agent : l'interface
//! remplace sa copie à chaque événement `agent` (`session.svelte.ts`), elle ne
//! fusionne pas.
//!
//! Sources, dans l'ordre où le flux les donne (`t3`) :
//! - `tool_use` `Agent` (message `assistant`) : `Starting`, type, description,
//!   arrière-plan (`input.run_in_background`), couleur ;
//! - `system/background_tasks_changed` : tâches d'arrière-plan actives ; peut
//!   précéder `task_started` (`t3`, ligne 13), l'agent est alors retrouvé par
//!   son type et sa description ;
//! - `system/task_started` : `Running`, relie `task_id` et `tool_use_id` ;
//! - `system/task_progress` : tokens, nombre d'appels, outil en cours
//!   (`last_tool_name`) ;
//! - `tool_progress` (`parent_tool_use_id` = l'agent) et `tool_use` dans le
//!   fil de l'agent : outil en cours ;
//! - `system/task_updated` (`patch.status`) puis `system/task_notification` :
//!   état final, résumé, usage ;
//! - `tool_result` de l'appel `Agent` d'avant-plan : fin, si aucune
//!   notification n'est arrivée.
//!
//! Les tâches qui ne sont pas des agents (`task_type: "local_bash"`, `t5`,
//! `t14`) sont ignorées : elles ne doivent pas transformer la ligne de l'outil
//! Bash en ligne d'agent dans l'interface.
//!
//! `started_at` et `ended_at` sont l'heure de lecture de la ligne (`t`) qui a
//! ouvert ou fermé l'agent, sur la même horloge que les `CoreEvent`.

use std::collections::HashMap;

use serde_json::Value;

use crate::api::events::{AgentEvent, AgentStatus};
use crate::colors::{agent_color, agent_name};
use crate::protocol::inbound::{
    TaskNotification, TaskProgress, TaskStarted, TaskUpdated, TaskUsage, ToolProgress,
};

/// Noms de l'outil qui lance un sous-agent (spec 4.4).
pub const AGENT_TOOL_NAMES: &[&str] = &["Agent", "Task"];

/// Outil des questions de Claude (`can_use_tool` en attente de l'interface).
const ASK_TOOL: &str = "AskUserQuestion";

pub fn is_agent_tool(name: &str) -> bool {
    AGENT_TOOL_NAMES.contains(&name)
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    /// Agents dans l'ordre d'apparition.
    agents: Vec<AgentEvent>,
    /// `tool_use_id` -> index dans `agents`.
    by_tool: HashMap<String, usize>,
    /// `task_id` -> `tool_use_id` (`task_started`, `background_tasks_changed`).
    task_to_tool: HashMap<String, String>,
    /// `tool_use_id` d'un `AskUserQuestion` appelé dans le fil d'un agent ->
    /// `tool_use_id` de cet agent ([`AgentRegistry::asking_agent`]). Seul cet
    /// outil est noté : la table reste minuscule.
    ask_thread: HashMap<String, String>,
}

fn is_terminal(s: AgentStatus) -> bool {
    matches!(
        s,
        AgentStatus::Completed | AgentStatus::Failed | AgentStatus::Stopped
    )
}

/// Statut de tâche du CLI (`task_updated.patch.status`, `task_notification`).
fn status_from(s: &str) -> Option<AgentStatus> {
    Some(match s {
        "completed" | "success" => AgentStatus::Completed,
        "failed" | "error" => AgentStatus::Failed,
        "stopped" | "killed" | "cancelled" | "canceled" | "aborted" => AgentStatus::Stopped,
        "running" | "pending" | "started" => AgentStatus::Running,
        _ => return None,
    })
}

fn is_agent_task(task_type: Option<&str>, subagent_type: Option<&str>) -> bool {
    task_type == Some("local_agent") || subagent_type.is_some_and(|s| !s.trim().is_empty())
}

fn new_agent(tool_use_id: &str, agent_type: &str, now: u64) -> AgentEvent {
    let agent_type = agent_name(agent_type);
    AgentEvent {
        tool_use_id: tool_use_id.to_string(),
        task_id: None,
        color: agent_color(&agent_type).map(str::to_string),
        agent_type,
        description: String::new(),
        status: AgentStatus::Starting,
        background: false,
        started_at: Some(now),
        ended_at: None,
        last_tool: None,
        tokens: None,
        tool_uses: None,
        summary: None,
    }
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Agents connus, dans l'ordre d'apparition.
    pub fn all(&self) -> &[AgentEvent] {
        &self.agents
    }

    pub fn get(&self, tool_use_id: &str) -> Option<&AgentEvent> {
        self.by_tool.get(tool_use_id).map(|&i| &self.agents[i])
    }

    pub fn is_agent(&self, tool_use_id: &str) -> bool {
        self.by_tool.contains_key(tool_use_id)
    }

    /// `tool_use_id` lié à `task_id`.
    pub fn tool_of_task(&self, task_id: &str) -> Option<&str> {
        self.task_to_tool.get(task_id).map(String::as_str)
    }

    /// Applique `f` à l'agent `tool_use_id` (créé par `create` s'il manque et
    /// si `create` le permet) ; renvoie son nouvel état s'il a changé.
    fn update(
        &mut self,
        tool_use_id: &str,
        create: Option<&dyn Fn() -> AgentEvent>,
        f: impl FnOnce(&mut AgentEvent),
    ) -> Option<AgentEvent> {
        let (i, created) = match self.by_tool.get(tool_use_id) {
            Some(&i) => (i, false),
            None => {
                let a = create?();
                self.agents.push(a);
                let i = self.agents.len() - 1;
                self.by_tool.insert(tool_use_id.to_string(), i);
                (i, true)
            }
        };
        let before = self.agents[i].clone();
        f(&mut self.agents[i]);
        let a = &mut self.agents[i];
        if a.color.is_none() {
            a.color = agent_color(&a.agent_type).map(str::to_string);
        }
        if created || *a != before {
            Some(a.clone())
        } else {
            None
        }
    }

    fn link_task(&mut self, task_id: &str, tool_use_id: &str) {
        self.task_to_tool
            .insert(task_id.to_string(), tool_use_id.to_string());
        if let Some(&i) = self.by_tool.get(tool_use_id) {
            self.agents[i].task_id = Some(task_id.to_string());
        }
    }

    /// Bloc `tool_use` complet (message `assistant`), dans le fil `thread`
    /// (`None` = fil principal). Un appel `Agent`/`Task` crée l'agent ; un
    /// autre outil appelé dans le fil d'un agent devient son outil en cours.
    pub fn on_tool_use(
        &mut self,
        tool_use_id: &str,
        name: &str,
        input: &Value,
        thread: Option<&str>,
        now: u64,
    ) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        if is_agent_tool(name) && !tool_use_id.is_empty() {
            let raw_type = input
                .get("subagent_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let description = input
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let background = input
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let create = || new_agent(tool_use_id, raw_type, now);
            out.extend(self.update(tool_use_id, Some(&create), |a| {
                if a.agent_type.is_empty() {
                    a.agent_type = agent_name(raw_type);
                }
                if a.description.is_empty() {
                    a.description = description;
                }
                a.background |= background;
            }));
        }
        if let Some(parent) = thread {
            if name == ASK_TOOL && !tool_use_id.is_empty() {
                self.ask_thread
                    .insert(tool_use_id.to_string(), parent.to_string());
            }
            out.extend(self.set_last_tool(parent, name));
        }
        out
    }

    /// Agent (son `tool_use_id`) qui attend la réponse à un `can_use_tool` :
    /// d'abord par `agent_id` de la requête (le `task_id` de l'agent, ou son
    /// `tool_use_id`), sinon par le fil où l'appel `tool_use_id` a été vu.
    /// `None` : la question vient du fil principal (ou d'un agent inconnu).
    pub fn asking_agent(&self, agent_id: Option<&str>, tool_use_id: Option<&str>) -> Option<&str> {
        let by_agent = agent_id.filter(|id| !id.is_empty()).and_then(|id| {
            self.tool_of_task(id)
                .or_else(|| self.by_tool.get_key_value(id).map(|(k, _)| k.as_str()))
        });
        by_agent.or_else(|| {
            tool_use_id
                .and_then(|t| self.ask_thread.get(t))
                .map(String::as_str)
                .filter(|id| self.is_agent(id))
        })
    }

    fn set_last_tool(&mut self, tool_use_id: &str, tool: &str) -> Option<AgentEvent> {
        if tool.is_empty() {
            return None;
        }
        self.update(tool_use_id, None, |a| {
            if !is_terminal(a.status) {
                a.last_tool = Some(tool.to_string());
            }
        })
    }

    /// `system/task_started`.
    pub fn on_task_started(&mut self, t: &TaskStarted, now: u64) -> Option<AgentEvent> {
        let tool_use_id = t.tool_use_id.as_deref()?;
        let known = self.is_agent(tool_use_id);
        if !known && !is_agent_task(t.task_type.as_deref(), t.subagent_type.as_deref()) {
            return None;
        }
        if let Some(task_id) = &t.task_id {
            self.link_task(task_id, tool_use_id);
        }
        let raw_type = t.subagent_type.clone().unwrap_or_default();
        let create = || new_agent(tool_use_id, &raw_type, now);
        self.update(tool_use_id, Some(&create), |a| {
            if a.agent_type.is_empty() {
                a.agent_type = agent_name(&raw_type);
            }
            if a.description.is_empty() {
                a.description = t.description.clone().unwrap_or_default();
            }
            if a.task_id.is_none() {
                a.task_id = t.task_id.clone();
            }
            a.background |= t.is_backgrounded.unwrap_or(false);
            if a.status == AgentStatus::Starting {
                a.status = AgentStatus::Running;
            }
        })
    }

    fn resolve(&self, tool_use_id: Option<&str>, task_id: Option<&str>) -> Option<String> {
        if let Some(id) = tool_use_id.filter(|id| self.is_agent(id)) {
            return Some(id.to_string());
        }
        task_id
            .and_then(|t| self.task_to_tool.get(t))
            .filter(|id| self.is_agent(id))
            .cloned()
    }

    fn apply_usage(a: &mut AgentEvent, u: Option<&TaskUsage>) {
        if let Some(u) = u {
            if u.total_tokens.is_some() {
                a.tokens = u.total_tokens;
            }
            if let Some(n) = u.tool_uses {
                a.tool_uses = Some(u32::try_from(n).unwrap_or(u32::MAX));
            }
        }
    }

    /// `system/task_progress` : usage et outil en cours.
    pub fn on_task_progress(&mut self, p: &TaskProgress) -> Option<AgentEvent> {
        let id = self.resolve(p.tool_use_id.as_deref(), p.task_id.as_deref())?;
        self.update(&id, None, |a| {
            if is_terminal(a.status) {
                return;
            }
            if a.status == AgentStatus::Starting {
                a.status = AgentStatus::Running;
            }
            Self::apply_usage(a, p.usage.as_ref());
            if let Some(tool) = p.last_tool_name.as_ref().filter(|t| !t.is_empty()) {
                a.last_tool = Some(tool.clone());
            }
        })
    }

    /// `tool_progress` d'un outil appelé par un agent.
    pub fn on_tool_progress(&mut self, p: &ToolProgress) -> Option<AgentEvent> {
        let parent = p.parent_tool_use_id.as_deref()?;
        if !self.is_agent(parent) {
            return None;
        }
        self.set_last_tool(parent, p.tool_name.as_deref().unwrap_or(""))
    }

    /// `system/task_updated` : `patch.status` (et `patch.end_time`, ignoré :
    /// `ended_at` suit l'horloge des événements).
    pub fn on_task_updated(&mut self, u: &TaskUpdated, now: u64) -> Option<AgentEvent> {
        let id = self.resolve(None, u.task_id.as_deref())?;
        let status = u
            .patch
            .as_ref()
            .and_then(|p| p.get("status"))
            .and_then(Value::as_str)
            .and_then(status_from)?;
        self.update(&id, None, |a| Self::set_status(a, status, now))
    }

    fn set_status(a: &mut AgentEvent, status: AgentStatus, now: u64) {
        if is_terminal(a.status) {
            return;
        }
        a.status = status;
        if is_terminal(status) && a.ended_at.is_none() {
            a.ended_at = Some(now);
        }
    }

    /// `system/task_notification` : état final, résumé, usage.
    pub fn on_task_notification(&mut self, n: &TaskNotification, now: u64) -> Option<AgentEvent> {
        let id = self.resolve(n.tool_use_id.as_deref(), n.task_id.as_deref())?;
        let status = n
            .status
            .as_deref()
            .and_then(status_from)
            .unwrap_or(AgentStatus::Completed);
        self.update(&id, None, |a| {
            Self::set_status(a, status, now);
            Self::apply_usage(a, n.usage.as_ref());
            if let Some(s) = n.summary.as_ref().filter(|s| !s.is_empty()) {
                a.summary = Some(s.clone());
            }
        })
    }

    /// `system/background_tasks_changed` : liste des tâches d'arrière-plan
    /// actives. Une tâche d'agent listée est en arrière-plan et en cours ; si
    /// elle n'est pas encore reliée (`task_started` pas encore reçu), elle est
    /// rattachée à l'agent `Starting` non relié de même type et description.
    /// Une tâche qui disparaît de la liste ne change pas d'état : la fin vient
    /// de `task_updated` / `task_notification`.
    pub fn on_background_tasks(&mut self, tasks: Option<&Value>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Some(list) = tasks.and_then(Value::as_array) else {
            return out;
        };
        for task in list {
            let s = |k: &str| task.get(k).and_then(Value::as_str);
            let Some(task_id) = s("task_id") else {
                continue;
            };
            if !is_agent_task(s("task_type"), s("subagent_type")) {
                continue;
            }
            let id = match self.task_to_tool.get(task_id) {
                Some(id) => id.clone(),
                None => {
                    let ty = agent_name(s("subagent_type").unwrap_or(""));
                    let desc = s("description").unwrap_or("");
                    let found = self.agents.iter().find(|a| {
                        a.task_id.is_none()
                            && !is_terminal(a.status)
                            && a.agent_type == ty
                            && a.description == desc
                    });
                    let Some(id) = found.map(|a| a.tool_use_id.clone()) else {
                        continue;
                    };
                    self.link_task(task_id, &id);
                    id
                }
            };
            let task_id = task_id.to_string();
            out.extend(self.update(&id, None, |a| {
                a.background = true;
                if a.task_id.is_none() {
                    a.task_id = Some(task_id);
                }
                if a.status == AgentStatus::Starting {
                    a.status = AgentStatus::Running;
                }
            }));
        }
        out
    }

    /// `tool_result` de l'appel `Agent` : un agent d'avant-plan est fini (le
    /// résultat est son rapport) ; en arrière-plan, le résultat n'est que
    /// l'accusé de lancement (« Async agent launched »).
    pub fn on_tool_result(
        &mut self,
        tool_use_id: &str,
        is_error: bool,
        now: u64,
    ) -> Option<AgentEvent> {
        self.update(tool_use_id, None, |a| {
            if a.background {
                // Accusé de lancement ; en erreur, l'agent n'a pas démarré.
                if is_error {
                    Self::set_status(a, AgentStatus::Failed, now);
                }
                return;
            }
            let status = if is_error {
                AgentStatus::Failed
            } else {
                AgentStatus::Completed
            };
            Self::set_status(a, status, now);
        })
    }
}

impl AgentRegistry {
    /// Fin du processus : les agents non terminés passent en `Stopped`.
    /// Renvoie leur nouvel état complet.
    pub fn stop_unfinished(&mut self, now: u64) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        for a in &mut self.agents {
            if !is_terminal(a.status) {
                Self::set_status(a, AgentStatus::Stopped, now);
                out.push(a.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn background_launch_error_fails_and_exit_stops_the_rest() {
        let mut r = AgentRegistry::new();
        r.on_tool_use(
            "t_bg",
            "Agent",
            &json!({"subagent_type":"agent-rouge","description":"A","run_in_background":true}),
            None,
            10,
        );
        r.on_tool_use(
            "t_ok",
            "Agent",
            &json!({"subagent_type":"agent-bleu","description":"B","run_in_background":true}),
            None,
            10,
        );
        r.on_tool_use(
            "t_fg",
            "Agent",
            &json!({"subagent_type":"agent-vert","description":"C","run_in_background":false}),
            None,
            10,
        );
        // Lancement d'arrière-plan en erreur : `Failed`.
        let a = r.on_tool_result("t_bg", true, 20).unwrap();
        assert_eq!(a.status, AgentStatus::Failed);
        assert_eq!(a.ended_at, Some(20));
        // Accusé normal : l'agent continue.
        assert!(r.on_tool_result("t_ok", false, 20).is_none());
        assert!(!is_terminal(r.get("t_ok").unwrap().status));
        // Fin du processus : les deux agents non terminés passent en `Stopped`.
        let stopped = r.stop_unfinished(30);
        let ids: Vec<_> = stopped.iter().map(|a| a.tool_use_id.as_str()).collect();
        assert_eq!(ids, vec!["t_ok", "t_fg"]);
        assert!(stopped
            .iter()
            .all(|a| a.status == AgentStatus::Stopped && a.ended_at == Some(30)));
        assert_eq!(r.get("t_bg").unwrap().status, AgentStatus::Failed);
        assert!(r.stop_unfinished(40).is_empty(), "une seule fois");
    }

    #[test]
    fn foreground_agent_lifecycle() {
        let mut r = AgentRegistry::new();
        let ev = r.on_tool_use(
            "tu1",
            "Agent",
            &json!({"subagent_type":"agent-bleu","description":"D","run_in_background":false}),
            None,
            10,
        );
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].status, AgentStatus::Starting);
        assert_eq!(ev[0].color.as_deref(), Some("blue"));
        assert_eq!(ev[0].started_at, Some(10));
        // Outil appelé dans le fil de l'agent.
        let ev = r.on_tool_use("tu2", "Read", &json!({}), Some("tu1"), 11);
        assert_eq!(ev[0].last_tool.as_deref(), Some("Read"));
        // Fin par le tool_result.
        let a = r.on_tool_result("tu1", false, 20).unwrap();
        assert_eq!(a.status, AgentStatus::Completed);
        assert_eq!(a.ended_at, Some(20));
        // Rien ne change après la fin.
        assert!(r.on_tool_result("tu1", true, 30).is_none());
    }

    #[test]
    fn bash_tasks_are_not_agents() {
        let mut r = AgentRegistry::new();
        let t = TaskStarted {
            task_id: Some("b1".into()),
            tool_use_id: Some("toolu_bash".into()),
            task_type: Some("local_bash".into()),
            ..Default::default()
        };
        assert!(r.on_task_started(&t, 1).is_none());
        assert!(r.all().is_empty());
        let n = TaskNotification {
            task_id: Some("b1".into()),
            tool_use_id: Some("toolu_bash".into()),
            status: Some("failed".into()),
            ..Default::default()
        };
        assert!(r.on_task_notification(&n, 2).is_none());
    }

    #[test]
    fn background_list_links_task_before_task_started() {
        let mut r = AgentRegistry::new();
        r.on_tool_use(
            "tu9",
            "Agent",
            &json!({"subagent_type":"agent-rouge","description":"Dire bonjour","run_in_background":true}),
            None,
            1,
        );
        let tasks = json!([{"task_id":"a9","task_type":"local_agent","subagent_type":"\u{a0}agent-rouge\u{a0}","description":"Dire bonjour"}]);
        let ev = r.on_background_tasks(Some(&tasks));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].task_id.as_deref(), Some("a9"));
        assert_eq!(ev[0].status, AgentStatus::Running);
        assert_eq!(r.tool_of_task("a9"), Some("tu9"));
        // Le tool_result d'arrière-plan n'est qu'un accusé de lancement.
        assert!(r.on_tool_result("tu9", false, 2).is_none());
        let u = TaskUpdated {
            task_id: Some("a9".into()),
            patch: Some(json!({"status":"completed","end_time":5})),
        };
        let a = r.on_task_updated(&u, 3).unwrap();
        assert_eq!(a.status, AgentStatus::Completed);
        assert_eq!(a.ended_at, Some(3));
    }
}
