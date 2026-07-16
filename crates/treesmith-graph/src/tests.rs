//! Unit tests over tempdir-authored mini-trees (DESIGN.md §14: the graph
//! crate never depends on `fixtures/`).

use super::*;
use crate::query::{glob_match, parse_query};
use std::sync::atomic::{AtomicU32, Ordering};

// ---- temp-dir + tree authoring helpers -------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "treesmith-graph-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.path.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

// GUID register for the mini tree.
const ROOT: &str = "aaaaaaaa-0000-4000-8000-0000000000aa"; // unserialized parent
const PAGE_T: &str = "7c1e1c2a-0001-4000-8000-000000000001";
const HOME: &str = "c0ffee00-0001-4000-8000-000000000001";
const ABOUT: &str = "c0ffee00-0002-4000-8000-000000000002";
const ZOO: &str = "c0ffee00-0003-4000-8000-000000000003";
const DATA: &str = "c0ffee00-0004-4000-8000-000000000004";
const TITLE_F: &str = "beefbeef-0001-4000-8000-000000000001";
const BODY_F: &str = "beefbeef-0002-4000-8000-000000000002";

fn item_yaml(id: &str, parent: &str, template: &str, path: &str, extra: &str) -> String {
    format!(
        "---\nID: \"{id}\"\nParent: \"{parent}\"\nTemplate: \"{template}\"\nPath: {path}\nDB: master\n{extra}"
    )
}

fn shared_field(fid: &str, hint: &str, value: &str) -> String {
    format!("SharedFields:\n- ID: \"{fid}\"\n  Hint: {hint}\n  Value: {value}\n")
}

/// Authors the standard mini tree; returns its root TempDir.
fn mini_tree() -> TempDir {
    let dir = TempDir::new("tree");
    // Template item named "Page" (its own Template guid is arbitrary here).
    dir.write(
        "items/templates/Page.yml",
        &item_yaml(
            PAGE_T,
            ROOT,
            "ab86861a-6030-46c5-b394-e8f99e8b87db",
            "/sitecore/templates/Page",
            "",
        ),
    );
    dir.write(
        "items/content/Home.yml",
        &item_yaml(
            HOME,
            ROOT,
            PAGE_T,
            "/sitecore/content/Home",
            &format!(
                "{}Languages:\n- Language: en\n  Fields:\n  - ID: \"{BODY_F}\"\n    Hint: NavTitle\n    Value: nav\n  Versions:\n  - Version: 1\n    Fields:\n    - ID: \"{BODY_F}\"\n      Hint: Body\n      Value: body one\n  - Version: 2\n    Fields:\n    - ID: \"{BODY_F}\"\n      Hint: Body\n      Value: body two\n- Language: da\n  Versions:\n  - Version: 1\n    Fields:\n    - ID: \"{BODY_F}\"\n      Hint: Body\n      Value: krop\n",
                shared_field(TITLE_F, "Title", "hello world")
            ),
        ),
    );
    dir.write(
        "items/content/Home/About.yml",
        &item_yaml(
            ABOUT,
            HOME,
            PAGE_T,
            "/sitecore/content/Home/About",
            &shared_field(TITLE_F, "Title", "about us"),
        ),
    );
    dir.write(
        "items/content/Home/Zoo.yml",
        &item_yaml(ZOO, HOME, PAGE_T, "/sitecore/content/Home/Zoo", ""),
    );
    dir.write(
        "items/content/Home/Data.yml",
        &item_yaml(
            DATA,
            HOME,
            "a87a00b1-e6db-45ab-8b54-636fec3b5523",
            "/sitecore/content/Home/Data",
            "",
        ),
    );
    // Non-item repo files for RepoFiles.
    dir.write("src/Views/Hero.cshtml", "@Html.Sitecore()\n");
    dir.write("src/Views/NotHero.cshtml", "@x\n");
    dir.write(
        "src/Controllers/NavBarController.cs",
        "class NavBarController {}\n",
    );
    // Excluded dirs and non-item yml never enter the graph.
    dir.write("node_modules/pkg/item.yml", "---\nID: \"x\"\n");
    dir.write("config/app.yml", "settings:\n  a: 1\n");
    dir
}

// ---- build + indexes --------------------------------------------------------

#[test]
fn build_indexes_and_accessors() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());

    assert!(graph.faults().is_empty(), "faults: {:?}", graph.faults());
    assert_eq!(graph.root(), dir.path());
    assert_eq!(graph.format().key(), "rainbow");

    let home = graph.get(g(HOME)).expect("Home present");
    assert_eq!(home.meta.path, "/sitecore/content/Home");
    assert_eq!(home.meta.name, "Home");
    assert_eq!(home.meta.parent, Some(g(ROOT)));
    assert_eq!(home.meta.template, Some(g(PAGE_T)));
    assert_eq!(home.meta.db.as_deref(), Some("master"));
    assert_eq!(
        home.meta.languages,
        vec![("da".to_string(), vec![1]), ("en".to_string(), vec![1, 2]),]
    );
    assert!(home.file.ends_with(Path::new("items/content/Home.yml")));
    assert_eq!(graph.file_of(g(HOME)), Some(home.file.as_path()));
    assert_eq!(
        graph.file_of(g("11111111-1111-4111-8111-111111111111")),
        None
    );

    // by_template sorted by path.
    assert_eq!(
        graph.by_template(g(PAGE_T)),
        vec![g(HOME), g(ABOUT), g(ZOO)],
        "paths: /sitecore/content/Home < .../Home/About < .../Home/Zoo"
    );

    // ids_by_path: all items, (path, id).
    assert_eq!(
        graph.ids_by_path(),
        vec![g(HOME), g(ABOUT), g(DATA), g(ZOO), g(PAGE_T)]
    );

    // The graph never sees excluded dirs or non-item yml.
    assert!(graph
        .get(g("11111111-2222-4333-8444-555555555555"))
        .is_none());
    assert_eq!(graph.find_path("/x").len(), 0);
}

#[test]
fn children_sorted_by_name_then_id() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());
    assert_eq!(
        graph.children(g(HOME)),
        vec![g(ABOUT), g(DATA), g(ZOO)],
        "About < Data < Zoo by name"
    );
    // Children of an unserialized parent still resolve (partial tree root).
    assert_eq!(graph.children(g(ROOT)), vec![g(HOME), g(PAGE_T)]);
    // Leaf items have no children.
    assert!(graph.children(g(ABOUT)).is_empty());
}

#[test]
fn find_path_is_case_insensitive() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());
    for form in [
        "/sitecore/content/Home/About",
        "/SITECORE/CONTENT/HOME/ABOUT",
        "/sitecore/Content/home/aBoUt",
    ] {
        assert_eq!(graph.find_path(form), vec![g(ABOUT)], "form {form}");
    }
    assert!(graph.find_path("/sitecore/content/Home/Missing").is_empty());
}

// ---- faults ------------------------------------------------------------------

#[test]
fn duplicate_id_keeps_lexically_first_and_is_deterministic() {
    let dir = TempDir::new("dup");
    let yaml = item_yaml(HOME, ROOT, PAGE_T, "/sitecore/content/Home", "");
    dir.write("a/Dup.yml", &yaml);
    dir.write("b/Dup.yml", &yaml);

    let one = Graph::build(dir.path());
    let two = Graph::build(dir.path());

    assert_eq!(one.faults(), two.faults(), "bit-identical across builds");
    assert_eq!(one.faults().len(), 1);
    let fault = &one.faults()[0];
    assert_eq!(fault.kind, "duplicate-id");
    assert_eq!(fault.file, Path::new("b/Dup.yml"), "later file is faulted");
    assert!(fault.message.contains(HOME));
    assert!(fault.message.contains("a/Dup.yml"), "names the kept file");

    let node = one.get(g(HOME)).unwrap();
    assert!(
        node.file.ends_with(Path::new("a/Dup.yml")),
        "lexically-first file wins"
    );
    assert_eq!(one.ids_by_path(), vec![g(HOME)], "item indexed once");
}

#[test]
fn missing_id_and_parse_faults_are_recorded_and_sorted() {
    let dir = TempDir::new("faults");
    dir.write(
        "items/NoId.yml",
        "---\nID: \"not-a-guid\"\nPath: /sitecore/content/NoId\n",
    );
    dir.write(
        "items/Broken.yml",
        &format!("---\nID: \"{ABOUT}\"\nSharedFields:\n\t- bad\n"),
    );
    dir.write(
        "items/Ok.yml",
        &item_yaml(HOME, ROOT, PAGE_T, "/sitecore/content/Home", ""),
    );

    let graph = Graph::build(dir.path());
    let kinds: Vec<(&Path, &str)> = graph
        .faults()
        .iter()
        .map(|f| (f.file.as_path(), f.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (Path::new("items/Broken.yml"), "parse"),
            (Path::new("items/NoId.yml"), "missing-id"),
        ],
        "sorted by file"
    );
    assert!(
        graph.faults()[0].message.starts_with("line 4:"),
        "parse fault names the line: {}",
        graph.faults()[0].message
    );
    // The healthy item is still indexed alongside the faults.
    assert!(graph.get(g(HOME)).is_some());
}

#[test]
fn tree_fault_serializes_with_forward_slash_file() {
    let fault = TreeFault {
        file: PathBuf::from("a").join("b.yml"),
        kind: "parse".to_string(),
        message: "boom".to_string(),
    };
    let json = serde_json::to_value(&fault).unwrap();
    assert_eq!(json["file"], "a/b.yml");
    assert_eq!(json["kind"], "parse");
    assert_eq!(json["message"], "boom");
}

// ---- refresh -----------------------------------------------------------------

#[test]
fn refresh_after_change_delete_and_add() {
    let dir = mini_tree();
    let mut graph = Graph::build(dir.path());

    // Change: About gets a new Title value.
    let about_file = dir.write(
        "items/content/Home/About.yml",
        &item_yaml(
            ABOUT,
            HOME,
            PAGE_T,
            "/sitecore/content/Home/About",
            &shared_field(TITLE_F, "Title", "changed"),
        ),
    );
    // Delete: Zoo disappears.
    let zoo_file = dir.path().join("items/content/Home/Zoo.yml");
    std::fs::remove_file(&zoo_file).unwrap();
    // Add: a brand-new item and a new code file.
    let new_id = "c0ffee00-0005-4000-8000-000000000005";
    let new_file = dir.write(
        "items/content/Home/News.yml",
        &item_yaml(new_id, HOME, PAGE_T, "/sitecore/content/Home/News", ""),
    );
    let css = dir.write("src/site.css", "body{}\n");

    graph.refresh_paths(&[about_file, zoo_file, new_file, css]);

    let about = graph.get(g(ABOUT)).unwrap();
    let (_, field) = about.item.find_field(g(TITLE_F)).unwrap();
    assert_eq!(field.value, "changed");

    assert!(graph.get(g(ZOO)).is_none(), "deleted item dropped");
    assert!(graph.find_path("/sitecore/content/Home/Zoo").is_empty());
    assert!(graph.get(g(new_id)).is_some(), "new item picked up");
    assert_eq!(
        graph.children(g(HOME)),
        vec![g(ABOUT), g(DATA), g(new_id)],
        "children re-sorted after refresh"
    );
    assert!(!graph
        .repo_files()
        .all
        .contains(&"items/content/Home/Zoo.yml".to_string()));
    assert!(graph.repo_files().all.contains(&"src/site.css".to_string()));
}

#[test]
fn refresh_relative_paths_and_rebuild_match_disk() {
    let dir = mini_tree();
    let mut graph = Graph::build(dir.path());
    std::fs::remove_file(dir.path().join("items/content/Home/Zoo.yml")).unwrap();
    graph.refresh_paths(&[PathBuf::from("items/content/Home/Zoo.yml")]);
    assert!(graph.get(g(ZOO)).is_none(), "relative path resolves");

    // rebuild reproduces the refreshed state from disk alone (I1).
    let fresh = Graph::build(dir.path());
    assert_eq!(fresh.ids_by_path(), graph.ids_by_path());
    assert_eq!(fresh.repo_files().all, graph.repo_files().all);
    assert_eq!(fresh.faults(), graph.faults());
}

#[test]
fn refresh_promotes_duplicate_loser_after_winner_deleted() {
    let dir = TempDir::new("dup-promote");
    let yaml = item_yaml(HOME, ROOT, PAGE_T, "/sitecore/content/Home", "");
    let a = dir.write("a/Dup.yml", &yaml);
    dir.write("b/Dup.yml", &yaml);

    let mut graph = Graph::build(dir.path());
    assert_eq!(graph.faults().len(), 1);

    std::fs::remove_file(&a).unwrap();
    graph.refresh_paths(&[a]);
    assert!(graph.faults().is_empty(), "duplicate fault cleared");
    let node = graph.get(g(HOME)).unwrap();
    assert!(
        node.file.ends_with(Path::new("b/Dup.yml")),
        "loser promoted"
    );
}

#[test]
fn refresh_deleted_directory_drops_subtree() {
    let dir = mini_tree();
    let mut graph = Graph::build(dir.path());
    let sub = dir.path().join("items/content/Home");
    std::fs::remove_dir_all(&sub).unwrap();
    graph.refresh_paths(&[sub]);
    assert!(graph.get(g(ABOUT)).is_none());
    assert!(graph.get(g(ZOO)).is_none());
    assert!(graph.get(g(HOME)).is_some(), "Home.yml itself remains");
    assert!(graph.children(g(HOME)).is_empty());
}

// ---- RepoFiles -----------------------------------------------------------------

#[test]
fn repo_files_lists_everything_sorted_outside_excluded_dirs() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());
    let all = &graph.repo_files().all;
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(*all, sorted, "sorted");
    assert!(all.contains(&"src/Views/Hero.cshtml".to_string()));
    assert!(
        all.contains(&"config/app.yml".to_string()),
        "non-item yml listed"
    );
    assert!(
        !all.iter().any(|f| f.starts_with("node_modules/")),
        "excluded dirs never scanned"
    );
}

#[test]
fn find_suffix_backslash_leading_slash_and_case_variance() {
    let files = RepoFiles {
        all: vec![
            "src/Controllers/NavBarController.cs".to_string(),
            "src/Views/Hero.cshtml".to_string(),
            "src/Views/NotHero.cshtml".to_string(),
        ],
    };
    for form in [
        "/Views/Hero.cshtml",
        "\\Views\\Hero.cshtml",
        "~/Views/Hero.cshtml",
        "Views/Hero.cshtml",
        "/VIEWS/HERO.CSHTML",
        "~\\views\\hero.CSHTML",
    ] {
        assert_eq!(
            files.find_suffix(form),
            vec!["src/Views/Hero.cshtml"],
            "form {form}"
        );
    }
    // Segment boundary: Hero.cshtml must not match NotHero.cshtml…
    assert_eq!(
        files.find_suffix("Hero.cshtml"),
        vec!["src/Views/Hero.cshtml"]
    );
    // …but NotHero matches itself, including as a full-length match.
    assert_eq!(
        files.find_suffix("src/Views/NotHero.cshtml"),
        vec!["src/Views/NotHero.cshtml"]
    );
    assert!(files.find_suffix("/Views/Missing.cshtml").is_empty());
    assert!(files.find_suffix("").is_empty());
    assert!(files.find_suffix("/").is_empty());
}

#[test]
fn with_extension_case_insensitive_dot_optional() {
    let files = RepoFiles {
        all: vec![
            "a/One.cshtml".to_string(),
            "b/Two.CSHTML".to_string(),
            "c/Three.cs".to_string(),
            "d/notanextension_cshtml".to_string(),
        ],
    };
    assert_eq!(
        files.with_extension("cshtml"),
        vec!["a/One.cshtml", "b/Two.CSHTML"]
    );
    assert_eq!(
        files.with_extension(".CSHTML"),
        vec!["a/One.cshtml", "b/Two.CSHTML"]
    );
    assert_eq!(files.with_extension("cs"), vec!["c/Three.cs"]);
    assert!(files.with_extension("").is_empty());
}

// ---- query: parsing -------------------------------------------------------------

#[test]
fn parse_query_terms_including_quoted_values() {
    assert!(parse_query("")
        .unwrap()
        .run(&Graph::build(TempDir::new("empty").path()))
        .is_empty());
    parse_query("path:/sitecore/** name:Home").unwrap();
    parse_query(&format!("template:{PAGE_T}")).unwrap();
    parse_query("template:{AB86861A-6030-46C5-B394-E8F99E8B87DB}").unwrap();
    parse_query("template:Page").unwrap();
    parse_query("field:Title").unwrap();
    parse_query("field:Title=hello").unwrap();
    parse_query("field:Title=\"hello world\"").unwrap();
    parse_query("name:\"My Page\"").unwrap();
    parse_query("field:\"Nav Title=two words\"").unwrap();
}

#[test]
fn parse_query_bad_terms_are_err() {
    assert!(parse_query("bare").is_err(), "bare term");
    assert!(parse_query("path:/a bare2").is_err(), "bare among good");
    assert!(parse_query("size:big").is_err(), "unknown key");
    assert!(parse_query("path:").is_err(), "empty value");
    assert!(parse_query("field:=v").is_err(), "empty field name");
    assert!(parse_query("name:\"unclosed").is_err(), "unclosed quote");
}

// ---- query: glob semantics --------------------------------------------------------

#[test]
fn glob_star_stays_within_segment() {
    assert!(glob_match("*", "abc"));
    assert!(!glob_match("*", "a/b"));
    assert!(glob_match("/a/*/c", "/a/b/c"));
    assert!(!glob_match("/a/*", "/a/b/c"));
    assert!(glob_match("/a/*", "/a/bc"));
    assert!(glob_match("ab*", "ab"));
}

#[test]
fn glob_double_star_crosses_segments() {
    assert!(glob_match("**", "a/b/c"));
    assert!(glob_match("/a/**", "/a/b/c"));
    assert!(glob_match("/a/**/d", "/a/b/c/d"));
    assert!(glob_match("**/Home", "/sitecore/content/Home"));
    assert!(!glob_match("/a/**/x", "/a/b/c/d"));
}

#[test]
fn glob_question_mark_one_non_slash_char() {
    assert!(glob_match("a?c", "abc"));
    assert!(!glob_match("a?c", "ac"));
    assert!(!glob_match("a?c", "a/c"));
    assert!(!glob_match("a?c", "abbc"));
}

#[test]
fn glob_is_case_insensitive_and_literal_otherwise() {
    assert!(glob_match("HOME", "home"));
    assert!(glob_match("/Sitecore/Content/*", "/sitecore/content/x"));
    assert!(!glob_match("abc", "abd"));
    assert!(glob_match("", ""));
    assert!(!glob_match("", "a"));
}

// ---- query: matching over the graph ------------------------------------------------

#[test]
fn query_matches_and_orders_by_path_then_id() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());

    let q = parse_query("path:/sitecore/content/**").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME), g(ABOUT), g(DATA), g(ZOO)]);

    let q = parse_query("name:About").unwrap();
    assert_eq!(q.run(&graph), vec![g(ABOUT)]);

    let q = parse_query("name:*o*").unwrap();
    assert_eq!(
        q.run(&graph),
        vec![g(HOME), g(ABOUT), g(ZOO)],
        "Home, About, Zoo all contain an o"
    );

    // template by GUID (any form) and by exact name, case-insensitive.
    let q = parse_query(&format!("template:{PAGE_T}")).unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME), g(ABOUT), g(ZOO)]);
    let q = parse_query("template:page").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME), g(ABOUT), g(ZOO)]);
    let q = parse_query("template:Nothing").unwrap();
    assert!(q.run(&graph).is_empty());

    // field existence: hint (case-insensitive) or GUID.
    let q = parse_query("field:title").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME), g(ABOUT)]);
    let q = parse_query(&format!("field:{TITLE_F}")).unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME), g(ABOUT)]);

    // field equality: exact value, any slot (shared + versioned here).
    let q = parse_query("field:Title=\"hello world\"").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME)]);
    let q = parse_query("field:Body=\"body two\"").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME)], "versioned slot matched");
    let q = parse_query("field:Body=krop").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME)], "second language matched");
    let q = parse_query("field:NavTitle=nav").unwrap();
    assert_eq!(q.run(&graph), vec![g(HOME)], "unversioned slot matched");
    let q = parse_query("field:Title=\"HELLO WORLD\"").unwrap();
    assert!(q.run(&graph).is_empty(), "values compare exactly");

    // AND semantics across terms.
    let q = parse_query("path:/sitecore/content/** field:Title=\"about us\"").unwrap();
    assert_eq!(q.run(&graph), vec![g(ABOUT)]);
    let q = parse_query("name:About template:page field:Title").unwrap();
    assert_eq!(q.run(&graph), vec![g(ABOUT)]);
    let q = parse_query("name:About field:Body").unwrap();
    assert!(q.run(&graph).is_empty());
}

#[test]
fn query_run_is_deterministic() {
    let dir = mini_tree();
    let graph = Graph::build(dir.path());
    let q = parse_query("path:**").unwrap();
    assert_eq!(q.run(&graph), q.run(&graph));
    assert_eq!(q.run(&graph), graph.ids_by_path());
}
