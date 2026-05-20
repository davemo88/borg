//! The global borg registry: maps join/master codes to running actors.
//!
//! This is the only globally-shared mutable structure. It is touched only on
//! create / join / connect / send — never on the broadcast hot path — so a
//! plain `std::sync::Mutex` with tiny critical sections is the right tool.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::borg::BorgCommand;
use crate::codes;

/// A cheap, cloneable handle to one borg actor.
#[derive(Clone)]
pub struct BorgHandle {
    pub cmd: mpsc::Sender<BorgCommand>,
}

/// Maps borg codes to actor handles.
pub struct Registry {
    by_join: Mutex<HashMap<String, BorgHandle>>,
    master_to_join: Mutex<HashMap<String, String>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            by_join: Mutex::new(HashMap::new()),
            master_to_join: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a unique join code, build a handle for it (via `mk_handle`, which
    /// spawns the actor), and register it under both the join and master codes.
    /// Holding the lock across `mk_handle` keeps code allocation race-free.
    pub fn try_register(
        &self,
        master_code: &str,
        mk_handle: impl FnOnce(String) -> BorgHandle,
    ) -> String {
        let mut joins = self.by_join.lock().unwrap();
        let join = loop {
            let candidate = codes::join_code();
            if !joins.contains_key(&candidate) {
                break candidate;
            }
        };
        joins.insert(join.clone(), mk_handle(join.clone()));
        drop(joins);

        self.master_to_join
            .lock()
            .unwrap()
            .insert(master_code.to_string(), join.clone());
        join
    }

    /// Look up a borg by its join code.
    pub fn get(&self, join_code: &str) -> Option<BorgHandle> {
        self.by_join.lock().unwrap().get(join_code).cloned()
    }

    /// Check that `master_code` is the master of the borg with `join_code`.
    pub fn verify_master(&self, master_code: &str, join_code: &str) -> bool {
        self.master_to_join
            .lock()
            .unwrap()
            .get(master_code)
            .is_some_and(|j| j == join_code)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
