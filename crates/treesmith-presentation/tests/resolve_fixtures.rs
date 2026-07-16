//! Full-stack resolution against `fixtures/rainbow/basic` (DESIGN.md §14).

use std::path::PathBuf;

use treesmith_graph::Graph;
use treesmith_presentation::{
    resolve, scan_placeholders, DatasourceResolution, ResolvedPresentation,
};
use treesmith_template::TemplateIndex;
use treesmith_types::Guid;

const HOME: &str = "c0ffee00-0001-4000-8000-000000000001";
const HERO_DATA: &str = "c0ffee00-0002-4000-8000-000000000002";
const ABOUT: &str = "c0ffee00-0003-4000-8000-000000000003";
const MAIN_LAYOUT: &str = "9a11aaaa-0001-4000-8000-000000000001";
const DEFAULT_DEVICE: &str = "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3";

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rainbow/basic")
}

fn build() -> (Graph, TemplateIndex) {
    let graph = Graph::build(&fixture_root());
    assert!(
        graph.faults().is_empty(),
        "basic fixture must be clean: {:?}",
        graph.faults()
    );
    let templates = TemplateIndex::build(&graph);
    (graph, templates)
}

fn guid(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}

fn resolve_home_en_v1(graph: &Graph, templates: &TemplateIndex) -> ResolvedPresentation {
    resolve(graph, templates, guid(HOME), Some("en"), Some(1)).unwrap()
}

#[test]
fn home_en_v1_merges_final_delta_in_order() {
    let (graph, templates) = build();
    let resolved = resolve_home_en_v1(&graph, &templates);

    assert_eq!(resolved.item_id, guid(HOME));
    assert_eq!(resolved.item_path, "/sitecore/content/Home");
    assert_eq!(resolved.language, "en");
    assert_eq!(resolved.version, 1);

    assert_eq!(resolved.devices.len(), 1);
    let device = &resolved.devices[0];
    assert_eq!(device.device_id, DEFAULT_DEVICE);
    assert!(device.notes.is_empty(), "clean fixture: {:?}", device.notes);

    // NavBar + Hero + PromoBanner, in order (PromoBanner p:after Hero).
    let names: Vec<Option<&str>> = device
        .renderings
        .iter()
        .map(|r| r.rendering_name.as_deref())
        .collect();
    assert_eq!(
        names,
        vec![Some("NavBar"), Some("Hero"), Some("PromoBanner")]
    );
}

#[test]
fn home_en_v1_hero_datasource_resolves_local_path() {
    let (graph, templates) = build();
    let resolved = resolve_home_en_v1(&graph, &templates);
    let device = &resolved.devices[0];

    let hero = &device.renderings[1];
    assert_eq!(hero.rendering_name.as_deref(), Some("Hero"));
    assert_eq!(
        hero.datasource,
        DatasourceResolution::Item {
            raw: "local:/Data/HeroData".to_string(),
            id: guid(HERO_DATA),
            path: "/sitecore/content/Home/Data/HeroData".to_string(),
        },
        "final delta swapped the shared GUID datasource to local:"
    );
    assert_eq!(hero.code_files, vec!["src/Views/Hero.cshtml".to_string()]);
    assert_eq!(hero.placeholder, "main");
    assert_eq!(hero.placeholder_leaf, "main");
}

#[test]
fn home_en_v1_layout_and_code_files() {
    let (graph, templates) = build();
    let resolved = resolve_home_en_v1(&graph, &templates);
    let device = &resolved.devices[0];

    let layout = device.layout.as_ref().expect("device has a layout");
    assert_eq!(layout.id, MAIN_LAYOUT);
    assert_eq!(
        layout.path.as_deref(),
        Some("/sitecore/layout/Layouts/MainLayout")
    );
    assert_eq!(
        device.layout_code_files,
        vec!["src/Views/Shared/MainLayout.cshtml".to_string()]
    );

    let navbar = &device.renderings[0];
    assert_eq!(
        navbar.code_files,
        vec!["src/Controllers/NavBarController.cs".to_string()],
        "controller rendering resolves through class NavBarController"
    );
    assert_eq!(navbar.datasource, DatasourceResolution::ContextItem);

    let promo = &device.renderings[2];
    assert_eq!(
        promo.code_files,
        vec!["src/Views/PromoBanner.cshtml".to_string()]
    );
}

#[test]
fn home_en_v1_sources_mark_shared_and_final() {
    let (graph, templates) = build();
    let resolved = resolve_home_en_v1(&graph, &templates);
    let sources: Vec<&str> = resolved.devices[0]
        .renderings
        .iter()
        .map(|r| r.source.as_str())
        .collect();
    // NavBar and Hero originate in the shared standard-values layout
    // (Hero's ds overlay does not change its origin); PromoBanner was
    // inserted by the final-renderings delta.
    assert_eq!(sources, vec!["shared", "shared", "final"]);
}

#[test]
fn home_defaults_pick_first_language_with_versions() {
    let (graph, templates) = build();
    // Home has da (v1) and en (v1, v2): alphabetically first language with
    // versions is `da`; its max version is 1.
    let resolved = resolve(&graph, &templates, guid(HOME), None, None).unwrap();
    assert_eq!(resolved.language, "da");
    assert_eq!(resolved.version, 1);

    // en default version = max existing (2). The final delta sits on v1
    // only, so v2 shows the shared layout untouched.
    let resolved = resolve(&graph, &templates, guid(HOME), Some("en"), None).unwrap();
    assert_eq!(resolved.version, 2);
    let names: Vec<Option<&str>> = resolved.devices[0]
        .renderings
        .iter()
        .map(|r| r.rendering_name.as_deref())
        .collect();
    assert_eq!(names, vec![Some("NavBar"), Some("Hero")]);
    assert!(matches!(
        resolved.devices[0].renderings[1].datasource,
        DatasourceResolution::Item { ref raw, .. } if raw.contains("C0FFEE00-0002")
    ));
}

#[test]
fn about_resolves_std_values_only() {
    let (graph, templates) = build();
    // About has no layout of its own: everything comes from Page's
    // standard values.
    let resolved = resolve(&graph, &templates, guid(ABOUT), None, None).unwrap();
    assert_eq!(resolved.language, "en");
    assert_eq!(resolved.version, 1);
    assert_eq!(resolved.item_path, "/sitecore/content/Home/About");

    assert_eq!(resolved.devices.len(), 1);
    let device = &resolved.devices[0];
    assert_eq!(device.device_id, DEFAULT_DEVICE);
    assert!(device.notes.is_empty());

    let layout = device.layout.as_ref().expect("layout from std values");
    assert_eq!(layout.id, MAIN_LAYOUT);
    assert_eq!(
        device.layout_code_files,
        vec!["src/Views/Shared/MainLayout.cshtml".to_string()]
    );

    let names: Vec<Option<&str>> = device
        .renderings
        .iter()
        .map(|r| r.rendering_name.as_deref())
        .collect();
    assert_eq!(names, vec![Some("NavBar"), Some("Hero")]);
    assert!(
        device.renderings.iter().all(|r| r.source == "shared"),
        "no final layer touched About"
    );

    // Hero's shared datasource is the HeroData GUID.
    assert_eq!(
        device.renderings[1].datasource,
        DatasourceResolution::Item {
            raw: "{C0FFEE00-0002-4000-8000-000000000002}".to_string(),
            id: guid(HERO_DATA),
            path: "/sitecore/content/Home/Data/HeroData".to_string(),
        }
    );
}

#[test]
fn unknown_item_is_an_error() {
    let (graph, templates) = build();
    let missing = guid("99999999-9999-4999-8999-999999999999");
    let err = resolve(&graph, &templates, missing, None, None).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[test]
fn resolved_presentation_serializes_camel_case() {
    let (graph, templates) = build();
    let resolved = resolve_home_en_v1(&graph, &templates);
    let json = serde_json::to_value(&resolved).unwrap();

    assert_eq!(json["itemId"], HOME);
    assert_eq!(json["itemPath"], "/sitecore/content/Home");
    assert_eq!(json["language"], "en");
    assert_eq!(json["version"], 1);

    let device = &json["devices"][0];
    assert_eq!(device["deviceId"], DEFAULT_DEVICE);
    assert_eq!(device["layout"]["id"], MAIN_LAYOUT);
    assert_eq!(
        device["layout"]["path"],
        "/sitecore/layout/Layouts/MainLayout"
    );
    assert!(device["layoutCodeFiles"].is_array());
    assert_eq!(device["notes"], serde_json::json!([]));

    let hero = &device["renderings"][1];
    assert_eq!(hero["renderingId"], "9a11aaaa-0002-4000-8000-000000000002");
    assert_eq!(hero["renderingName"], "Hero");
    assert_eq!(hero["placeholder"], "main");
    assert_eq!(hero["placeholderLeaf"], "main");
    assert_eq!(hero["uid"], "11111111-1111-4111-8111-111111111102");
    assert_eq!(hero["datasource"]["kind"], "item");
    assert_eq!(hero["datasource"]["raw"], "local:/Data/HeroData");
    assert_eq!(hero["datasource"]["id"], HERO_DATA);
    assert_eq!(hero["source"], "shared");
    assert_eq!(hero["parameters"], serde_json::json!({}));
    assert_eq!(
        hero["codeFiles"],
        serde_json::json!(["src/Views/Hero.cshtml"])
    );

    let navbar = &device["renderings"][0];
    assert_eq!(
        navbar["datasource"],
        serde_json::json!({"kind": "contextItem"})
    );
}

#[test]
fn resolve_is_deterministic() {
    let (graph, templates) = build();
    let a = serde_json::to_string(&resolve_home_en_v1(&graph, &templates)).unwrap();
    let b = serde_json::to_string(&resolve_home_en_v1(&graph, &templates)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn placeholder_scan_finds_main() {
    let (graph, _) = build();
    let scan = scan_placeholders(graph.root(), graph.repo_files());
    assert_eq!(scan.files_scanned, 3, "three .cshtml files in the fixture");
    assert!(scan.exposed.contains("main"), "{:?}", scan.exposed);
    assert_eq!(scan.exposed.len(), 1);
}
