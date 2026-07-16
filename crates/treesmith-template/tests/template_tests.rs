//! Template-resolution tests: `fixtures/rainbow/basic` (read-only) plus
//! synthetic tempdir trees for cycles, unresolved bases, and
//! field-definition value precedence (DESIGN.md §5, §14).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use treesmith_graph::Graph;
use treesmith_template::TemplateIndex;
use treesmith_types::{Guid, SectionKind};

fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

// ---- fixture GUID register (DESIGN.md §13) ----------------------------------

const PAGE_T: &str = "7c1e1c2a-0001-4000-8000-000000000001";
const TITLE_F: &str = "7c1e1c2a-0003-4000-8000-000000000003";
const BODY_F: &str = "7c1e1c2a-0004-4000-8000-000000000004";
const NAVTITLE_F: &str = "7c1e1c2a-0005-4000-8000-000000000005";
const RELATED_F: &str = "7c1e1c2a-0006-4000-8000-000000000006";
const META_T: &str = "7c1e1c2a-0010-4000-8000-000000000010";
const KEYWORDS_F: &str = "7c1e1c2a-0012-4000-8000-000000000012";
const ARTICLE_T: &str = "7c1e1c2a-0020-4000-8000-000000000020";
const PAGE_STD_VALUES: &str = "7c1e1c2a-0030-4000-8000-000000000030";

fn fixture_index() -> TemplateIndex {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("rainbow")
        .join("basic");
    let graph = Graph::build(&root);
    assert!(
        graph.faults().is_empty(),
        "fixture tree must be fault-free: {:?}",
        graph.faults()
    );
    TemplateIndex::build(&graph)
}

// ---- fixtures/rainbow/basic --------------------------------------------------

#[test]
fn article_page_resolves_page_then_meta_chain() {
    let index = fixture_index();
    let eff = index.resolve(g(ARTICLE_T)).expect("ArticlePage resolves");
    assert_eq!(eff.id, g(ARTICLE_T));
    assert_eq!(eff.name, "ArticlePage");
    assert_eq!(eff.chain, vec![g(ARTICLE_T), g(PAGE_T), g(META_T)]);
    assert!(eff.unresolved_bases.is_empty());
}

#[test]
fn article_page_effective_fields_have_correct_sections_and_types() {
    let index = fixture_index();
    let eff = index.resolve(g(ARTICLE_T)).unwrap();

    let title = eff.field_by_id(g(TITLE_F)).expect("Title");
    assert_eq!(title.name, "Title");
    assert_eq!(title.field_type, "Single-Line Text");
    assert_eq!(title.section, SectionKind::Versioned);
    assert_eq!(title.section_name, "Content");
    assert_eq!(title.defined_by, g(PAGE_T));

    let body = eff.field_by_id(g(BODY_F)).expect("Body");
    assert_eq!(body.name, "Body");
    assert_eq!(body.field_type, "Rich Text");
    assert_eq!(body.section, SectionKind::Versioned);
    assert_eq!(body.defined_by, g(PAGE_T));

    let nav = eff.field_by_id(g(NAVTITLE_F)).expect("NavTitle");
    assert_eq!(nav.name, "NavTitle");
    assert_eq!(nav.section, SectionKind::Unversioned);
    assert_eq!(nav.defined_by, g(PAGE_T));

    let related = eff.field_by_id(g(RELATED_F)).expect("RelatedPages");
    assert_eq!(related.name, "RelatedPages");
    assert_eq!(related.field_type, "Treelist");
    assert_eq!(related.section, SectionKind::Shared);
    assert_eq!(related.defined_by, g(PAGE_T));

    let keywords = eff.field_by_id(g(KEYWORDS_F)).expect("Keywords");
    assert_eq!(keywords.name, "Keywords");
    assert_eq!(keywords.field_type, "Single-Line Text");
    assert_eq!(keywords.section, SectionKind::Shared);
    assert_eq!(keywords.section_name, "SEO");
    assert_eq!(keywords.defined_by, g(META_T));

    assert_eq!(eff.fields.len(), 5, "Page's 4 fields + Meta's Keywords");
}

#[test]
fn field_by_name_is_case_insensitive_and_matches_field_by_id() {
    let index = fixture_index();
    let eff = index.resolve(g(ARTICLE_T)).unwrap();
    for (name, id) in [
        ("Title", TITLE_F),
        ("title", TITLE_F),
        ("NAVTITLE", NAVTITLE_F),
        ("relatedpages", RELATED_F),
        ("Keywords", KEYWORDS_F),
        ("kEyWoRdS", KEYWORDS_F),
    ] {
        let f = eff
            .field_by_name(name)
            .unwrap_or_else(|| panic!("field_by_name({name})"));
        assert_eq!(f.id, g(id), "field_by_name({name})");
        assert_eq!(eff.field_by_id(g(id)).unwrap(), f);
    }
    assert!(eff.field_by_name("NoSuchField").is_none());
    assert!(eff
        .field_by_id(g("dddddddd-dddd-4ddd-8ddd-dddddddddddd"))
        .is_none());
}

#[test]
fn find_by_name_is_case_insensitive() {
    let index = fixture_index();
    assert_eq!(index.find_by_name("ArticlePage"), vec![g(ARTICLE_T)]);
    assert_eq!(index.find_by_name("articlepage"), vec![g(ARTICLE_T)]);
    assert_eq!(index.find_by_name("PAGE"), vec![g(PAGE_T)]);
    assert!(index.find_by_name("NoSuchTemplate").is_empty());
}

#[test]
fn std_values_chain_surfaces_page_standard_values() {
    let index = fixture_index();
    // ArticlePage has no std values of its own; Page's comes through the chain.
    assert_eq!(
        index.std_values_chain(g(ARTICLE_T)),
        vec![g(PAGE_STD_VALUES)]
    );
    assert_eq!(index.std_values_chain(g(PAGE_T)), vec![g(PAGE_STD_VALUES)]);
    assert!(index.std_values_chain(g(META_T)).is_empty());
    // Unknown template -> empty.
    assert!(index
        .std_values_chain(g("dddddddd-dddd-4ddd-8ddd-dddddddddddd"))
        .is_empty());
}

#[test]
fn template_defs_extracted_from_fixture() {
    let index = fixture_index();
    let page = index.get(g(PAGE_T)).expect("Page def");
    assert_eq!(page.name, "Page");
    assert_eq!(page.path, "/sitecore/templates/Sample/Page");
    assert!(page.bases.is_empty());
    assert_eq!(page.standard_values, Some(g(PAGE_STD_VALUES)));
    assert_eq!(page.fields.len(), 4);

    let article = index.get(g(ARTICLE_T)).expect("ArticlePage def");
    assert_eq!(article.bases, vec![g(PAGE_T), g(META_T)], "listed order");
    assert!(article.fields.is_empty());
    assert_eq!(article.standard_values, None);

    // The std-values item itself is not a template.
    assert!(index.get(g(PAGE_STD_VALUES)).is_none());
    assert!(index
        .resolve(g("dddddddd-dddd-4ddd-8ddd-dddddddddddd"))
        .is_none());
}

#[test]
fn resolution_is_deterministic_across_builds() {
    let a = fixture_index().resolve(g(ARTICLE_T)).unwrap();
    let b = fixture_index().resolve(g(ARTICLE_T)).unwrap();
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

// ---- synthetic tempdir trees ---------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "treesmith-template-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.path.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const TEMPLATE_TEMPLATE: &str = "ab86861a-6030-46c5-b394-e8f99e8b87db";
const TEMPLATE_SECTION: &str = "e269fbb5-3750-427a-9149-7aa950b49301";
const TEMPLATE_FIELD: &str = "455a3e98-a627-4b40-8035-e683a0331ac7";
const BASE_TEMPLATE_FIELD: &str = "12c33f3f-86c5-43a5-aeb4-5598cec45116";

const ROOT: &str = "aaaaaaaa-0000-4000-8000-0000000000aa";
const T_A: &str = "1a000000-0000-4000-8000-000000000001";
const T_B: &str = "1b000000-0000-4000-8000-000000000002";
const T_C: &str = "1c000000-0000-4000-8000-000000000003";
const T_D: &str = "1d000000-0000-4000-8000-000000000004";
const MISSING_X: &str = "ee000000-0000-4000-8000-0000000000ee";
const MISSING_Y: &str = "ee000000-0000-4000-8000-0000000000ef";
const SHARED_FIELD_ID: &str = "2f000000-0000-4000-8000-00000000002f";

fn item_yaml(id: &str, parent: &str, template: &str, path: &str, extra: &str) -> String {
    format!(
        "---\nID: \"{id}\"\nParent: \"{parent}\"\nTemplate: \"{template}\"\nPath: {path}\nDB: master\n{extra}"
    )
}

/// A template item named after the last path segment, with an optional
/// `__Base template` value (`|`-separated raw guids).
fn template_yaml(id: &str, name: &str, bases: &[&str]) -> String {
    let extra = if bases.is_empty() {
        String::new()
    } else {
        format!(
            "SharedFields:\n- ID: \"{BASE_TEMPLATE_FIELD}\"\n  Hint: __Base template\n  Value: {}\n",
            bases.join("|")
        )
    };
    item_yaml(
        id,
        ROOT,
        TEMPLATE_TEMPLATE,
        &format!("/sitecore/templates/{name}"),
        &extra,
    )
}

fn build_index(dir: &TempDir) -> TemplateIndex {
    let graph = Graph::build(&dir.path);
    assert!(graph.faults().is_empty(), "faults: {:?}", graph.faults());
    TemplateIndex::build(&graph)
}

#[test]
fn cycle_between_bases_is_guarded() {
    let dir = TempDir::new("cycle");
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[T_B]));
    dir.write("t/B.yml", &template_yaml(T_B, "B", &[T_A]));
    let index = build_index(&dir);

    let a = index.resolve(g(T_A)).unwrap();
    assert_eq!(
        a.chain,
        vec![g(T_A), g(T_B)],
        "cycle terminates, keep-first"
    );
    assert!(a.unresolved_bases.is_empty());

    let b = index.resolve(g(T_B)).unwrap();
    assert_eq!(b.chain, vec![g(T_B), g(T_A)]);
}

#[test]
fn self_cycle_is_guarded() {
    let dir = TempDir::new("selfcycle");
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[T_A]));
    let index = build_index(&dir);
    let a = index.resolve(g(T_A)).unwrap();
    assert_eq!(a.chain, vec![g(T_A)]);
    assert!(a.unresolved_bases.is_empty());
}

#[test]
fn unresolved_bases_are_collected_keep_first() {
    let dir = TempDir::new("unresolved");
    // A -> [X (unknown), B, Y (unknown)]; B -> [X (again), Y? no: just X].
    dir.write(
        "t/A.yml",
        &template_yaml(T_A, "A", &[MISSING_X, T_B, MISSING_Y]),
    );
    dir.write("t/B.yml", &template_yaml(T_B, "B", &[MISSING_X]));
    let index = build_index(&dir);

    let a = index.resolve(g(T_A)).unwrap();
    assert_eq!(
        a.chain,
        vec![g(T_A), g(T_B)],
        "unknown bases never enter the chain"
    );
    assert_eq!(
        a.unresolved_bases,
        vec![g(MISSING_X), g(MISSING_Y)],
        "discovery order, deduped keep-first (X seen once despite two references)"
    );
}

#[test]
fn diamond_inheritance_dedups_keep_first_and_dfs_self_first() {
    let dir = TempDir::new("diamond");
    // A -> [B, C]; B -> [D]; C -> [D]. DFS self-first: A, B, D, C.
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[T_B, T_C]));
    dir.write("t/B.yml", &template_yaml(T_B, "B", &[T_D]));
    dir.write("t/C.yml", &template_yaml(T_C, "C", &[T_D]));
    dir.write("t/D.yml", &template_yaml(T_D, "D", &[]));
    let index = build_index(&dir);

    let a = index.resolve(g(T_A)).unwrap();
    assert_eq!(a.chain, vec![g(T_A), g(T_B), g(T_D), g(T_C)]);
}

#[test]
fn first_field_definition_in_chain_order_wins() {
    let dir = TempDir::new("firstwins");
    // B and C both define the same field GUID with different types; A -> [B, C].
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[T_B, T_C]));
    dir.write("t/B.yml", &template_yaml(T_B, "B", &[]));
    dir.write("t/C.yml", &template_yaml(T_C, "C", &[]));
    for (t, tname, ftype) in [(T_B, "B", "Single-Line Text"), (T_C, "C", "Rich Text")] {
        let section_id = format!(
            "3a000000-0000-4000-8000-0000000000{}",
            if t == T_B { "3b" } else { "3c" }
        );
        dir.write(
            &format!("t/{tname}/Data.yml"),
            &item_yaml(
                &section_id,
                t,
                TEMPLATE_SECTION,
                &format!("/sitecore/templates/{tname}/Data"),
                "",
            ),
        );
        dir.write(
            &format!("t/{tname}/Data/Field.yml"),
            &item_yaml(
                SHARED_FIELD_ID,
                &section_id,
                TEMPLATE_FIELD,
                &format!("/sitecore/templates/{tname}/Data/Field"),
                &format!(
                    "SharedFields:\n- ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n  Hint: Type\n  Value: {ftype}\n"
                ),
            ),
        );
    }
    // Both field-definition items share one GUID — the graph keeps the
    // lexically-first file and records duplicate-id, which would fault the
    // build_index helper; give C's field its own GUID instead.
    let c_field = "2f000000-0000-4000-8000-000000000030";
    dir.write(
        "t/C/Data/Field.yml",
        &item_yaml(
            c_field,
            "3a000000-0000-4000-8000-00000000003c",
            TEMPLATE_FIELD,
            "/sitecore/templates/C/Data/Field",
            "SharedFields:\n- ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n  Hint: Type\n  Value: Rich Text\n",
        ),
    );
    let index = build_index(&dir);
    let a = index.resolve(g(T_A)).unwrap();

    // Same *name* on both branches: chain order (B before C) wins by name.
    let by_name = a.field_by_name("Field").unwrap();
    assert_eq!(by_name.defined_by, g(T_B));
    assert_eq!(by_name.field_type, "Single-Line Text");
    assert_eq!(by_name.id, g(SHARED_FIELD_ID));

    // Both definitions are present by id (distinct field GUIDs).
    assert_eq!(a.field_by_id(g(c_field)).unwrap().defined_by, g(T_C));
    assert_eq!(a.fields.len(), 2);
}

#[test]
fn definition_value_precedence_shared_then_unversioned_then_highest_version() {
    let dir = TempDir::new("precedence");
    let section_id = "3a000000-0000-4000-8000-00000000003a";
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[]));
    dir.write(
        "t/A/Data.yml",
        &item_yaml(
            section_id,
            T_A,
            TEMPLATE_SECTION,
            "/sitecore/templates/A/Data",
            "",
        ),
    );
    // Field definition whose `Type` value appears only in language versions:
    // da v1 = "DaOld", da v2 = "DaNew", en v1 = "EnOnly".
    // Precedence: languages alphabetical, highest version → da v2 wins.
    dir.write(
        "t/A/Data/F.yml",
        &item_yaml(
            SHARED_FIELD_ID,
            section_id,
            TEMPLATE_FIELD,
            "/sitecore/templates/A/Data/F",
            concat!(
                "Languages:\n",
                "- Language: da\n",
                "  Versions:\n",
                "  - Version: 1\n",
                "    Fields:\n",
                "    - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "      Hint: Type\n",
                "      Value: DaOld\n",
                "  - Version: 2\n",
                "    Fields:\n",
                "    - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "      Hint: Type\n",
                "      Value: DaNew\n",
                "- Language: en\n",
                "  Versions:\n",
                "  - Version: 1\n",
                "    Fields:\n",
                "    - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "      Hint: Type\n",
                "      Value: EnOnly\n",
            ),
        ),
    );
    let index = build_index(&dir);
    let a = index.resolve(g(T_A)).unwrap();
    assert_eq!(
        a.field_by_id(g(SHARED_FIELD_ID)).unwrap().field_type,
        "DaNew",
        "first language alphabetically, highest version"
    );

    // Now add an unversioned value in `en`: unversioned (any language)
    // outranks every versioned value.
    let dir2 = TempDir::new("precedence2");
    dir2.write("t/A.yml", &template_yaml(T_A, "A", &[]));
    dir2.write(
        "t/A/Data.yml",
        &item_yaml(
            section_id,
            T_A,
            TEMPLATE_SECTION,
            "/sitecore/templates/A/Data",
            "",
        ),
    );
    dir2.write(
        "t/A/Data/F.yml",
        &item_yaml(
            SHARED_FIELD_ID,
            section_id,
            TEMPLATE_FIELD,
            "/sitecore/templates/A/Data/F",
            concat!(
                "Languages:\n",
                "- Language: da\n",
                "  Versions:\n",
                "  - Version: 1\n",
                "    Fields:\n",
                "    - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "      Hint: Type\n",
                "      Value: Versioned\n",
                "- Language: en\n",
                "  Fields:\n",
                "  - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "    Hint: Type\n",
                "    Value: UnversionedEn\n",
            ),
        ),
    );
    let index2 = build_index(&dir2);
    let a2 = index2.resolve(g(T_A)).unwrap();
    assert_eq!(
        a2.field_by_id(g(SHARED_FIELD_ID)).unwrap().field_type,
        "UnversionedEn"
    );

    // And a shared value outranks everything.
    let dir3 = TempDir::new("precedence3");
    dir3.write("t/A.yml", &template_yaml(T_A, "A", &[]));
    dir3.write(
        "t/A/Data.yml",
        &item_yaml(
            section_id,
            T_A,
            TEMPLATE_SECTION,
            "/sitecore/templates/A/Data",
            "",
        ),
    );
    dir3.write(
        "t/A/Data/F.yml",
        &item_yaml(
            SHARED_FIELD_ID,
            section_id,
            TEMPLATE_FIELD,
            "/sitecore/templates/A/Data/F",
            concat!(
                "SharedFields:\n",
                "- ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "  Hint: Type\n",
                "  Value: SharedWins\n",
                "Languages:\n",
                "- Language: en\n",
                "  Fields:\n",
                "  - ID: \"ab162cc0-dc80-4abf-8871-998ee5d7ba32\"\n",
                "    Hint: Type\n",
                "    Value: UnversionedEn\n",
            ),
        ),
    );
    let index3 = build_index(&dir3);
    let a3 = index3.resolve(g(T_A)).unwrap();
    assert_eq!(
        a3.field_by_id(g(SHARED_FIELD_ID)).unwrap().field_type,
        "SharedWins"
    );
}

#[test]
fn standard_values_falls_back_to_child_named_standard_values() {
    let dir = TempDir::new("stdvalues");
    let std_id = "5d000000-0000-4000-8000-00000000005d";
    // Template with no STANDARD_VALUES field but a child named __Standard Values.
    dir.write("t/A.yml", &template_yaml(T_A, "A", &[]));
    dir.write(
        "t/A/__Standard Values.yml",
        &item_yaml(
            std_id,
            T_A,
            T_A,
            "/sitecore/templates/A/__Standard Values",
            "",
        ),
    );
    let index = build_index(&dir);
    assert_eq!(index.get(g(T_A)).unwrap().standard_values, Some(g(std_id)));
    assert_eq!(index.std_values_chain(g(T_A)), vec![g(std_id)]);
}
