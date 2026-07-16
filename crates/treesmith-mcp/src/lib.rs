//! MCP server surface (spec §3.3): a hand-rolled, newline-delimited
//! JSON-RPC 2.0 server over stdio, owning the warm workspace graph. Thin
//! over `treesmith-kernel`.
//!
//! **O6 spike outcome (also recorded in the README):** we hand-roll
//! JSON-RPC instead of pulling in `rmcp` — zero async runtime, no
//! API-drift risk, and the whole protocol surface is four methods
//! (`initialize`, `ping`, `tools/list`, `tools/call`, plus the ignored
//! `notifications/initialized`). `rmcp` stays a clean later swap because
//! this crate is the only MCP-aware code in the workspace.
//!
//! The pure protocol layer (routing, error codes, version echo) lives in
//! [`protocol`] and is unit-tested there without spawning a process. This
//! module owns the I/O loop, the [`Workspace`] behind a `Mutex`, and the
//! `notify` watcher that keeps the graph warm (spec §3.3).

mod protocol;
mod tools;

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use treesmith_kernel::Workspace;

use protocol::{Routed, SERVER_NAME};

/// Shared set of filesystem paths reported dirty by the watcher since the
/// last drain. Guarded by its own mutex so the watcher thread and the
/// request loop never contend on the `Workspace` lock.
type DirtySet = Arc<Mutex<BTreeSet<PathBuf>>>;

/// Launches the MCP server on stdio, serving requests until stdin reaches
/// EOF (a clean exit, spec §10).
///
/// Owns a [`Workspace`] behind a `Mutex` and starts a `notify` watcher
/// thread that collects dirty paths into a shared set; each `tools/call`
/// drains that set and `refresh_paths` first so reads run against a warm
/// graph (spec §3.3). If the watcher cannot start, we log to stderr **once**
/// and fall back to rebuilding the whole workspace before every call.
pub fn serve(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = Workspace::open(root)?;
    let workspace = Arc::new(Mutex::new(workspace));

    let dirty: DirtySet = Arc::new(Mutex::new(BTreeSet::new()));
    // `Some(_watcher)` keeps the watcher alive for the process lifetime;
    // `None` means the watcher failed to start and we rebuild per call.
    let watcher = start_watcher(root, Arc::clone(&dirty));
    let rebuild_per_call = watcher.is_none();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            break; // EOF → clean exit.
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(trimmed) {
            Err(_) => Some(protocol::parse_error()),
            Ok(message) => handle_message(&message, &workspace, &dirty, rebuild_per_call),
        };

        if let Some(response) = response {
            write_message(&mut out, &response)?;
        }
    }

    Ok(())
}

/// Routes one parsed message and, for `tools/call`, executes the tool
/// against the workspace (draining the dirty set first). Returns the
/// response to send, or `None` for notifications (never answered).
fn handle_message(
    message: &Value,
    workspace: &Arc<Mutex<Workspace>>,
    dirty: &DirtySet,
    rebuild_per_call: bool,
) -> Option<Value> {
    match protocol::route(message) {
        Routed::Respond(value) => Some(value),
        Routed::Ignore => None,
        Routed::ToolCall {
            id,
            name,
            arguments,
        } => {
            let mut ws = workspace.lock().expect("workspace mutex poisoned");
            refresh_before_call(&mut ws, dirty, rebuild_per_call);
            let (text, is_error) = tools::dispatch(&mut ws, &name, &arguments);
            Some(protocol::success(id, protocol::tool_result(text, is_error)))
        }
    }
}

/// Warms the graph before a `tools/call`: drain the watcher's dirty set
/// and `refresh_paths`, or — if the watcher never started — rebuild the
/// whole workspace (spec §10 fallback).
fn refresh_before_call(workspace: &mut Workspace, dirty: &DirtySet, rebuild_per_call: bool) {
    if rebuild_per_call {
        workspace.rebuild();
        return;
    }
    let drained: Vec<PathBuf> = {
        let mut set = dirty.lock().expect("dirty set mutex poisoned");
        std::mem::take(&mut *set).into_iter().collect()
    };
    if !drained.is_empty() {
        workspace.refresh_paths(&drained);
    }
}

/// Serializes a JSON-RPC message as one newline-terminated line.
fn write_message(out: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let line = serde_json::to_string(value).expect("json value serializes");
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Starts a `notify` watcher over `root`, feeding changed paths into
/// `dirty`. Returns the watcher (kept alive by the caller) on success, or
/// `None` after logging once to stderr on failure.
fn start_watcher(root: &Path, dirty: DirtySet) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(err) => {
            eprintln!(
                "{SERVER_NAME}: filesystem watcher unavailable ({err}); \
                 rebuilding the workspace before every call"
            );
            return None;
        }
    };
    if let Err(err) = watcher.watch(root, RecursiveMode::Recursive) {
        eprintln!(
            "{SERVER_NAME}: filesystem watcher unavailable ({err}); \
             rebuilding the workspace before every call"
        );
        return None;
    }

    // Drain watcher events into the shared dirty set on a background thread.
    std::thread::spawn(move || {
        for event in rx {
            let Ok(event) = event else { continue };
            if let Ok(mut set) = dirty.lock() {
                for path in event.paths {
                    set.insert(path);
                }
            }
        }
    });

    Some(watcher)
}
