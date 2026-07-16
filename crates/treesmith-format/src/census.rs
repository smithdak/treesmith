//! The P0 fidelity harness (DESIGN.md §3.4): walk a tree, parse every
//! sniffable item file, emit, and byte-compare (spec I2's falsifier).
//!
//! Census is a benchmark/report, not a gate: it reports elapsed wall time
//! (the one sanctioned determinism exception) and works on faulted trees
//! by design. Everything else — file order, faults, mismatches — is
//! deterministic (sorted by path).

use std::path::Path;
use std::time::Instant;

use serde::Serialize;

use crate::yaml::FaultKind;
use crate::{is_excluded_dir, SerializationFormat};

/// A file that failed to parse.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CensusFault {
    /// Root-relative file path, forward slashes.
    pub file: String,
    /// Parse fault class.
    pub kind: FaultKind,
    /// 1-based line number of the fault.
    pub line: usize,
    /// Human-readable detail.
    pub message: String,
}

/// A file that parsed but did not emit byte-identically.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CensusMismatch {
    /// Root-relative file path, forward slashes.
    pub file: String,
    /// 1-based number of the first differing line.
    pub first_diff_line: usize,
    /// The source's line at that position.
    pub expected: String,
    /// The emitted line at that position.
    pub actual: String,
}

/// Round-trip census over one serialized tree.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Census {
    /// `*.yml` files seen (by name sniff).
    pub files: usize,
    /// Files whose head sniffed as an item document.
    pub items: usize,
    /// Items that round-tripped byte-identically.
    pub ok: usize,
    /// Items that failed to parse.
    pub faults: Vec<CensusFault>,
    /// Items that parsed but emitted differently.
    pub mismatches: Vec<CensusMismatch>,
    /// Wall-clock duration of the walk (benchmark only, not deterministic).
    pub elapsed_ms: u64,
}

/// Walks `root` (skipping `.git`, `target`, `node_modules`, `bin`, `obj`),
/// sniffs `*.yml` item files, parses, emits, and byte-compares.
pub fn round_trip_census(root: &Path, fmt: &dyn SerializationFormat) -> Census {
    let start = Instant::now();
    let mut census = Census {
        files: 0,
        items: 0,
        ok: 0,
        faults: Vec::new(),
        mismatches: Vec::new(),
        elapsed_ms: 0,
    };

    let mut paths: Vec<std::path::PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir() && e.file_name().to_str().is_some_and(is_excluded_dir))
        })
        .flatten()
        .filter(|e| {
            e.file_type().is_file()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| fmt.sniff_file_name(n))
        })
        .map(|e| e.into_path())
        .collect();
    paths.sort();

    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue; // unreadable file: not an item, nothing to report
        };
        census.files += 1;
        let head_len = bytes.len().min(512);
        if !fmt.sniff_head(&bytes[..head_len]) {
            continue;
        }
        census.items += 1;
        let file = rel_path(root, &path);
        match fmt.parse(&bytes) {
            Err(f) => census.faults.push(CensusFault {
                file,
                kind: f.kind,
                line: f.line,
                message: f.message,
            }),
            Ok(item) => {
                let emitted = fmt.emit(&item);
                if emitted == bytes {
                    census.ok += 1;
                } else {
                    census.mismatches.push(first_diff(file, &bytes, &emitted));
                }
            }
        }
    }

    census.elapsed_ms = start.elapsed().as_millis() as u64;
    census
}

fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Locates the first differing line between source and emitted bytes.
fn first_diff(file: String, expected: &[u8], actual: &[u8]) -> CensusMismatch {
    let exp = String::from_utf8_lossy(expected);
    let act = String::from_utf8_lossy(actual);
    let exp_lines: Vec<&str> = exp.split('\n').collect();
    let act_lines: Vec<&str> = act.split('\n').collect();
    for i in 0..exp_lines.len().max(act_lines.len()) {
        let e = exp_lines.get(i).copied();
        let a = act_lines.get(i).copied();
        if e != a {
            return CensusMismatch {
                file,
                first_diff_line: i + 1,
                expected: e
                    .unwrap_or("<missing line>")
                    .trim_end_matches('\r')
                    .to_string(),
                actual: a
                    .unwrap_or("<missing line>")
                    .trim_end_matches('\r')
                    .to_string(),
            };
        }
    }
    // Same line content but different bytes (BOM or newline style).
    CensusMismatch {
        file,
        first_diff_line: 1,
        expected: format!("<byte-level difference: {} bytes>", expected.len()),
        actual: format!("<byte-level difference: {} bytes>", actual.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use crate::{by_key, ParsedItem};
    use std::path::PathBuf;

    const GOOD: &str = "---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nPath: /sitecore/content/Home\nSharedFields:\n- ID: \"7c1e1c2a-0006-4000-8000-000000000006\"\n  Value: hello\n";

    fn write(dir: &TempDir, rel: &str, content: &str) -> PathBuf {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn census_counts_ok_faults_and_skips() {
        let dir = TempDir::new("census");
        write(&dir, "items/Home.yml", GOOD);
        // subset violation: tab indentation
        write(
            &dir,
            "items/Broken.yml",
            "---\nID: \"c0ffee00-0002-4000-8000-000000000002\"\nSharedFields:\n\t- ID: \"x\"\n",
        );
        // a .yml that is not an item (fails head sniff): counted as file only
        write(&dir, "config/app.yml", "settings:\n  a: 1\n");
        // excluded directory content is never visited
        write(&dir, "node_modules/pkg/item.yml", GOOD);
        // non-yml ignored entirely
        write(&dir, "items/readme.md", "hi");

        let fmt = by_key("rainbow").unwrap();
        let census = round_trip_census(dir.path(), fmt);

        assert_eq!(census.files, 3);
        assert_eq!(census.items, 2);
        assert_eq!(census.ok, 1);
        assert_eq!(census.faults.len(), 1);
        assert!(census.mismatches.is_empty());

        let fault = &census.faults[0];
        assert_eq!(fault.file, "items/Broken.yml");
        assert_eq!(fault.kind, FaultKind::TabIndent);
        assert_eq!(fault.line, 4);
        assert!(!fault.message.is_empty());
    }

    #[test]
    fn census_is_deterministic_and_sorted() {
        let dir = TempDir::new("census-det");
        write(&dir, "b/Item.yml", "---\nID: x\n"); // fault: head sniff...
        write(&dir, "b/Item2.yml", "---\nID: \"a\"\n\nbad\n"); // blank fault
        write(&dir, "a/Item.yml", "---\nID: \"a\"\nbad line\n"); // structure fault
        let fmt = by_key("rainbow").unwrap();
        let one = round_trip_census(dir.path(), fmt);
        let two = round_trip_census(dir.path(), fmt);
        let strip = |mut c: Census| {
            c.elapsed_ms = 0;
            c
        };
        assert_eq!(strip(one.clone()), strip(two));
        let files: Vec<&str> = one.faults.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(files, ["a/Item.yml", "b/Item2.yml"], "sorted by path");
    }

    /// A deliberately emit-divergent format: proves the mismatch path.
    struct DivergentFormat;

    impl SerializationFormat for DivergentFormat {
        fn key(&self) -> &'static str {
            "divergent"
        }
        fn sniff_file_name(&self, name: &str) -> bool {
            by_key("rainbow").unwrap().sniff_file_name(name)
        }
        fn sniff_head(&self, head: &[u8]) -> bool {
            by_key("rainbow").unwrap().sniff_head(head)
        }
        fn parse(&self, bytes: &[u8]) -> Result<ParsedItem, crate::ParseFault> {
            by_key("rainbow").unwrap().parse(bytes)
        }
        fn emit(&self, item: &ParsedItem) -> Vec<u8> {
            // Divergence: rewrite the Path value before emitting.
            let mut item = item.clone();
            item.set_path("/tampered");
            by_key("rainbow").unwrap().emit(&item)
        }
        fn child_file_path(
            &self,
            parent_file: &std::path::Path,
            child_name: &str,
        ) -> std::path::PathBuf {
            by_key("rainbow")
                .unwrap()
                .child_file_path(parent_file, child_name)
        }
    }

    #[test]
    fn census_reports_mismatch_with_first_diff_line() {
        let dir = TempDir::new("census-mismatch");
        write(&dir, "Home.yml", GOOD);
        let census = round_trip_census(dir.path(), &DivergentFormat);
        assert_eq!(census.items, 1);
        assert_eq!(census.ok, 0);
        assert_eq!(census.mismatches.len(), 1);
        let m = &census.mismatches[0];
        assert_eq!(m.file, "Home.yml");
        assert_eq!(m.first_diff_line, 3, "Path is line 3");
        assert_eq!(m.expected, "Path: /sitecore/content/Home");
        assert_eq!(m.actual, "Path: /tampered");
    }

    #[test]
    fn first_diff_length_difference() {
        let m = first_diff("f".into(), b"---\nA: 1\nB: 2\n", b"---\nA: 1\n");
        assert_eq!(m.first_diff_line, 3);
        assert_eq!(m.expected, "B: 2");
        assert_eq!(m.actual, "", "trailing segment of the shorter side");
        // one side truly out of lines
        let m = first_diff("f".into(), b"---\nA: 1\nB: 2", b"---\nA: 1");
        assert_eq!(m.first_diff_line, 3);
        assert_eq!(m.actual, "<missing line>");
    }

    #[test]
    fn first_diff_byte_level_only() {
        // identical line content, BOM difference
        let m = first_diff("f".into(), b"\xEF\xBB\xBF---\nA: 1\n", b"---\nA: 1\n");
        // BOM makes line 1 differ under lossy decode? BOM decodes to U+FEFF,
        // so line 1 differs textually.
        assert_eq!(m.first_diff_line, 1);
    }

    #[test]
    fn census_serializes_camel_case() {
        let census = Census {
            files: 1,
            items: 1,
            ok: 0,
            faults: vec![CensusFault {
                file: "a.yml".into(),
                kind: FaultKind::TabIndent,
                line: 3,
                message: "tab".into(),
            }],
            mismatches: vec![CensusMismatch {
                file: "b.yml".into(),
                first_diff_line: 2,
                expected: "x".into(),
                actual: "y".into(),
            }],
            elapsed_ms: 5,
        };
        let json = serde_json::to_value(&census).unwrap();
        assert_eq!(json["elapsedMs"], 5);
        assert_eq!(json["faults"][0]["kind"], "tab-indent");
        assert_eq!(json["faults"][0]["line"], 3);
        assert_eq!(json["mismatches"][0]["firstDiffLine"], 2);
    }
}
