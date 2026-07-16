//! The Sitecore Content Serialization (SCS) format.
//!
//! SCS item files use the same codec as Rainbow (VERIFY-P0); the two
//! implementations differ only in `key()` and, later, discovery nuances.

use std::path::{Path, PathBuf};

use crate::{sniff, SerializationFormat};
use crate::{yaml, ParseFault, ParsedItem};

/// SCS YAML items as written by the Sitecore CLI.
pub struct ScsFormat;

/// The singleton [`ScsFormat`] instance behind the registry.
pub static SCS: ScsFormat = ScsFormat;

impl SerializationFormat for ScsFormat {
    fn key(&self) -> &'static str {
        "scs"
    }

    fn sniff_file_name(&self, name: &str) -> bool {
        sniff::is_yml_name(name)
    }

    fn sniff_head(&self, head: &[u8]) -> bool {
        sniff::looks_like_item_head(head)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedItem, ParseFault> {
        Ok(ParsedItem {
            doc: yaml::parse(bytes)?,
        })
    }

    fn emit(&self, item: &ParsedItem) -> Vec<u8> {
        yaml::emit(&item.doc)
    }

    fn child_file_path(&self, parent_file: &Path, child_name: &str) -> PathBuf {
        sniff::child_file_path(parent_file, child_name)
    }
}
