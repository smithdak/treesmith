//! The Rainbow (Unicorn) serialization format.

use std::path::{Path, PathBuf};

use crate::{sniff, SerializationFormat};
use crate::{yaml, ParseFault, ParsedItem};

/// Rainbow YAML items as written by Unicorn.
pub struct RainbowFormat;

/// The singleton [`RainbowFormat`] instance behind the registry.
pub static RAINBOW: RainbowFormat = RainbowFormat;

impl SerializationFormat for RainbowFormat {
    fn key(&self) -> &'static str {
        "rainbow"
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
