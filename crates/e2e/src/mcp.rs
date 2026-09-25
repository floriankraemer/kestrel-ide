//! The MCP control channel, used for *observation only*.
//!
//! `read_buffer` is the cheapest way to ask what a document actually contains
//! and `index_status` the only honest way to know the index finished — the
//! step a naive harness would spell `sleep`. It never drives the app: an
//! `open_file` over MCP routes through `AppSession` and never touches a
//! widget.
//!
//! A round-trip here is also the suite's quiescence probe. Editor-touching
//! tool calls are marshalled onto the Qt thread, so a reply proves the event
//! loop has drained past everything queued before the request.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

pub struct Mcp {
    port: u16,
    token: String,
    next_id: AtomicU64,
}

impl Mcp {
    /// Read the discovery file the app writes into its config dir. Returns
    /// `None` until the server is listening, which is what makes this
    /// pollable.
    pub fn discover(config_dir: &Path) -> Option<Mcp> {
        let raw = std::fs::read_to_string(config_dir.join("mcp-discovery.json")).ok()?;
        let value: Value = serde_json::from_str(&raw).ok()?;
        Some(Mcp {
            port: value.get("port")?.as_u64()? as u16,
            token: value.get("token")?.as_str()?.to_string(),
            next_id: AtomicU64::new(1),
        })
    }

    /// [`discover`](Self::discover), but waits until the port the
    /// discovery file currently names is actually accepting connections —
    /// not just that the file exists, which `discover`'s own caller
    /// (`Ide::mcp`) stops at.
    ///
    /// The Settings dialog's OK handler restarts the MCP server
    /// unconditionally on every accept, on a fresh ephemeral port
    /// (`mcp_page.cpp`), regardless of which page was open. Any flow that
    /// drives Settings to completion and then talks to MCP needs this
    /// rather than a handle grabbed before the dialog opened, or the
    /// stale port's `post` fails outright (`ECONNREFUSED`, not a JSON-RPC
    /// error `try_call`'s own retry loop would catch) — and a bare
    /// `discover` can still read a stale-but-present file for a poll or
    /// two right after the restart begins, before it is rewritten.
    pub fn reconnect(config_dir: &Path) -> Mcp {
        crate::wait_for("the MCP server to accept connections", || {
            let mcp = Self::discover(config_dir)?;
            TcpStream::connect(("127.0.0.1", mcp.port)).ok()?;
            Some(mcp)
        })
    }

    /// One JSON-RPC call over the flat method surface, returning `result`.
    pub fn call(&self, method: &str, params: Value) -> Value {
        self.try_call(method, params)
            .unwrap_or_else(|error| panic!("MCP {method} failed: {error}"))
    }

    /// Same as [`call`](Self::call), but hands back the JSON-RPC error
    /// object instead of panicking on one — for a call a flow *expects* to
    /// fail transiently (`find_files`/`search_text` while a rescope's index
    /// reopen is still running) and wants to retry with `e2e::wait_for`
    /// rather than treat as fatal.
    pub fn try_call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let response = self.post(&body);
        let parsed: Value = serde_json::from_str(&response)
            .unwrap_or_else(|e| panic!("MCP {method} returned {response:?}: {e}"));
        if let Some(error) = parsed.get("error").filter(|e| !e.is_null()) {
            return Err(error.clone());
        }
        Ok(parsed
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("MCP {method} returned no result: {parsed}")))
    }

    // A hand-rolled HTTP/1.1 POST rather than a client crate: `Connection:
    // close` makes the body length the rest of the socket, so this is the
    // whole protocol we need and it keeps the harness dependency-free.
    fn post(&self, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port))
            .unwrap_or_else(|e| panic!("connecting to the MCP server on {}: {e}", self.port));
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.token,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .expect("writing the MCP request");
        let mut raw = String::new();
        stream
            .read_to_string(&mut raw)
            .expect("reading the MCP response");
        let (head, payload) = raw
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("malformed HTTP response: {raw:?}"));
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "MCP replied {head:?} to {body}"
        );
        payload.to_string()
    }
}
