//! Integration test for the hand-rolled MCP server (DESIGN.md §10).
//!
//! Spawns the real `treesmith` binary in `mcp` mode against
//! `fixtures/rainbow/basic`, then drives a full JSON-RPC session over the
//! child's stdin/stdout pipes: `initialize` → `notifications/initialized`
//! → `tools/list` → two `tools/call` round-trips (query + validate) →
//! close stdin and assert a clean exit.
//!
//! Every response is read line-by-line behind a timeout guard: a reader
//! thread pushes each line onto a channel, and each read blocks on the
//! channel with a deadline so a server hang fails the test instead of
//! freezing the suite.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// Per-line read timeout — generous enough for a debug-build cold start,
/// short enough that a genuine hang fails within the suite's patience.
const LINE_TIMEOUT: Duration = Duration::from_secs(30);

/// Drives one JSON-RPC session end to end.
#[test]
fn mcp_handshake_list_and_calls() {
    let mut session = Session::spawn();

    // 1. initialize: echo a supported protocol version, report identity.
    let init = session.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "integration-test", "version": "0" }
        }),
    );
    assert_eq!(init["id"], 1, "initialize response id");
    let result = &init["result"];
    assert_eq!(
        result["protocolVersion"], "2025-06-18",
        "protocol version echoed"
    );
    assert_eq!(
        result["serverInfo"]["name"], "treesmith",
        "serverInfo.name is treesmith"
    );
    assert!(
        result["serverInfo"]["version"].is_string(),
        "serverInfo.version present"
    );

    // 2. notifications/initialized: a notification, never answered.
    session.notify("notifications/initialized", json!({}));

    // 3. tools/list: all eight tool names, in order.
    let list = session.request(2, "tools/list", json!({}));
    let tools = list["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("tool name is a string"))
        .collect();
    assert_eq!(
        names,
        [
            "query",
            "get",
            "set_field",
            "forge",
            "move",
            "resolve_presentation",
            "validate",
            "census",
        ],
        "all eight tools present, mirroring the CLI verbs"
    );

    // 4a. tools/call query — text payload parses to the §8 query shape.
    let query = session.request(
        3,
        "tools/call",
        json!({
            "name": "query",
            "arguments": { "expr": "path:/sitecore/content/**" }
        }),
    );
    let query_result = &query["result"];
    assert_eq!(query_result["isError"], false, "query is not an error");
    let query_payload = tool_text(query_result);
    assert_eq!(query_payload["ok"], true, "query ok");
    assert!(
        query_payload["count"].is_number(),
        "query has a numeric count"
    );
    let items = query_payload["items"]
        .as_array()
        .expect("query items is an array");
    assert!(
        !items.is_empty(),
        "content tree query matches at least one item"
    );
    // ItemSummary shape (DESIGN §8): id / path / name present.
    let first = &items[0];
    assert!(first["id"].is_string(), "item summary has an id");
    assert!(first["path"].is_string(), "item summary has a path");
    assert!(first["name"].is_string(), "item summary has a name");

    // 4b. tools/call validate — text payload parses to the §8 validate shape.
    let validate = session.request(
        4,
        "tools/call",
        json!({ "name": "validate", "arguments": {} }),
    );
    let validate_result = &validate["result"];
    let validate_payload = tool_text(validate_result);
    assert!(validate_payload["ok"].is_boolean(), "validate ok is a bool");
    assert!(
        validate_payload["errors"].is_number(),
        "validate has an errors count"
    );
    assert!(
        validate_payload["warnings"].is_number(),
        "validate has a warnings count"
    );
    assert!(
        validate_payload["infos"].is_number(),
        "validate has an infos count"
    );
    assert!(
        validate_payload["findings"].is_array(),
        "validate has a findings array"
    );
    assert!(
        validate_payload["skipped"].is_array(),
        "validate has a skipped array"
    );
    // isError must mirror validate-with-errors semantics (DESIGN §10).
    let has_errors = validate_payload["errors"].as_u64().unwrap_or(0) > 0;
    assert_eq!(
        validate_result["isError"], has_errors,
        "validate isError tracks the error count"
    );

    // 5. Close stdin → EOF → clean exit.
    session.finish();
}

/// A driven MCP child process with a line-buffered, timeout-guarded reader.
struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Session {
    fn spawn() -> Session {
        let mut child = Command::new(env!("CARGO_BIN_EXE_treesmith"))
            .args(["mcp", "--root", "fixtures/rainbow/basic"])
            .current_dir(workspace_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn treesmith mcp");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        // Reader thread: forward each stdout line onto the channel. The
        // channel closes when the child's stdout hits EOF.
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Session {
            child,
            stdin,
            lines: rx,
        }
    }

    /// Sends a request and returns the next response line parsed as JSON.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.read_line()
    }

    /// Sends a notification (no `id`); never expects a response.
    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn send(&mut self, message: Value) {
        let line = serde_json::to_string(&message).expect("serialize request");
        self.stdin
            .write_all(line.as_bytes())
            .expect("write request");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush request");
    }

    /// Reads the next response line under [`LINE_TIMEOUT`]; a hang fails
    /// the test rather than freezing it.
    fn read_line(&mut self) -> Value {
        match self.lines.recv_timeout(LINE_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("response line is not JSON ({e}): {line}")),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                panic!("timed out waiting for an MCP response (server hang)");
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("MCP server closed stdout before responding");
            }
        }
    }

    /// Closes stdin and asserts the child exits cleanly within the timeout.
    fn finish(self) {
        // Dropping stdin sends EOF.
        drop(self.stdin);

        let (tx, rx) = mpsc::channel();
        // We cannot move `child` into the thread and still own it, so poll
        // for exit on a deadline via a short-lived waiter thread.
        let mut child = self.child;
        thread::spawn(move || {
            let status = child.wait();
            let _ = tx.send(status);
        });
        match rx.recv_timeout(LINE_TIMEOUT) {
            Ok(Ok(status)) => assert!(
                status.success(),
                "MCP server exited with failure status: {status:?}"
            ),
            Ok(Err(e)) => panic!("waiting on MCP child failed: {e}"),
            Err(_) => panic!("MCP server did not exit after stdin close"),
        }

        // Drain any trailing lines so the reader thread can finish.
        while self.lines.try_recv().is_ok() {}
    }
}

/// Extracts and parses the single text-content block of a tool result into
/// its JSON payload (DESIGN §10: `content[0].text` is a JSON string).
fn tool_text(tool_result: &Value) -> Value {
    let text = tool_result["content"][0]["text"]
        .as_str()
        .expect("tool result content[0].text is a string");
    serde_json::from_str(text).expect("tool result text parses as JSON")
}

/// The workspace root (this test lives in the root package, so its
/// manifest dir is the workspace root) — fixtures resolve relative to it.
fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
