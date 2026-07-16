//! Deterministic gate engine G1–G7: identical tree in, identical verdict
//! out, machine-readable reason for every failure (spec I5).
//!
//! Every gate emits [`Finding`]s with the exact reason codes and
//! severities of DESIGN.md §7. Findings are sorted by
//! `(gate, item_path, code, message)` (with deterministic tiebreakers)
//! and deduplicated, so the same tree always produces the same report.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use treesmith_format::{FieldRef, FieldSlot, ParsedItem};
use treesmith_graph::{Graph, ItemNode};
use treesmith_presentation::{PlaceholderScan, PresentationError, ResolvedPresentation};
use treesmith_template::{EffectiveTemplate, TemplateIndex};
use treesmith_types::{wellknown, Guid, SectionKind};

mod gates;

/// How bad a finding is. Serializes lowercase (`"error"` / `"warning"` /
/// `"info"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The tree is broken; `validate` exits non-zero.
    Error,
    /// Suspicious but possibly intentional.
    Warning,
    /// Informational only (e.g. dynamic datasources).
    Info,
}

/// One gate verdict with a machine-readable reason code (spec I5).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    /// The gate key, `"G1"`..`"G7"`.
    pub gate: &'static str,
    /// The reason code, e.g. `"g1.missing-datasource"`.
    pub code: String,
    /// Severity per the DESIGN.md §7 table.
    pub severity: Severity,
    /// The item the finding is attributed to, when known.
    #[serde(rename = "itemId")]
    pub item: Option<Guid>,
    /// That item's `Path`.
    #[serde(rename = "itemPath")]
    pub item_path: Option<String>,
    /// Root-relative forward-slash path of the backing file.
    pub file: Option<String>,
    /// Human-readable description.
    pub message: String,
    /// Machine-readable specifics (code-dependent shape).
    pub details: serde_json::Value,
}

/// Gate configuration (the kernel loads it from `<root>/treesmith.toml`;
/// an absent file means the defaults).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GateConfig {
    /// Gate keys (`"G1"`..`"G7"`) that `run_all` must skip.
    pub disabled: BTreeSet<String>,
    /// Languages every content item must carry a version in. `None`
    /// (no policy) skips G7 entirely.
    pub required_languages: Option<Vec<String>>,
    /// Path prefixes G7 applies to (case-insensitive).
    pub language_paths: Vec<String>,
}

impl Default for GateConfig {
    fn default() -> Self {
        GateConfig {
            disabled: BTreeSet::new(),
            required_languages: None,
            language_paths: vec!["/sitecore/content".to_string()],
        }
    }
}

/// Everything a gate may look at. Built once, shared by all gates.
pub struct GateCtx<'a> {
    /// The item graph (spec I1: derived from the working tree).
    pub graph: &'a Graph,
    /// The template index over the same graph.
    pub templates: &'a TemplateIndex,
    /// The static placeholder scan of the repo's views.
    pub placeholders: &'a PlaceholderScan,
    /// The effective gate configuration.
    pub config: &'a GateConfig,
}

/// The full gate run outcome: sorted findings plus `(gate, reason)`
/// pairs for gates that did not run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GateReport {
    /// All findings, sorted by `(gate, item_path, code, message)`.
    pub findings: Vec<Finding>,
    /// Gates that did not run: `(gate key, reason)`.
    pub skipped: Vec<(String, String)>,
}

/// Every gate key, in run order.
pub const GATES: &[&str] = &["G1", "G2", "G3", "G4", "G5", "G6", "G7"];

type GateFn = fn(&GateCtx, &mut Vec<Finding>);

const GATE_FNS: &[(&str, GateFn)] = &[
    ("G1", gates::g1_datasources),
    ("G2", gates::g2_layout_xml),
    ("G3", gates::g3_code_files),
    ("G4", gates::g4_placeholders),
    ("G5", gates::g5_field_refs),
    ("G6", gates::g6_conformance),
    ("G7", gates::g7_languages),
];

/// Runs every gate that the configuration does not disable. Disabled
/// gates and G7-without-a-policy land in [`GateReport::skipped`].
pub fn run_all(ctx: &GateCtx) -> GateReport {
    run_selected(ctx, |key| !ctx.config.disabled.contains(key), true)
}

/// Runs exactly the named gates (keys `"G1"`..`"G7"`, case-insensitive,
/// any order, duplicates collapsed; execution is always in `GATES`
/// order). An explicit request overrides `config.disabled`; G7 skip
/// semantics (no language policy) still apply. Unknown gate name →
/// `Err` (usage-class error).
pub fn run_some(ctx: &GateCtx, gates: &[String]) -> Result<GateReport, String> {
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for name in gates {
        let key = name.trim().to_ascii_uppercase();
        if !GATES.contains(&key.as_str()) {
            return Err(format!(
                "unknown gate `{name}` (expected one of {})",
                GATES.join(", ")
            ));
        }
        wanted.insert(key);
    }
    Ok(run_selected(ctx, |key| wanted.contains(key), false))
}

/// Shared driver: runs gates in `GATES` order, then sorts + dedups.
fn run_selected(ctx: &GateCtx, selected: impl Fn(&str) -> bool, note_disabled: bool) -> GateReport {
    let mut report = GateReport::default();
    for (key, gate_fn) in GATE_FNS {
        if !selected(key) {
            if note_disabled {
                report
                    .skipped
                    .push((key.to_string(), "disabled by config".to_string()));
            }
            continue;
        }
        if *key == "G7" && ctx.config.required_languages.is_none() {
            report
                .skipped
                .push((key.to_string(), "no language policy configured".to_string()));
            continue;
        }
        gate_fn(ctx, &mut report.findings);
    }
    sort_and_dedup(&mut report.findings);
    report
}

/// Deterministic order (DESIGN.md §7): `(gate, item_path, code, message)`
/// with `details`/`file` tiebreakers so the sort is total; exact
/// duplicates (e.g. the same shared-layout fault seen at several
/// language/version resolutions) collapse to one finding.
fn sort_and_dedup(findings: &mut Vec<Finding>) {
    findings.sort_by(|a, b| {
        (a.gate, &a.item_path, &a.code, &a.message)
            .cmp(&(b.gate, &b.item_path, &b.code, &b.message))
            .then_with(|| a.details.to_string().cmp(&b.details.to_string()))
            .then_with(|| a.file.cmp(&b.file))
    });
    findings.dedup();
}

// ---- shared helpers used by the gate implementations --------------------------

/// Builds a finding attributed to a graph item (path/file filled in when
/// the item is known).
fn finding_for(
    ctx: &GateCtx,
    gate: &'static str,
    code: &str,
    severity: Severity,
    item: Option<Guid>,
    message: String,
    details: serde_json::Value,
) -> Finding {
    let node = item.and_then(|id| ctx.graph.get(id));
    Finding {
        gate,
        code: code.to_string(),
        severity,
        item,
        item_path: node.map(|n| n.meta.path.clone()),
        file: node.map(|n| relative_file(ctx.graph.root(), &n.file)),
        message,
        details,
    }
}

/// Root-relative forward-slash rendering of an item's backing file.
fn relative_file(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    rel.to_string_lossy().replace('\\', "/")
}

/// The `(language, version)` pairs an item must be resolved at to cover
/// every layout value in its stack; items with no versions resolve once
/// at the defaults.
fn resolution_targets(node: &ItemNode) -> Vec<(Option<String>, Option<u32>)> {
    let mut targets: Vec<(Option<String>, Option<u32>)> = Vec::new();
    for (language, versions) in &node.meta.languages {
        for version in versions {
            targets.push((Some(language.clone()), Some(*version)));
        }
    }
    if targets.is_empty() {
        targets.push((None, None));
    }
    targets
}

/// Resolves every item at every language/version (G1/G2/G4 walk the full
/// layout stack this way, DESIGN.md §7 notes), invoking `f` per
/// resolution attempt.
fn for_each_resolution(
    ctx: &GateCtx,
    mut f: impl FnMut(&ItemNode, &Result<ResolvedPresentation, PresentationError>),
) {
    for id in ctx.graph.ids_by_path() {
        let Some(node) = ctx.graph.get(id) else {
            continue;
        };
        for (language, version) in resolution_targets(node) {
            let result = treesmith_presentation::resolve(
                ctx.graph,
                ctx.templates,
                id,
                language.as_deref(),
                version,
            );
            f(node, &result);
        }
    }
}

/// Every serialized field with its slot, in deterministic order: shared,
/// then languages alphabetically (unversioned first, versions ascending).
fn all_fields(item: &ParsedItem) -> Vec<(FieldSlot, FieldRef)> {
    let mut out = Vec::new();
    for f in item.shared_fields() {
        out.push((FieldSlot::Shared, f));
    }
    let mut languages = item.languages();
    languages.sort_by(|a, b| a.language.cmp(&b.language));
    for lang in languages {
        for f in lang.unversioned {
            out.push((
                FieldSlot::Unversioned {
                    language: lang.language.clone(),
                },
                f,
            ));
        }
        let mut versions = lang.versions;
        versions.sort_by_key(|(n, _)| *n);
        for (version, fields) in versions {
            for f in fields {
                out.push((
                    FieldSlot::Versioned {
                        language: lang.language.clone(),
                        version,
                    },
                    f,
                ));
            }
        }
    }
    out
}

/// Stable human/machine rendering of a slot for messages and details.
fn slot_label(slot: &FieldSlot) -> String {
    match slot {
        FieldSlot::Shared => "shared".to_string(),
        FieldSlot::Unversioned { language } => format!("{language} (unversioned)"),
        FieldSlot::Versioned { language, version } => format!("{language} #{version}"),
    }
}

/// The section kind a slot physically corresponds to.
fn slot_section(slot: &FieldSlot) -> SectionKind {
    match slot {
        FieldSlot::Shared => SectionKind::Shared,
        FieldSlot::Unversioned { .. } => SectionKind::Unversioned,
        FieldSlot::Versioned { .. } => SectionKind::Versioned,
    }
}

/// Well-known platform system fields (`__Renderings`, `__Base template`,
/// ...) are exempt from template-conformance checks: real repos rarely
/// serialize the system templates that define them (DESIGN.md §8).
fn is_wellknown_field(id: Guid) -> bool {
    [
        wellknown::BASE_TEMPLATE_FIELD,
        wellknown::STANDARD_VALUES_FIELD,
        wellknown::FIELD_TYPE_FIELD,
        wellknown::FIELD_SHARED_FIELD,
        wellknown::FIELD_UNVERSIONED_FIELD,
        wellknown::LAYOUT_FIELD,
        wellknown::FINAL_LAYOUT_FIELD,
        wellknown::DISPLAY_NAME_FIELD,
        wellknown::SORTORDER_FIELD,
        wellknown::CREATED_FIELD,
        wellknown::CREATED_BY_FIELD,
        wellknown::LAYOUT_PATH_FIELD,
    ]
    .contains(&id)
}

/// Well-known platform templates: items using them are structural
/// (templates, renderings, folders) whose defining templates are never
/// serialized, so G6 conformance does not apply to them.
fn is_wellknown_template(id: Guid) -> bool {
    [
        wellknown::TEMPLATE_TEMPLATE,
        wellknown::TEMPLATE_SECTION,
        wellknown::TEMPLATE_FIELD,
        wellknown::TEMPLATE_FOLDER,
        wellknown::STANDARD_TEMPLATE,
        wellknown::FOLDER,
        wellknown::VIEW_RENDERING,
        wellknown::CONTROLLER_RENDERING,
        wellknown::LAYOUT,
        wellknown::PLACEHOLDER_SETTINGS,
    ]
    .contains(&id)
}

/// Display name for a field in messages: template name, else hint, else id.
fn field_display_name(def_name: Option<&str>, field: &FieldRef) -> String {
    if let Some(name) = def_name {
        return name.to_string();
    }
    if let Some(hint) = field.hint.as_deref() {
        return hint.to_string();
    }
    field.id.rainbow()
}

/// Reference family for G5: multilist family plus single-item reference
/// types (DESIGN.md §7).
fn is_reference_type(t: &str) -> bool {
    treesmith_format::valuefmt::is_multilist_type(t)
        || ["Droplink", "Droptree", "Reference", "Grouped Droplink"]
            .iter()
            .any(|k| k.eq_ignore_ascii_case(t))
}

/// General Link family: `id=` attributes checked when
/// `linktype="internal"` (G5).
fn is_general_link_type(t: &str) -> bool {
    t.eq_ignore_ascii_case("General Link") || t.eq_ignore_ascii_case("General Link with Search")
}

/// The field's effective type: template definition first, else the
/// serialized `Type:` hint.
fn effective_field_type<'a>(
    effective: Option<&'a EffectiveTemplate>,
    field: &'a FieldRef,
) -> Option<&'a str> {
    if let Some(def) = effective.and_then(|e| e.field_by_id(field.id)) {
        return Some(&def.field_type);
    }
    field.type_hint.as_deref()
}

/// Case-insensitive "is `path` at or under `prefix`" with a `/` boundary.
fn path_under(path: &str, prefix: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let prefix = prefix.trim_end_matches('/').to_ascii_lowercase();
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn severity_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Severity::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
    }

    #[test]
    fn default_config_has_content_path_and_no_policy() {
        let cfg = GateConfig::default();
        assert!(cfg.disabled.is_empty());
        assert!(cfg.required_languages.is_none());
        assert_eq!(cfg.language_paths, vec!["/sitecore/content".to_string()]);
    }

    #[test]
    fn path_under_respects_segment_boundaries() {
        assert!(path_under("/sitecore/content", "/sitecore/content"));
        assert!(path_under("/Sitecore/Content/Home", "/sitecore/content"));
        assert!(!path_under("/sitecore/contentions", "/sitecore/content"));
        assert!(!path_under("/sitecore/media library", "/sitecore/content"));
    }

    #[test]
    fn finding_serializes_camel_case_keys() {
        let f = Finding {
            gate: "G1",
            code: "g1.missing-datasource".to_string(),
            severity: Severity::Error,
            item: None,
            item_path: Some("/sitecore/content/X".to_string()),
            file: None,
            message: "m".to_string(),
            details: serde_json::json!({}),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.as_object().unwrap().contains_key("itemId"));
        assert!(v.as_object().unwrap().contains_key("itemPath"));
        assert_eq!(v["severity"], "error");
    }

    #[test]
    fn sort_is_total_and_dedups() {
        let mk = |code: &str, msg: &str| Finding {
            gate: "G5",
            code: code.to_string(),
            severity: Severity::Error,
            item: None,
            item_path: Some("/a".to_string()),
            file: None,
            message: msg.to_string(),
            details: serde_json::json!({}),
        };
        let mut v = vec![mk("b", "x"), mk("a", "y"), mk("a", "y"), mk("a", "x")];
        sort_and_dedup(&mut v);
        assert_eq!(
            v.iter()
                .map(|f| (f.code.as_str(), f.message.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "x"), ("a", "y"), ("b", "x")]
        );
    }
}
