//! Request routing across the servers of one language (ADR-0066): sending,
//! threading and merging. *Which* servers may answer is [`crate::routing`]'s
//! rule and how answers combine is [`crate::merge`]'s; this only carries them
//! out.
//!
//! A child of `manager` so it can reach `Server` and the manager's private
//! helpers without widening their visibility.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::{LspError, LspManager, Server, METHOD_NOT_FOUND};
use crate::routing::{self, Route};

impl LspManager {
    /// Ask every server of the language that can answer `method`, in answer
    /// order, concurrently. The answers come back unmerged, tagged with the
    /// server that gave them (a failure of one server is its own entry).
    pub fn request_all(
        &self,
        language_id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Vec<(String, Result<Value, LspError>)> {
        let mut servers = self.servers_of(language_id);
        if servers.len() > 1 {
            servers.retain(|s| s.can_answer(method));
        }
        fan_out(&servers, method, &params, timeout)
    }

    /// Does any server of the language offer `method`, by static capability
    /// or dynamic registration? For callers that prefer a request when it is
    /// offered and have a fallback when it is not.
    pub fn supports(&self, language_id: &str, method: &str) -> bool {
        self.servers_of(language_id)
            .iter()
            .any(|server| server.can_answer(method))
    }

    pub(super) fn route(
        &self,
        servers: &[Arc<Server>],
        method: &str,
        mut params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let capable: Vec<Arc<Server>> = servers
            .iter()
            .filter(|s| s.can_answer(method))
            .cloned()
            .collect();
        match routing::route_of(method) {
            Route::Origin => {
                let origin = routing::take_origin(&mut params).or_else(|| {
                    let command = params.get("command").and_then(Value::as_str)?;
                    let capabilities: Vec<_> = servers
                        .iter()
                        .map(|s| (s.id.clone(), s.capabilities.lock().unwrap().clone()))
                        .collect();
                    routing::command_owner(
                        command,
                        capabilities.iter().map(|(id, caps)| (id.as_str(), caps)),
                    )
                    .map(str::to_string)
                });
                let target = origin
                    .and_then(|id| servers.iter().find(|s| s.id == id))
                    .or(capable.first())
                    .or(servers.first())
                    .expect("route is only called with servers");
                target.request(method, params, timeout)
            }
            Route::First => {
                let mut first_error = None;
                let mut last_empty = None;
                for server in &capable {
                    match server.request(method, params.clone(), timeout) {
                        Ok(answer) if routing::is_empty_answer(&answer) => {
                            last_empty = Some(answer)
                        }
                        Ok(answer) => return Ok(answer),
                        Err(e) => {
                            first_error.get_or_insert(e);
                        }
                    }
                }
                match (last_empty, first_error) {
                    (Some(empty), _) => Ok(empty),
                    (None, Some(e)) => Err(e),
                    (None, None) => Err(no_capable_server(method)),
                }
            }
            Route::Merge => {
                let mut answers = Vec::new();
                let mut first_error = None;
                for (id, result) in fan_out(&capable, method, &params, timeout) {
                    match result {
                        Ok(answer) => answers.push((id, answer)),
                        Err(e) => {
                            first_error.get_or_insert(e);
                        }
                    }
                }
                match (answers.is_empty(), first_error) {
                    (false, _) => Ok(crate::merge::merge(method, answers)),
                    (true, Some(e)) => Err(e),
                    (true, None) => Err(no_capable_server(method)),
                }
            }
        }
    }
}

fn no_capable_server(method: &str) -> LspError {
    LspError::Response {
        code: METHOD_NOT_FOUND,
        message: format!("no language server for this language offers {method}"),
    }
}

/// Send `method` to every server in `servers` at once and collect the answers
/// in the order given. One thread per server, so the slowest sets the pace.
fn fan_out(
    servers: &[Arc<Server>],
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Vec<(String, Result<Value, LspError>)> {
    thread::scope(|scope| {
        let handles: Vec<_> = servers
            .iter()
            .map(|server| {
                scope.spawn(move || {
                    (
                        server.id.clone(),
                        server.request(method, params.clone(), timeout),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("request thread does not panic"))
            .collect()
    })
}
