//! Fils et blocs (plan 1.5 : `ThreadId`, `MsgKey`, index de bloc).
//!
//! Le message `assistant` complet arrive en **plusieurs lignes** qui partagent
//! le même `message.id`, une ligne par bloc (`t3` : thinking, puis Agent,
//! puis Agent). L'index de bloc est donc compté par `(fil, message)` ; il
//! coïncide avec l'`index` des `content_block_start` du flux partiel (`t1`).
//!
//! Les compteurs du fil principal sont vidés à chaque `result` (un message
//! ne chevauche pas deux tours) ; ceux d'un sous-agent à la fin de l'agent.
//! La mémoire reste bornée sur une longue session.

use std::collections::HashMap;

use crate::api::events::ThreadId;

/// `"main"` ou l'id du `tool_use` Agent (`parent_tool_use_id`).
pub fn thread_of(parent_tool_use_id: Option<&str>) -> ThreadId {
    match parent_tool_use_id {
        Some(id) if !id.is_empty() => ThreadId(id.to_string()),
        _ => ThreadId::main(),
    }
}

#[derive(Debug, Default)]
pub struct BlockCounters {
    /// fil -> (message.id -> index du prochain bloc).
    next: HashMap<String, HashMap<String, u32>>,
}

impl BlockCounters {
    /// Index du prochain bloc du message `msg` dans le fil `thread`.
    pub fn next_index(&mut self, thread: &ThreadId, msg: &str) -> u32 {
        let c = self
            .next
            .entry(thread.0.clone())
            .or_default()
            .entry(msg.to_string())
            .or_insert(0);
        let i = *c;
        *c += 1;
        i
    }

    /// Oublie les messages du fil `thread`.
    pub fn clear_thread(&mut self, thread: &str) {
        self.next.remove(thread);
    }

    /// Nombre de messages suivis (tous fils confondus).
    pub fn len(&self) -> usize {
        self.next.values().map(HashMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_are_per_thread_and_message() {
        let mut c = BlockCounters::default();
        let main = ThreadId::main();
        let sub = thread_of(Some("toolu_1"));
        assert_eq!(c.next_index(&main, "m1"), 0);
        assert_eq!(c.next_index(&main, "m1"), 1);
        assert_eq!(c.next_index(&sub, "m1"), 0);
        assert_eq!(c.next_index(&main, "m2"), 0);
        c.clear_thread("main");
        assert_eq!(c.len(), 1);
        assert_eq!(c.next_index(&main, "m1"), 0);
    }

    #[test]
    fn empty_parent_is_main() {
        assert_eq!(thread_of(None), ThreadId::main());
        assert_eq!(thread_of(Some("")), ThreadId::main());
    }
}
