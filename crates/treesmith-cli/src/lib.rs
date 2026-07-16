//! CLI verb surface (spec §3.2): argument parsing and output shaping only,
//! thin over `treesmith-kernel`. The root binary bridges `LaunchMcp` to the
//! MCP crate so the two surfaces never import each other (spec §2 rule 4).

pub enum CliOutcome {
    Exit(u8),
    LaunchMcp { root: std::path::PathBuf },
}

pub fn run() -> CliOutcome {
    eprintln!("treesmith: not yet implemented");
    CliOutcome::Exit(2)
}
