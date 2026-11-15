//! datarig: the connection's runtime parameters, shared with the caller (all of this file is
//! datarig's).
//!
//! The `Connection` keeps the parameters the server reports (`ParameterStatus`), but it is
//! polled in a task of its own, so the caller cannot ask it. A [`SharedParameters`] handle is
//! updated by the connection as each report arrives, before any message the server sent after
//! the report is handed to its request: once a request's response has been read, the handle
//! holds every report sent before the end of that response.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// A handle on a connection's runtime parameters, as the server last reported them.
#[derive(Clone, Debug, Default)]
pub struct SharedParameters(Arc<Mutex<HashMap<String, Reported>>>);

#[derive(Clone, Debug)]
struct Reported {
    value: String,
    /// How many times the server reported this parameter after startup.
    reports: u64,
}

impl SharedParameters {
    pub(crate) fn new(startup: &HashMap<String, String>) -> Self {
        let map = startup
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    Reported {
                        value: value.clone(),
                        reports: 0,
                    },
                )
            })
            .collect();
        SharedParameters(Arc::new(Mutex::new(map)))
    }

    /// The server reported `name` as `value`.
    pub(crate) fn set(&self, name: &str, value: &str) {
        let mut map = self.0.lock();
        match map.get_mut(name) {
            Some(r) => {
                r.value = value.to_string();
                r.reports += 1;
            }
            None => {
                map.insert(
                    name.to_string(),
                    Reported {
                        value: value.to_string(),
                        reports: 1,
                    },
                );
            }
        }
    }

    /// The value of parameter `name` as last reported, and how many times the server reported
    /// it after startup (a report that names the value it had counts too); `None` when the
    /// server never reported it.
    pub fn get(&self, name: &str) -> Option<(String, u64)> {
        self.0.lock().get(name).map(|r| (r.value.clone(), r.reports))
    }
}
