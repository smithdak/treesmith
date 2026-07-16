//! The CLI verb surface (spec §3.2, DESIGN.md §9). Thin over
//! [`treesmith_kernel::Workspace`]: every verb parses arguments, calls one
//! kernel op, and shapes output. All logic lives in the kernel (spec §2
//! rule 3: surfaces are thin, and `cli` never imports `mcp` — the `mcp`
//! verb returns [`CliOutcome::LaunchMcp`] for the root binary to bridge).
//!
//! Output contract (spec §3.2): pretty JSON on stdout when stdout is not a
//! TTY (`std::io::IsTerminal`) or `--json`; human-readable lines otherwise;
//! diagnostics to stderr only. Exit codes: `0` success · `1`
//! gate/validation failure · `2` usage (clap errors already exit 2, kernel
//! `Usage` maps to 2) · `3` tree unreadable.

mod human;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde_json::Value;

use treesmith_kernel::{ForgeRequest, KernelError, MoveRequest, SetFieldRequest, Workspace};

/// What [`run`] resolved the invocation to: an exit code, or a request for
/// the root binary to launch the MCP server (the `cli` crate never imports
/// `mcp`; the root binary bridges — DESIGN.md §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliOutcome {
    /// Terminate with this process exit code.
    Exit(u8),
    /// Hand off to `treesmith mcp` (server owned by the `mcp` crate).
    LaunchMcp {
        /// The resolved `--root` directory.
        root: PathBuf,
    },
}

/// treesmith — agent-first content kernel for serialized CMS trees.
#[derive(Parser, Debug)]
#[command(
    name = "treesmith",
    about = "Agent-first content kernel for serialized CMS trees.",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    /// Repository root (default: current directory).
    #[arg(long, global = true, default_value = ".", value_name = "DIR")]
    root: PathBuf,

    /// Force JSON output even on a TTY.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Path/template/field predicates over the graph.
    Query {
        /// Query expression, e.g. `path:/sitecore/content/** template:Page`.
        expr: String,
    },
    /// Item with resolved effective fields.
    Get {
        /// Item designator: a GUID or a `/sitecore/...` path.
        item: String,
    },
    /// Single-field mutation, template-validated.
    SetField {
        /// Item designator: a GUID or a `/sitecore/...` path.
        item: String,
        /// Field designator: name (via the effective template) or GUID.
        field: String,
        /// The raw value to store.
        value: String,
        /// Language for unversioned/versioned fields (default `en`).
        #[arg(long)]
        language: Option<String>,
        /// Version for versioned fields (default: max existing).
        #[arg(long)]
        version: Option<u32>,
        /// Do not create version 1 when the language has none.
        #[arg(long)]
        no_create_version: bool,
    },
    /// Create an item from a template (GUID-safe, section-correct).
    Forge {
        /// Template designator: GUID, path, or template name.
        template: String,
        /// Parent item designator (must be serialized).
        parent: String,
        /// New item name (one path segment).
        name: String,
        /// Explicit GUID (default: a random v4 GUID).
        #[arg(long)]
        id: Option<String>,
        /// Create version 1 in this language.
        #[arg(long)]
        language: Option<String>,
    },
    /// Structure-safe relocation with path/reference updates.
    Move {
        /// Item designator: a GUID or a `/sitecore/...` path.
        item: String,
        /// New parent designator (must be serialized).
        new_parent: String,
        /// Rename while moving (default: keep the current name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Placeholder/rendering tree with datasources and code files.
    ResolvePresentation {
        /// Item designator: a GUID or a `/sitecore/...` path.
        item: String,
        /// Language (default: first language with versions, else `en`).
        #[arg(long)]
        language: Option<String>,
        /// Version (default: max for the language).
        #[arg(long)]
        version: Option<u32>,
    },
    /// Run the gate engine; pre-commit-hook compatible.
    Validate {
        /// Restrict to these gates (repeatable), e.g. `--gate G1 --gate G5`.
        #[arg(long = "gate", value_name = "NAME")]
        gate: Vec<String>,
    },
    /// Round-trip fidelity census (the P0 harness).
    Census,
    /// Launch the persistent MCP server.
    Mcp,
}

/// Parses `std::env::args`, runs one verb, and returns the outcome. clap
/// usage errors (and `--help`/`--version`) are handled internally: they
/// print to the right stream and yield the correct exit code (2 for usage).
pub fn run() -> CliOutcome {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // clap prints help/version to stdout (exit 0) and usage errors
            // to stderr (exit 2). Route through a code so the root binary
            // owns process exit.
            let code = if err.use_stderr() { 2 } else { 0 };
            let _ = err.print();
            return CliOutcome::Exit(code);
        }
    };

    if matches!(cli.command, Command::Mcp) {
        return CliOutcome::LaunchMcp { root: cli.root };
    }

    let json_mode = cli.json || !std::io::stdout().is_terminal();
    CliOutcome::Exit(dispatch(&cli, json_mode))
}

/// Runs the (non-`mcp`) verb and returns the process exit code.
fn dispatch(cli: &Cli, json_mode: bool) -> u8 {
    // `census` runs on faulted trees by design and does not open a
    // Workspace the way the other verbs do.
    if matches!(cli.command, Command::Census) {
        let value = Workspace::census(&cli.root);
        let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
        emit(&value, json_mode, human::census);
        // census with faults/mismatches exits 3 (tree unreadable class).
        return if ok { 0 } else { 3 };
    }

    let mut workspace = match Workspace::open(&cli.root) {
        Ok(ws) => ws,
        Err(err) => return emit_error(&err, json_mode),
    };

    match &cli.command {
        Command::Query { expr } => run_read(workspace.query(expr), json_mode, human::query),
        Command::Get { item } => run_read(workspace.get(item), json_mode, human::get),
        Command::ResolvePresentation {
            item,
            language,
            version,
        } => run_read(
            workspace.resolve_presentation(item, language.as_deref(), *version),
            json_mode,
            human::resolve_presentation,
        ),
        Command::Validate { gate } => {
            let gates = (!gate.is_empty()).then(|| gate.clone());
            match workspace.validate(gates.as_deref()) {
                Ok((value, has_errors)) => {
                    emit(&value, json_mode, human::validate);
                    u8::from(has_errors)
                }
                Err(err) => emit_error(&err, json_mode),
            }
        }
        Command::SetField {
            item,
            field,
            value,
            language,
            version,
            no_create_version,
        } => {
            let req = SetFieldRequest {
                item: item.clone(),
                field: field.clone(),
                value: value.clone(),
                language: language.clone(),
                version: *version,
                create_version: !*no_create_version,
            };
            run_mutate(workspace.set_field(&req), json_mode)
        }
        Command::Forge {
            template,
            parent,
            name,
            id,
            language,
        } => {
            let parsed_id = match id.as_deref().map(treesmith_types::Guid::parse) {
                Some(Ok(g)) => Some(g),
                Some(Err(e)) => {
                    let raw = id.as_deref().unwrap_or_default();
                    let err = KernelError::usage(
                        "invalid-designator",
                        format!("invalid --id `{raw}`: {e}"),
                        serde_json::json!({ "id": raw }),
                    );
                    return emit_error(&err, json_mode);
                }
                None => None,
            };
            let req = ForgeRequest {
                template: template.clone(),
                parent: parent.clone(),
                name: name.clone(),
                id: parsed_id,
                language: language.clone(),
            };
            run_mutate(workspace.forge(&req), json_mode)
        }
        Command::Move {
            item,
            new_parent,
            name,
        } => {
            let req = MoveRequest {
                item: item.clone(),
                new_parent: new_parent.clone(),
                name: name.clone(),
            };
            run_mutate(workspace.move_item(&req), json_mode)
        }
        // Handled before opening the workspace.
        Command::Census | Command::Mcp => unreachable!("census/mcp handled earlier"),
    }
}

/// Read verbs: print the value on success (exit 0) or the error (mapped exit
/// code) on failure.
fn run_read(
    result: Result<Value, KernelError>,
    json_mode: bool,
    human: fn(&Value, &mut dyn Write) -> std::io::Result<()>,
) -> u8 {
    match result {
        Ok(value) => {
            emit(&value, json_mode, human);
            0
        }
        Err(err) => emit_error(&err, json_mode),
    }
}

/// Mutate verbs: on success print the mutate shape (exit 0); on failure the
/// error, exit per class (rejected writes are validation → exit 1).
fn run_mutate(result: Result<Value, KernelError>, json_mode: bool) -> u8 {
    match result {
        Ok(value) => {
            emit(&value, json_mode, human::mutate);
            0
        }
        Err(err) => emit_error(&err, json_mode),
    }
}

/// Prints a value: pretty JSON to stdout in JSON mode, else the human
/// rendering. Never mixes diagnostics into stdout.
fn emit(value: &Value, json_mode: bool, human: fn(&Value, &mut dyn Write) -> std::io::Result<()>) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = if json_mode {
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        )
    } else {
        human(value, &mut out)
    };
}

/// Prints a kernel error and returns its exit code. In JSON mode the
/// machine-readable error goes to stdout (it is the operation's result); a
/// one-line diagnostic always goes to stderr.
fn emit_error(err: &KernelError, json_mode: bool) -> u8 {
    if json_mode {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&err.to_json()).unwrap_or_default()
        );
    }
    eprintln!("treesmith: {}: {err}", err.class());
    err.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        // Catches derive-level mistakes (duplicate flags, bad arg config).
        Cli::command().debug_assert();
    }

    #[test]
    fn mcp_verb_parses_and_carries_root() {
        let cli =
            Cli::try_parse_from(["treesmith", "--root", "some/dir", "mcp"]).expect("mcp parses");
        assert!(matches!(cli.command, Command::Mcp));
        assert_eq!(cli.root, PathBuf::from("some/dir"));
    }

    #[test]
    fn global_flags_are_accepted_after_the_verb() {
        // `--root`/`--json` are global, so they may follow the subcommand.
        let cli = Cli::try_parse_from(["treesmith", "query", "path:/**", "--json", "--root", "x"])
            .expect("global flags parse after the verb");
        assert!(cli.json);
        assert_eq!(cli.root, PathBuf::from("x"));
        assert!(matches!(cli.command, Command::Query { .. }));
    }

    #[test]
    fn root_defaults_to_current_dir() {
        let cli = Cli::try_parse_from(["treesmith", "census"]).expect("census parses");
        assert_eq!(cli.root, PathBuf::from("."));
        assert!(!cli.json);
    }

    #[test]
    fn unknown_verb_is_a_parse_error() {
        let err = Cli::try_parse_from(["treesmith", "nope"]).unwrap_err();
        // clap usage errors route to stderr (→ exit 2 in `run`).
        assert!(err.use_stderr());
    }

    #[test]
    fn help_routes_to_stdout() {
        let err = Cli::try_parse_from(["treesmith", "--help"]).unwrap_err();
        // Help/version go to stdout (→ exit 0 in `run`).
        assert!(!err.use_stderr());
    }
}
