use std::process::ExitCode;

/// Single `treesmith` binary (spec I7). The CLI crate owns the verb surface;
/// `treesmith mcp` hands off to the MCP server crate. The root binary is the
/// only place both surfaces meet (spec §2 rule 4: no surface-to-surface imports).
fn main() -> ExitCode {
    match treesmith_cli::run() {
        treesmith_cli::CliOutcome::Exit(code) => ExitCode::from(code),
        treesmith_cli::CliOutcome::LaunchMcp { root } => match treesmith_mcp::serve(&root) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("treesmith mcp: {err}");
                ExitCode::from(1)
            }
        },
    }
}
