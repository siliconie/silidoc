//! One command entry point for the whole app.
//!
//! `alfadoc.*` commands are Alfadoc's own (link creation, sync now, convert…). Everything else is
//! forwarded to the active engine unchanged. Commands the engine reports as mutating are
//! recorded, so the sync layer knows when to look at the linked regions again.

use std::collections::HashMap;

use serde_json::Value as Json;

use crate::engine::{BridgeError, Engine};

pub const NAMESPACE: &str = "alfadoc.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub seq: u64,
    pub command: String,
}

type Handler = Box<dyn FnMut(&Json) -> Result<Json, BridgeError>>;

pub struct Dispatcher<E: Engine> {
    engine: E,
    own: HashMap<String, Handler>,
    changes: Vec<Change>,
    seq: u64,
}

impl<E: Engine> Dispatcher<E> {
    pub fn new(engine: E) -> Self {
        Dispatcher { engine, own: HashMap::new(), changes: vec![], seq: 0 }
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// Registers an `alfadoc.*` command. Other namespaces are refused so a typo cannot shadow an
    /// engine command.
    pub fn register(&mut self, id: &str, handler: impl FnMut(&Json) -> Result<Json, BridgeError> + 'static) -> Result<(), BridgeError> {
        if !id.starts_with(NAMESPACE) {
            return Err(BridgeError::BadParams(format!("own commands must start with {NAMESPACE}: {id}")));
        }
        self.own.insert(id.to_string(), Box::new(handler));
        Ok(())
    }

    pub fn execute(&mut self, id: &str, params: &Json) -> Result<Json, BridgeError> {
        if id.starts_with(NAMESPACE) {
            let h = self.own.get_mut(id).ok_or_else(|| BridgeError::UnknownCommand(id.to_string()))?;
            return h(params);
        }
        let out = self.engine.execute(id, params)?;
        if self.engine.is_mutating(id) {
            self.seq += 1;
            self.changes.push(Change { seq: self.seq, command: id.to_string() });
        }
        Ok(out)
    }

    /// Takes the changes recorded since the last call.
    pub fn drain_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineKind;
    use serde_json::json;

    struct Fake {
        log: Vec<String>,
    }
    impl Engine for Fake {
        fn kind(&self) -> EngineKind {
            EngineKind::Grid
        }
        fn execute(&mut self, c: &str, _: &Json) -> Result<Json, BridgeError> {
            if c == "boom" {
                return Err(BridgeError::Engine("nope".into()));
            }
            self.log.push(c.into());
            Ok(json!({"ok": c}))
        }
        fn is_mutating(&self, c: &str) -> bool {
            c.starts_with("cell.")
        }
    }

    #[test]
    fn engine_commands_pass_through_and_mutations_are_recorded() {
        let mut d = Dispatcher::new(Fake { log: vec![] });
        d.execute("cell.set", &json!({})).unwrap();
        d.execute("view.zoom", &json!({})).unwrap(); // not mutating
        let ch = d.drain_changes();
        assert_eq!(ch, vec![Change { seq: 1, command: "cell.set".into() }]);
        assert!(d.drain_changes().is_empty());
        assert_eq!(d.engine().log, vec!["cell.set", "view.zoom"]);
    }

    #[test]
    fn failed_commands_are_not_recorded_as_changes() {
        let mut d = Dispatcher::new(Fake { log: vec![] });
        assert!(d.execute("boom", &json!({})).is_err());
        assert!(d.drain_changes().is_empty());
    }

    #[test]
    fn own_commands_are_namespaced_and_never_reach_the_engine() {
        let mut d = Dispatcher::new(Fake { log: vec![] });
        d.register("alfadoc.ping", |_| Ok(json!("pong"))).unwrap();
        assert_eq!(d.execute("alfadoc.ping", &json!({})).unwrap(), json!("pong"));
        assert!(d.engine().log.is_empty());
        assert!(matches!(d.execute("alfadoc.missing", &json!({})), Err(BridgeError::UnknownCommand(_))));
    }

    #[test]
    fn registering_outside_the_namespace_is_refused() {
        let mut d = Dispatcher::new(Fake { log: vec![] });
        assert!(d.register("cell.set", |_| Ok(json!(null))).is_err());
    }
}
