//! Gate engine integration tests against the fixture repos
//! (DESIGN.md §13/§14): `rainbow/basic` must be spotless, `rainbow/broken`
//! must trip every expected reason code exactly once, and identical trees
//! must yield identical reports (spec I5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use treesmith_gate::{run_all, run_some, GateConfig, GateCtx, GateReport, Severity, GATES};
use treesmith_graph::Graph;
use treesmith_presentation::{scan_placeholders, PlaceholderScan};
use treesmith_template::TemplateIndex;

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/rainbow")
        .join(name)
}

/// Loads `<root>/treesmith.toml` the way the kernel does (DESIGN.md §8):
/// absent file = defaults; `[gates] disabled`; `[gates.language-policy]
/// required` arms G7 and `paths` overrides the default prefix list.
fn load_config(root: &Path) -> GateConfig {
    let mut config = GateConfig::default();
    let Ok(text) = std::fs::read_to_string(root.join("treesmith.toml")) else {
        return config;
    };
    let value: toml::Value = text.parse().expect("treesmith.toml parses");
    let Some(gates) = value.get("gates") else {
        return config;
    };
    if let Some(disabled) = gates.get("disabled").and_then(|v| v.as_array()) {
        config.disabled = disabled
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect();
    }
    if let Some(policy) = gates.get("language-policy") {
        if let Some(required) = policy.get("required").and_then(|v| v.as_array()) {
            config.required_languages = Some(
                required
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect(),
            );
        }
        if let Some(paths) = policy.get("paths").and_then(|v| v.as_array()) {
            config.language_paths = paths
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
        }
    }
    config
}

struct Fixture {
    graph: Graph,
    templates: TemplateIndex,
    placeholders: PlaceholderScan,
    config: GateConfig,
}

impl Fixture {
    fn open(name: &str) -> Fixture {
        let root = fixture_root(name);
        assert!(root.is_dir(), "fixture {name} missing at {root:?}");
        let graph = Graph::build(&root);
        assert!(
            graph.faults().is_empty(),
            "fixture {name} has tree faults: {:?}",
            graph.faults()
        );
        let templates = TemplateIndex::build(&graph);
        let placeholders = scan_placeholders(graph.root(), graph.repo_files());
        let config = load_config(graph.root());
        Fixture {
            graph,
            templates,
            placeholders,
            config,
        }
    }

    fn ctx(&self) -> GateCtx<'_> {
        GateCtx {
            graph: &self.graph,
            templates: &self.templates,
            placeholders: &self.placeholders,
            config: &self.config,
        }
    }
}

// ---- basic: a healthy tree is completely clean --------------------------------

#[test]
fn basic_report_is_empty_and_g7_skipped() {
    let fx = Fixture::open("basic");
    let report = run_all(&fx.ctx());
    assert_eq!(
        report.findings,
        Vec::new(),
        "healthy fixture must produce zero findings of any severity, got: {}",
        serde_json::to_string_pretty(&report.findings).unwrap()
    );
    assert_eq!(
        report.skipped,
        vec![(
            "G7".to_string(),
            "no language policy configured".to_string()
        )],
        "without a language policy, only G7 is skipped"
    );
}

// ---- broken: every expected code exactly once, correct severities -------------

#[test]
fn broken_trips_every_gate_exactly_once() {
    let fx = Fixture::open("broken");
    assert_eq!(
        fx.config.required_languages,
        Some(vec!["en".to_string(), "da".to_string()]),
        "broken fixture's treesmith.toml arms G7"
    );
    let report = run_all(&fx.ctx());

    let expected: &[(&str, Severity)] = &[
        ("g1.missing-datasource", Severity::Error),
        ("g2.malformed-xml", Severity::Error),
        ("g2.unknown-uid", Severity::Error),
        ("g3.missing-view", Severity::Error),
        ("g4.placeholder-not-exposed", Severity::Warning),
        ("g5.broken-reference", Severity::Error),
        ("g6.unknown-field", Severity::Error),
        ("g6.wrong-section", Severity::Error),
        ("g7.missing-language", Severity::Error),
    ];

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for finding in &report.findings {
        *counts.entry(finding.code.as_str()).or_default() += 1;
    }
    let expected_counts: BTreeMap<&str, usize> = expected.iter().map(|(c, _)| (*c, 1)).collect();
    assert_eq!(
        counts,
        expected_counts,
        "each expected code exactly once, nothing else; full report: {}",
        serde_json::to_string_pretty(&report.findings).unwrap()
    );
    for (code, severity) in expected {
        let finding = report
            .findings
            .iter()
            .find(|f| f.code == *code)
            .expect("code present");
        assert_eq!(
            finding.severity, *severity,
            "severity mismatch for {code}: {finding:?}"
        );
    }
    assert!(report.skipped.is_empty(), "nothing disabled, policy armed");
}

#[test]
fn broken_findings_land_on_the_right_items() {
    let fx = Fixture::open("broken");
    let report = run_all(&fx.ctx());
    let path_of = |code: &str| {
        report
            .findings
            .iter()
            .find(|f| f.code == code)
            .and_then(|f| f.item_path.clone())
            .unwrap_or_default()
    };
    assert_eq!(path_of("g1.missing-datasource"), "/sitecore/content/Alpha");
    assert_eq!(path_of("g2.malformed-xml"), "/sitecore/content/Bravo");
    assert_eq!(path_of("g2.unknown-uid"), "/sitecore/content/Charlie");
    assert_eq!(
        path_of("g3.missing-view"),
        "/sitecore/layout/Renderings/BrokenView"
    );
    assert_eq!(
        path_of("g4.placeholder-not-exposed"),
        "/sitecore/content/Dee"
    );
    assert_eq!(path_of("g5.broken-reference"), "/sitecore/content/Echo");
    assert_eq!(path_of("g6.unknown-field"), "/sitecore/content/Foxtrot");
    assert_eq!(path_of("g6.wrong-section"), "/sitecore/content/Foxtrot");
    assert_eq!(path_of("g7.missing-language"), "/sitecore/content/Golf");

    // Every finding carries the machine-consumable attribution surface.
    for finding in &report.findings {
        assert!(finding.item.is_some(), "item id set: {finding:?}");
        assert!(
            finding.file.as_deref().is_some_and(|f| f.ends_with(".yml")),
            "file set: {finding:?}"
        );
        assert!(!finding.message.is_empty());
    }
}

// ---- determinism (spec I5) ----------------------------------------------------

#[test]
fn run_all_is_deterministic() {
    for name in ["basic", "broken"] {
        let fx = Fixture::open(name);
        let first = serde_json::to_string(&run_all(&fx.ctx())).unwrap();
        let second = serde_json::to_string(&run_all(&fx.ctx())).unwrap();
        assert_eq!(first, second, "{name}: identical tree, identical report");

        // A freshly built context over the same tree agrees too.
        let fx2 = Fixture::open(name);
        let third = serde_json::to_string(&run_all(&fx2.ctx())).unwrap();
        assert_eq!(first, third, "{name}: rebuild-stable report");
    }
}

#[test]
fn findings_are_sorted_by_gate_path_code_message() {
    let fx = Fixture::open("broken");
    let report = run_all(&fx.ctx());
    let keys: Vec<_> = report
        .findings
        .iter()
        .map(|f| {
            (
                f.gate,
                f.item_path.clone(),
                f.code.clone(),
                f.message.clone(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

// ---- run_some -----------------------------------------------------------------

#[test]
fn run_some_rejects_unknown_gate() {
    let fx = Fixture::open("basic");
    let err = run_some(&fx.ctx(), &["G1".to_string(), "G9".to_string()])
        .expect_err("unknown gate must be a usage error");
    assert!(err.contains("G9"), "error names the bad gate: {err}");
    assert!(
        run_some(&fx.ctx(), &["gates".to_string()]).is_err(),
        "non-key names are rejected"
    );
}

#[test]
fn run_some_runs_exactly_the_requested_gates() {
    let fx = Fixture::open("broken");

    let g7_only = run_some(&fx.ctx(), &["G7".to_string()]).unwrap();
    assert_eq!(g7_only.findings.len(), 1);
    assert_eq!(g7_only.findings[0].code, "g7.missing-language");
    assert!(g7_only.skipped.is_empty());

    // Case-insensitive keys, duplicates collapsed, output order stable.
    let g2_twice = run_some(&fx.ctx(), &["g2".to_string(), "G2".to_string()]).unwrap();
    let codes: Vec<&str> = g2_twice.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, vec!["g2.malformed-xml", "g2.unknown-uid"]);

    // The full explicit list matches run_all under the same config.
    let all_names: Vec<String> = GATES.iter().map(|g| g.to_string()).collect();
    let explicit = run_some(&fx.ctx(), &all_names).unwrap();
    assert_eq!(explicit.findings, run_all(&fx.ctx()).findings);
}

#[test]
fn run_some_of_g7_without_policy_reports_skip() {
    let fx = Fixture::open("basic");
    let report = run_some(&fx.ctx(), &["G7".to_string()]).unwrap();
    assert!(report.findings.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "G7".to_string(),
            "no language policy configured".to_string()
        )]
    );
}

// ---- disabled gates -----------------------------------------------------------

#[test]
fn disabled_gates_are_skipped_by_run_all() {
    let fx = Fixture::open("broken");
    let mut config = fx.config.clone();
    config.disabled.insert("G2".to_string());
    let ctx = GateCtx {
        graph: &fx.graph,
        templates: &fx.templates,
        placeholders: &fx.placeholders,
        config: &config,
    };
    let report: GateReport = run_all(&ctx);
    assert!(
        report.findings.iter().all(|f| f.gate != "G2"),
        "disabled gate contributed findings"
    );
    assert!(report
        .skipped
        .contains(&("G2".to_string(), "disabled by config".to_string())));

    // An explicit run_some request overrides the disable switch.
    let explicit = run_some(&ctx, &["G2".to_string()]).unwrap();
    assert_eq!(explicit.findings.len(), 2);
}
