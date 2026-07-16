//! MCP server surface (spec §3.3): newline-delimited JSON-RPC over stdio,
//! owning the warm workspace graph. Thin over `treesmith-kernel`.

pub fn serve(_root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
