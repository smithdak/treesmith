//! `SerializationFormat` trait plus the Rainbow and SCS parse/emit codecs.
//! The only crate allowed to name a CMS or serialization dialect (spec I6);
//! byte-identical round-trip is enforced here (spec I2).

pub mod census;
pub mod item;
pub mod rainbow;
pub mod scs;
pub mod valuefmt;
pub mod yaml;

use std::path::{Path, PathBuf};

pub use item::{FieldRef, FieldSlot, LanguageBlock, ParsedItem};
pub use yaml::{scalar_for_new_value, FaultKind, ParseFault};

/// A serialization dialect: sniffing, codec, and physical layout rules.
///
/// Everything CMS-specific lives behind this trait; nothing outside this
/// crate names Sitecore, Unicorn, Rainbow, or SCS (spec I6).
pub trait SerializationFormat: Send + Sync {
    /// Stable registry key: `"rainbow" | "scs"`.
    fn key(&self) -> &'static str;

    /// Whether a file *name* may hold an item (`*.yml`).
    fn sniff_file_name(&self, name: &str) -> bool;

    /// Whether the first bytes look like an item document:
    /// optional BOM, a `---` line, then a line starting `ID: `.
    fn sniff_head(&self, head: &[u8]) -> bool;

    /// Parses item bytes losslessly.
    fn parse(&self, bytes: &[u8]) -> Result<ParsedItem, ParseFault>;

    /// Emits an item back to bytes — byte-identical for unmutated parses.
    fn emit(&self, item: &ParsedItem) -> Vec<u8>;

    /// Physical convention shared by Unicorn and SCS: children live in a
    /// folder named after the parent file stem:
    /// `.../Home.yml` -> `.../Home/<child>.yml`.
    fn child_file_path(&self, parent_file: &Path, child_name: &str) -> PathBuf;
}

/// Directory names never descended into by tree walks.
pub(crate) const EXCLUDED_DIRS: &[&str] = &[".git", "target", "node_modules", "bin", "obj"];

pub(crate) fn is_excluded_dir(name: &str) -> bool {
    EXCLUDED_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d))
}

/// Detects the format of a serialized tree: any `*.module.json` under
/// `root` means SCS, otherwise Rainbow.
pub fn detect(root: &Path) -> &'static dyn SerializationFormat {
    let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
        !(e.file_type().is_dir() && e.file_name().to_str().is_some_and(crate::is_excluded_dir))
    });
    for entry in walker.flatten() {
        if entry.file_type().is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.to_ascii_lowercase().ends_with(".module.json"))
        {
            return &scs::SCS;
        }
    }
    &rainbow::RAINBOW
}

/// Looks a format up by its registry key (`"rainbow" | "scs"`).
pub fn by_key(key: &str) -> Option<&'static dyn SerializationFormat> {
    match key {
        "rainbow" => Some(&rainbow::RAINBOW),
        "scs" => Some(&scs::SCS),
        _ => None,
    }
}

/// Sniffing + layout helpers shared by the Rainbow and SCS impls.
pub(crate) mod sniff {
    use std::path::{Path, PathBuf};

    pub fn is_yml_name(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        name.ends_with(".yml") && name.len() > 4
    }

    pub fn looks_like_item_head(head: &[u8]) -> bool {
        let head = head.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(head);
        let Some(rest) = head.strip_prefix(b"---") else {
            return false;
        };
        let rest = match rest {
            [b'\r', b'\n', r @ ..] => r,
            [b'\n', r @ ..] => r,
            _ => return false,
        };
        rest.starts_with(b"ID: ")
    }

    pub fn child_file_path(parent_file: &Path, child_name: &str) -> PathBuf {
        let stem = parent_file.file_stem().unwrap_or_default();
        let dir = parent_file.parent().unwrap_or_else(|| Path::new(""));
        dir.join(stem).join(format!("{child_name}.yml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_key_registry() {
        assert_eq!(by_key("rainbow").unwrap().key(), "rainbow");
        assert_eq!(by_key("scs").unwrap().key(), "scs");
        assert!(by_key("unicorn").is_none());
        assert!(by_key("").is_none());
    }

    #[test]
    fn sniff_file_name_yml_only() {
        let fmt = by_key("rainbow").unwrap();
        assert!(fmt.sniff_file_name("Home.yml"));
        assert!(fmt.sniff_file_name("HOME.YML"));
        assert!(!fmt.sniff_file_name("Home.yaml"));
        assert!(!fmt.sniff_file_name("Home.yml.bak"));
        assert!(!fmt.sniff_file_name(".yml"));
        assert!(!fmt.sniff_file_name("Home.json"));
    }

    #[test]
    fn sniff_head_variants() {
        let fmt = by_key("rainbow").unwrap();
        assert!(fmt.sniff_head(b"---\nID: \"x\"\n"));
        assert!(fmt.sniff_head(b"---\r\nID: \"x\"\r\n"));
        assert!(fmt.sniff_head(b"\xEF\xBB\xBF---\nID: \"x\"\n"));
        assert!(!fmt.sniff_head(b"---\nName: x\n"));
        assert!(!fmt.sniff_head(b"key: value\n"));
        assert!(!fmt.sniff_head(b"----\nID: \"x\"\n"));
        assert!(!fmt.sniff_head(b"---\nID:\n"), "bare ID: is not item head");
        assert!(!fmt.sniff_head(b""));
        assert!(!fmt.sniff_head(b"---"));
    }

    #[test]
    fn parse_emit_round_trip_via_trait() {
        let src = b"---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nPath: /a\n";
        for key in ["rainbow", "scs"] {
            let fmt = by_key(key).unwrap();
            let item = fmt.parse(src).unwrap();
            assert_eq!(fmt.emit(&item), src.to_vec(), "format {key}");
        }
        let fault = by_key("rainbow").unwrap().parse(b"no marker").unwrap_err();
        assert_eq!(fault.kind, FaultKind::MissingDocMarker);
    }

    #[test]
    fn child_file_path_convention() {
        let fmt = by_key("rainbow").unwrap();
        let parent = Path::new("serialization").join("content").join("Home.yml");
        let child = fmt.child_file_path(&parent, "About");
        assert_eq!(
            child,
            Path::new("serialization")
                .join("content")
                .join("Home")
                .join("About.yml")
        );
    }

    #[test]
    fn detect_scs_vs_rainbow() {
        let dir = testutil::TempDir::new("detect");
        std::fs::create_dir_all(dir.path().join("items")).unwrap();
        std::fs::write(dir.path().join("items").join("Home.yml"), "---\n").unwrap();
        assert_eq!(detect(dir.path()).key(), "rainbow");

        std::fs::write(dir.path().join("Site.module.json"), "{}").unwrap();
        assert_eq!(detect(dir.path()).key(), "scs");
    }

    #[test]
    fn detect_ignores_module_json_in_excluded_dirs() {
        let dir = testutil::TempDir::new("detect-excluded");
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("node_modules").join("x.module.json"), "{}").unwrap();
        assert_eq!(detect(dir.path()).key(), "rainbow");
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A unique temp directory removed on drop (no external dev-deps).
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new(label: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "treesmith-format-{label}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
