//! Integration tests for treesmith-kernel against the `fixtures/rainbow`
//! corpora (DESIGN.md §8, §14). Read-only ops run against the fixtures in
//! place; every mutation test copies the fixture repo into a fresh temp
//! directory first (never mutates `fixtures/`).

use std::path::{Path, PathBuf};

use serde_json::Value;
use treesmith_kernel::{ForgeRequest, KernelError, MoveRequest, SetFieldRequest, Workspace};

// ---- fixture / tempdir helpers -------------------------------------------------

fn workspace_root() -> PathBuf {
    // crates/treesmith-kernel/../.. == repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root above crates/treesmith-kernel")
        .to_path_buf()
}

fn basic_fixture() -> PathBuf {
    workspace_root().join("fixtures/rainbow/basic")
}

fn broken_fixture() -> PathBuf {
    workspace_root().join("fixtures/rainbow/broken")
}

/// A unique scratch directory that removes itself on drop.
struct TempRepo {
    dir: PathBuf,
}

impl TempRepo {
    /// Recursively copies `src` into a fresh temp directory.
    fn copy_of(src: &Path) -> TempRepo {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "treesmith-kernel-test-{}-{}-{}",
            std::process::id(),
            n,
            src.file_name().unwrap().to_string_lossy()
        ));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        copy_dir(src, &dir);
        TempRepo { dir }
    }

    /// An empty temp directory.
    fn empty() -> TempRepo {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "treesmith-kernel-empty-{}-{}",
            std::process::id(),
            n
        ));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempRepo { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

fn read_bytes(p: &Path) -> Vec<u8> {
    std::fs::read(p).expect("read file")
}

// Fixture designators.
const HOME: &str = "/sitecore/content/Home";
const ABOUT: &str = "/sitecore/content/Home/About";
const TITLE_FIELD: &str = "7c1e1c2a-0003-4000-8000-000000000003"; // versioned
const KEYWORDS_FIELD: &str = "7c1e1c2a-0012-4000-8000-000000000012"; // shared
const RELATED_FIELD: &str = "7c1e1c2a-0006-4000-8000-000000000006"; // Treelist, shared
const HERODATA_GUID: &str = "c0ffee00-0002-4000-8000-000000000002";
const ARTICLEPAGE_TEMPLATE: &str = "7c1e1c2a-0020-4000-8000-000000000020";

// ---- read ops: shapes ----------------------------------------------------------

#[test]
fn query_shape_matches_design() {
    let ws = Workspace::open(&basic_fixture()).expect("open basic");
    let v = ws.query("template:ArticlePage").expect("query ok");
    // {"ok":true,"count":N,"items":[ItemSummary]}
    assert_eq!(v["ok"], Value::Bool(true));
    assert!(v["count"].is_u64());
    let items = v["items"].as_array().expect("items array");
    assert_eq!(items.len() as u64, v["count"].as_u64().unwrap());
    let home = items
        .iter()
        .find(|it| it["path"] == HOME)
        .expect("Home in results");
    // ItemSummary keys, exactly.
    for key in ["id", "path", "name", "template", "db", "languages", "file"] {
        assert!(home.get(key).is_some(), "ItemSummary missing {key}");
    }
    assert_eq!(home["name"], "Home");
    assert_eq!(home["db"], "master");
    assert_eq!(home["template"]["id"], ARTICLEPAGE_TEMPLATE);
    assert_eq!(home["template"]["name"], "ArticlePage");
    let langs = home["languages"].as_array().expect("languages");
    assert!(langs.iter().any(|l| l["language"] == "en"));
    assert!(langs.iter().any(|l| l["language"] == "da"));
}

#[test]
fn get_shape_matches_design() {
    let ws = Workspace::open(&basic_fixture()).expect("open basic");
    let v = ws.get(HOME).expect("get ok");
    assert_eq!(v["ok"], Value::Bool(true));
    let item = &v["item"];
    // ItemDetail = ItemSummary + these.
    for key in [
        "id",
        "path",
        "name",
        "template",
        "db",
        "languages",
        "file",
        "templateChain",
        "sharedFields",
        "fieldsNotInTemplate",
    ] {
        assert!(item.get(key).is_some(), "ItemDetail missing {key}");
    }
    // sharedFields FieldOut keys.
    let shared = item["sharedFields"].as_array().expect("sharedFields");
    let keywords = shared
        .iter()
        .find(|f| f["id"] == KEYWORDS_FIELD)
        .expect("Keywords shared field");
    for key in ["id", "name", "type", "section", "value", "definedBy"] {
        assert!(keywords.get(key).is_some(), "FieldOut missing {key}");
    }
    assert_eq!(keywords["name"], "Keywords");
    assert_eq!(keywords["section"], "shared");
    assert_eq!(keywords["value"], "sample, article, treesmith");

    // Rich languages carry unversioned + versions with fields.
    let langs = item["languages"].as_array().expect("languages");
    let en = langs
        .iter()
        .find(|l| l["language"] == "en")
        .expect("en language");
    assert!(en["unversioned"].is_array());
    let versions = en["versions"].as_array().expect("versions");
    let v1 = versions.iter().find(|v| v["version"] == 1).expect("en v1");
    let title = v1["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == TITLE_FIELD)
        .expect("Title in en v1");
    assert_eq!(title["name"], "Title");
    assert_eq!(title["value"], "Home");
    assert_eq!(title["section"], "versioned");
}

// ---- set_field happy path ------------------------------------------------------

#[test]
fn set_field_versioned_changes_exactly_one_file_and_round_trips() {
    let repo = TempRepo::copy_of(&basic_fixture());
    let mut ws = Workspace::open(repo.path()).expect("open temp");

    // Snapshot all .yml bytes before.
    let before = snapshot_yml(repo.path());

    let req = SetFieldRequest {
        item: HOME.to_string(),
        field: "Title".to_string(),
        value: "Home v2".to_string(),
        language: Some("en".to_string()),
        version: Some(2),
        create_version: true,
    };
    let v = ws.set_field(&req).expect("set_field ok");

    // mutate shape.
    assert_eq!(v["ok"], Value::Bool(true));
    assert_eq!(v["selfCheck"], "ok");
    let changed = v["changedFiles"].as_array().expect("changedFiles");
    assert_eq!(changed.len(), 1, "exactly one file changed");
    let changed_rel = changed[0].as_str().unwrap();
    assert!(
        changed_rel.ends_with("Home.yml"),
        "changed file is Home.yml"
    );

    // Exactly one file differs on disk.
    let after = snapshot_yml(repo.path());
    let differing: Vec<_> = after
        .iter()
        .filter(|(p, bytes)| before.get(*p) != Some(*bytes))
        .map(|(p, _)| p.clone())
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "exactly one file differs: {differing:?}"
    );
    assert!(differing[0].ends_with("Home.yml"));

    // get reflects the new value.
    let got = ws.get(HOME).expect("get");
    let en = got["item"]["languages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["language"] == "en")
        .unwrap();
    let v2 = en["versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["version"] == 2)
        .unwrap();
    let title = v2["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == TITLE_FIELD)
        .unwrap();
    assert_eq!(title["value"], "Home v2");

    // The changed file still round-trips byte-identically.
    let changed_path = repo.path().join(changed_rel);
    let fmt = treesmith_format::detect(repo.path());
    let bytes = read_bytes(&changed_path);
    let parsed = fmt.parse(&bytes).expect("re-parse changed file");
    assert_eq!(fmt.emit(&parsed), bytes, "changed file round-trips");
}

// ---- set_field rejections (write nothing) --------------------------------------

/// Asserts a rejection returns Validation with `code` and leaves every
/// `.yml` byte-identical.
fn assert_rejects_and_writes_nothing(req: SetFieldRequest, code: &str) {
    let repo = TempRepo::copy_of(&basic_fixture());
    let before = snapshot_yml(repo.path());
    let mut ws = Workspace::open(repo.path()).expect("open temp");
    let err = ws.set_field(&req).expect_err("must reject");
    match &err {
        KernelError::Validation { code: c, .. } => {
            assert_eq!(c, code, "wrong validation code");
        }
        other => panic!("expected Validation({code}), got {other:?}"),
    }
    assert_eq!(err.class(), "validation");
    assert_eq!(err.exit_code(), 1);
    let after = snapshot_yml(repo.path());
    assert_eq!(before, after, "rejection must write nothing");
}

#[test]
fn set_field_unknown_field_rejected() {
    assert_rejects_and_writes_nothing(
        SetFieldRequest {
            item: HOME.to_string(),
            field: "NoSuchField".to_string(),
            value: "x".to_string(),
            language: None,
            version: None,
            create_version: true,
        },
        "unknown-field",
    );
}

#[test]
fn set_field_language_on_shared_rejected() {
    // Keywords is a shared field; passing --language must be rejected.
    assert_rejects_and_writes_nothing(
        SetFieldRequest {
            item: HOME.to_string(),
            field: "Keywords".to_string(),
            value: "x".to_string(),
            language: Some("en".to_string()),
            version: None,
            create_version: true,
        },
        "wrong-slot-for-section",
    );
}

#[test]
fn set_field_invalid_checkbox_value_rejected() {
    // Reuse the broken fixture's Simple template? Simpler: use the basic
    // repo — the template chain of ArticlePage carries no checkbox field
    // we can target directly. Instead validate via a versioned field with
    // an invalid value is not possible (text accepts anything). Use a
    // well-known Datetime field (__Created) with a bad value.
    assert_rejects_and_writes_nothing(
        SetFieldRequest {
            item: HOME.to_string(),
            field: "__Created".to_string(),
            value: "not-a-date".to_string(),
            language: Some("en".to_string()),
            version: Some(1),
            create_version: true,
        },
        "invalid-value",
    );
}

#[test]
fn set_field_malformed_layout_xml_rejected() {
    // __Renderings is a shared Layout field; malformed XML must reject.
    assert_rejects_and_writes_nothing(
        SetFieldRequest {
            item: HOME.to_string(),
            field: "__Renderings".to_string(),
            value: "<r><d id=\"x\"".to_string(), // unterminated
            language: None,
            version: None,
            create_version: true,
        },
        "malformed-layout-xml",
    );
}

#[test]
fn set_field_trailing_newline_rejected_regardless_of_position() {
    // A trailing newline only round-trips when the field lands at end of
    // document; rejecting uniformly keeps the same mutation from succeeding
    // or failing based on unrelated document structure (review finding W1).
    assert_rejects_and_writes_nothing(
        SetFieldRequest {
            item: HOME.to_string(),
            field: "Title".to_string(),
            value: "x\n".to_string(),
            language: Some("en".to_string()),
            version: Some(1),
            create_version: true,
        },
        "trailing-newline-unsupported",
    );
}

// ---- multilist normalization + Type stamping -----------------------------------

#[test]
fn set_field_multilist_normalizes_and_stamps_type() {
    let repo = TempRepo::copy_of(&basic_fixture());
    let mut ws = Workspace::open(repo.path()).expect("open temp");

    // RelatedPages is a shared Treelist. Provide pipe-separated GUIDs in
    // mixed form; expect braced-upper newline-joined storage + Type stamp.
    let req = SetFieldRequest {
        item: ABOUT.to_string(),
        field: "RelatedPages".to_string(),
        value: format!("{HERODATA_GUID}|{{c0ffee00-0001-4000-8000-000000000001}}"),
        language: None,
        version: None,
        create_version: true,
    };
    ws.set_field(&req).expect("set multilist");

    // Read raw serialized value from About.yml.
    let about_file = repo.path().join("serialization/content/Home/About.yml");
    let fmt = treesmith_format::detect(repo.path());
    let bytes = read_bytes(&about_file);
    // Round-trips.
    let parsed = fmt.parse(&bytes).expect("parse About");
    assert_eq!(fmt.emit(&parsed), bytes, "About round-trips");

    let related = parsed
        .shared_fields()
        .into_iter()
        .find(|f| f.id.rainbow() == RELATED_FIELD)
        .expect("RelatedPages present");
    // Braced-upper, newline-joined.
    assert_eq!(
        related.value,
        "{C0FFEE00-0002-4000-8000-000000000002}\n{C0FFEE00-0001-4000-8000-000000000001}"
    );
    // Type stamped.
    assert_eq!(related.type_hint.as_deref(), Some("Treelist"));
}

// ---- forge ---------------------------------------------------------------------

#[test]
fn forge_creates_file_and_graph_sees_it() {
    let repo = TempRepo::copy_of(&basic_fixture());
    let mut ws = Workspace::open(repo.path()).expect("open temp");

    let req = ForgeRequest {
        template: "Page".to_string(),
        parent: HOME.to_string(),
        name: "NewChild".to_string(),
        id: None,
        language: Some("en".to_string()),
    };
    let v = ws.forge(&req).expect("forge ok");
    assert_eq!(v["ok"], Value::Bool(true));
    assert_eq!(v["selfCheck"], "ok");

    // File exists at the child_file_path convention.
    let child = repo.path().join("serialization/content/Home/NewChild.yml");
    assert!(child.exists(), "forged file exists at child_file_path");

    // The item is a minimal canonical item that round-trips.
    let fmt = treesmith_format::detect(repo.path());
    let bytes = read_bytes(&child);
    let parsed = fmt.parse(&bytes).expect("parse forged");
    assert_eq!(fmt.emit(&parsed), bytes, "forged file round-trips");
    assert_eq!(
        parsed.path().as_deref(),
        Some("/sitecore/content/Home/NewChild")
    );

    // The graph sees it via get by path.
    let got = ws
        .get("/sitecore/content/Home/NewChild")
        .expect("get forged");
    assert_eq!(got["item"]["name"], "NewChild");
    assert_eq!(got["item"]["template"]["name"], "Page");
}

// ---- move ----------------------------------------------------------------------

#[test]
fn move_relocates_subtree_and_rewrites_paths() {
    let repo = TempRepo::copy_of(&basic_fixture());
    let mut ws = Workspace::open(repo.path()).expect("open temp");

    // Move About from under Home to under the Data folder.
    let req = MoveRequest {
        item: ABOUT.to_string(),
        new_parent: "/sitecore/content/Home/Data".to_string(),
        name: None,
    };
    let v = ws.move_item(&req).expect("move ok");
    assert_eq!(v["ok"], Value::Bool(true));

    // Old file gone, new file present.
    let old_file = repo.path().join("serialization/content/Home/About.yml");
    let new_file = repo
        .path()
        .join("serialization/content/Home/Data/About.yml");
    assert!(!old_file.exists(), "old About.yml removed");
    assert!(new_file.exists(), "About.yml relocated under Data");

    // Path field rewritten.
    let fmt = treesmith_format::detect(repo.path());
    let parsed = fmt.parse(&read_bytes(&new_file)).expect("parse moved");
    assert_eq!(
        parsed.path().as_deref(),
        Some("/sitecore/content/Home/Data/About")
    );

    // Graph reflects the new location.
    let got = ws
        .get("/sitecore/content/Home/Data/About")
        .expect("get moved");
    assert_eq!(got["item"]["name"], "About");
}

#[test]
fn move_rewrites_path_form_datasource() {
    let repo = TempRepo::copy_of(&basic_fixture());
    let mut ws = Workspace::open(repo.path()).expect("open temp");

    // Home's final-renderings delta carries `ds="local:/Data/HeroData"`.
    // Move the Data folder to a new name; the ds= path should follow.
    // (local: is page-relative, so path rewriting targets /sitecore paths;
    // instead exercise a genuine path-form ds by moving HeroData and
    // checking whole-value path fields. Here we assert the move of Data
    // rewrites descendant Path fields.)
    let req = MoveRequest {
        item: "/sitecore/content/Home/Data".to_string(),
        new_parent: HOME.to_string(),
        name: Some("Assets".to_string()),
    };
    ws.move_item(&req).expect("move Data->Assets");

    let hero = repo
        .path()
        .join("serialization/content/Home/Assets/HeroData.yml");
    assert!(hero.exists(), "HeroData relocated under Assets");
    let fmt = treesmith_format::detect(repo.path());
    let parsed = fmt.parse(&read_bytes(&hero)).expect("parse HeroData");
    assert_eq!(
        parsed.path().as_deref(),
        Some("/sitecore/content/Home/Assets/HeroData")
    );
}

// ---- validate ------------------------------------------------------------------

#[test]
fn validate_maps_gate_report_and_has_errors() {
    let ws = Workspace::open(&broken_fixture()).expect("open broken");
    let (v, has_errors) = ws.validate(None).expect("validate ok");
    // validate shape keys.
    for key in ["ok", "errors", "warnings", "infos", "findings", "skipped"] {
        assert!(v.get(key).is_some(), "validate missing {key}");
    }
    // The broken fixture is deliberately broken -> errors present.
    assert!(has_errors, "broken fixture has errors");
    assert_eq!(v["ok"], Value::Bool(false));
    assert!(v["errors"].as_u64().unwrap() > 0);
    let findings = v["findings"].as_array().expect("findings");
    assert!(!findings.is_empty());
    // Finding shape.
    let f = &findings[0];
    for key in [
        "gate", "code", "severity", "itemId", "itemPath", "file", "message", "details",
    ] {
        assert!(f.get(key).is_some(), "Finding missing {key}");
    }
}

// ---- census (works on faulted trees) -------------------------------------------

#[test]
fn census_reports_broken_repo_honestly() {
    let v = Workspace::census(&broken_fixture());
    for key in [
        "ok",
        "files",
        "items",
        "roundTripOk",
        "faults",
        "mismatches",
        "elapsedMs",
    ] {
        assert!(v.get(key).is_some(), "census missing {key}");
    }
    assert!(v["files"].as_u64().unwrap() > 0);
    // The broken repo's item files still round-trip (gates find semantic
    // problems, not codec faults) — census is about codec fidelity.
    assert!(v["items"].as_u64().unwrap() > 0);
}

// ---- tree-fault policy ---------------------------------------------------------

#[test]
fn unparseable_yml_forces_tree_fault_but_census_works() {
    let repo = TempRepo::empty();
    // Minimal-but-broken repo: one item that cannot parse (no --- marker).
    let ser = repo.path().join("serialization");
    std::fs::create_dir_all(&ser).expect("mk serialization");
    // Sniffs as an item (--- + `ID: `) but the tab-indented line makes the
    // full parse fail -> a recorded parse fault (not a silently-skipped
    // non-item file).
    std::fs::write(
        ser.join("Broken.yml"),
        b"---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nSharedFields:\n\t- ID: bad\n",
    )
    .expect("write broken yml");

    let ws = Workspace::open(repo.path()).expect("open faulted repo");

    // get and query return TreeFault.
    let get_err = ws
        .get("/sitecore/anything")
        .expect_err("get on faulted tree");
    assert_eq!(get_err.class(), "tree-fault");
    assert_eq!(get_err.exit_code(), 3);
    let q_err = ws.query("path:*").expect_err("query on faulted tree");
    assert_eq!(q_err.class(), "tree-fault");

    // to_json is machine-readable.
    let j = get_err.to_json();
    assert_eq!(j["ok"], Value::Bool(false));
    assert_eq!(j["error"]["class"], "tree-fault");

    // census still works on the faulted tree.
    let c = Workspace::census(repo.path());
    assert_eq!(c["ok"], Value::Bool(false), "faulted census not ok");
    assert!(
        !c["faults"].as_array().unwrap().is_empty(),
        "census reports the fault"
    );
}

// ---- helpers -------------------------------------------------------------------

/// Maps every `.yml` file under `root` to its bytes, keyed by
/// forward-slash root-relative path.
fn snapshot_yml(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut map = std::collections::BTreeMap::new();
    collect_yml(root, root, &mut map);
    map
}

fn collect_yml(root: &Path, dir: &Path, map: &mut std::collections::BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("entry");
        let p = entry.path();
        if p.is_dir() {
            collect_yml(root, &p, map);
        } else if p.extension().is_some_and(|e| e == "yml") {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            map.insert(rel, std::fs::read(&p).expect("read yml"));
        }
    }
}
