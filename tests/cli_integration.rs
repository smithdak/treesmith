//! Root-package integration tests for the `treesmith` CLI (DESIGN.md §9 /
//! §14). Drives the built binary via `CARGO_BIN_EXE_treesmith` +
//! `std::process::Command` with `--json`, asserting exit codes 0/1/2/3, that
//! output JSON parses with the documented keys, and that `set-field` on a
//! temp copy of a fixture changes the file while keeping the census clean.
//!
//! std-only (no dev-deps beyond the root package's `serde_json`). Mutation
//! tests copy fixtures into unique `std::env::temp_dir()` directories and
//! clean up; `fixtures/` is never mutated in place.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

/// The workspace root — this test crate lives at the workspace root, so
/// `CARGO_MANIFEST_DIR` *is* the root that holds `fixtures/`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(rel: &str) -> PathBuf {
    workspace_root().join("fixtures").join(rel)
}

/// Runs `treesmith --root <root> --json <args...>` and returns the output.
fn run_json(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_treesmith"));
    cmd.arg("--root")
        .arg(root)
        .arg("--json")
        .args(args)
        // Force JSON regardless of the harness's stdout wiring; belt and
        // braces alongside `--json`.
        .env("NO_COLOR", "1");
    cmd.output().expect("spawn treesmith")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("process exited with a code")
}

fn parse_stdout(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not valid JSON: {e}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    })
}

/// A unique temp directory under `std::env::temp_dir()`, removed on drop.
struct TempRepo {
    path: PathBuf,
}

impl TempRepo {
    fn new(tag: &str) -> TempRepo {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("treesmith-cli-{tag}-{pid}-{n}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp repo dir");
        TempRepo { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Recursively copies a directory tree (std-only).
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dest dir");
    for entry in std::fs::read_dir(from).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy file");
        }
    }
}

#[test]
fn query_basic_exits_zero_and_lists_items() {
    let out = run_json(&fixture("rainbow/basic"), &["query", "path:/**"]);
    assert_eq!(exit_code(&out), 0, "query should succeed on a healthy tree");

    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    assert!(v.get("count").and_then(Value::as_u64).unwrap() > 0);
    let items = v["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "basic fixture has items");
    // ItemSummary keys (DESIGN.md §8).
    let first = &items[0];
    for key in ["id", "path", "name", "template", "db", "languages", "file"] {
        assert!(first.get(key).is_some(), "ItemSummary missing key `{key}`");
    }
}

#[test]
fn get_basic_exits_zero_with_item_detail() {
    // Home is a stable, well-known GUID in the basic fixture (DESIGN.md §13).
    let out = run_json(
        &fixture("rainbow/basic"),
        &["get", "/sitecore/content/Home"],
    );
    assert_eq!(exit_code(&out), 0);

    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    let item = &v["item"];
    // ItemDetail = ItemSummary + these (DESIGN.md §8).
    for key in [
        "id",
        "path",
        "templateChain",
        "sharedFields",
        "languages",
        "fieldsNotInTemplate",
    ] {
        assert!(item.get(key).is_some(), "ItemDetail missing key `{key}`");
    }
    assert_eq!(item["path"], Value::String("/sitecore/content/Home".into()));
}

#[test]
fn validate_basic_exits_zero_and_is_clean() {
    let out = run_json(&fixture("rainbow/basic"), &["validate"]);
    assert_eq!(exit_code(&out), 0, "healthy tree validates clean");

    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    assert_eq!(v["errors"], Value::from(0));
    for key in ["ok", "errors", "warnings", "infos", "findings", "skipped"] {
        assert!(v.get(key).is_some(), "validate missing key `{key}`");
    }
}

#[test]
fn census_basic_exits_zero_and_round_trips() {
    let out = run_json(&fixture("rainbow/basic"), &["census"]);
    assert_eq!(
        exit_code(&out),
        0,
        "basic fixture round-trips byte-identical"
    );

    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    for key in [
        "ok",
        "files",
        "items",
        "roundTripOk",
        "faults",
        "mismatches",
        "elapsedMs",
    ] {
        assert!(v.get(key).is_some(), "census missing key `{key}`");
    }
    assert!(v["faults"].as_array().unwrap().is_empty());
    assert!(v["mismatches"].as_array().unwrap().is_empty());
}

#[test]
fn validate_broken_exits_one_with_findings() {
    let out = run_json(&fixture("rainbow/broken"), &["validate"]);
    assert_eq!(
        exit_code(&out),
        1,
        "the broken fixture must fail validation (gate/validation class)"
    );

    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(false));
    let findings = v["findings"].as_array().expect("findings array");
    assert!(!findings.is_empty(), "broken fixture has findings");
    assert!(
        v["errors"].as_u64().unwrap() > 0,
        "broken fixture has at least one error finding"
    );
}

#[test]
fn bad_verb_exits_two() {
    // clap usage errors exit 2 (DESIGN.md §9).
    let out = run_json(&fixture("rainbow/basic"), &["no-such-verb"]);
    assert_eq!(exit_code(&out), 2, "unknown verb is a usage error");
}

#[test]
fn missing_required_arg_exits_two() {
    // `get` requires an item designator; omitting it is a clap usage error.
    let out = run_json(&fixture("rainbow/basic"), &["get"]);
    assert_eq!(
        exit_code(&out),
        2,
        "missing required argument is a usage error"
    );
}

#[test]
fn garbage_yml_exits_three() {
    // A sniffable `.yml` (starts with `---`, second line `ID: ...`) that
    // then violates the codec (tab indentation) is a parse fault → the
    // tree-unreadable class (exit 3), never silently skipped (spec §3.4).
    let repo = TempRepo::new("garbage");
    let ser = repo.path().join("serialization");
    std::fs::create_dir_all(&ser).expect("create serialization dir");
    std::fs::write(ser.join("garbage.yml"), "---\nID: \"abc\"\n\tTabbed: bad\n")
        .expect("write garbage yml");

    // A read op refuses a faulted tree with exit 3.
    let out = run_json(repo.path(), &["query", "path:/**"]);
    assert_eq!(exit_code(&out), 3, "faulted tree is exit 3 for read ops");
    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(false));
    assert_eq!(v["error"]["class"], Value::String("tree-fault".into()));

    // Census diagnoses the same fault and also exits 3.
    let out = run_json(repo.path(), &["census"]);
    assert_eq!(exit_code(&out), 3, "census with faults is exit 3");
    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(false));
    assert!(
        !v["faults"].as_array().unwrap().is_empty(),
        "census reports the parse fault"
    );
}

#[test]
fn set_field_on_copy_changes_file_and_census_stays_clean() {
    // Never mutate `fixtures/` in place: copy `basic` to a temp repo.
    let repo = TempRepo::new("setfield");
    copy_dir(&fixture("rainbow/basic"), repo.path());

    let home_file = repo.path().join("serialization/content/Home.yml");
    let before = std::fs::read_to_string(&home_file).expect("read Home.yml");
    let sentinel = "Integration Test Title";
    assert!(
        !before.contains(sentinel),
        "sentinel must not pre-exist in the fixture"
    );

    // Title is a versioned field on Home (DESIGN.md §13). set-field must
    // succeed (exit 0), self-check ok, and report the changed file.
    let out = run_json(
        repo.path(),
        &[
            "set-field",
            "/sitecore/content/Home",
            "Title",
            sentinel,
            "--language",
            "en",
            "--version",
            "1",
        ],
    );
    assert_eq!(
        exit_code(&out),
        0,
        "set-field on a valid versioned field succeeds\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    assert_eq!(v["selfCheck"], Value::String("ok".into()));
    let changed = v["changedFiles"].as_array().expect("changedFiles array");
    assert!(
        changed
            .iter()
            .any(|f| f.as_str() == Some("serialization/content/Home.yml")),
        "the mutated file is reported changed"
    );

    // The file actually changed on disk.
    let after = std::fs::read_to_string(&home_file).expect("re-read Home.yml");
    assert_ne!(before, after, "Home.yml must have changed on disk");
    assert!(after.contains(sentinel), "the new value is present on disk");

    // The mutation must keep the tree byte-identical round-trippable
    // (I2/I3): census on the copy stays clean, exit 0.
    let out = run_json(repo.path(), &["census"]);
    assert_eq!(
        exit_code(&out),
        0,
        "census stays clean after a schema-aware write"
    );
    let v = parse_stdout(&out);
    assert_eq!(v["ok"], Value::Bool(true));
    assert!(v["faults"].as_array().unwrap().is_empty());
    assert!(v["mismatches"].as_array().unwrap().is_empty());
}
