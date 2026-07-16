//! Rendering-to-code-file map and the static placeholder scan
//! (DESIGN.md §6.4).

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use treesmith_format::{FieldRef, ParsedItem};
use treesmith_graph::{Graph, ItemNode, RepoFiles};
use treesmith_template::TemplateIndex;
use treesmith_types::{wellknown, Guid};

/// What kind of code artifact a rendering item points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CodeKind {
    /// A view rendering (`.cshtml` via its `Path` field).
    View,
    /// A controller rendering (controller class via its `Controller` field).
    Controller,
    /// A layout item (`.cshtml` via its `Path` field).
    Layout,
}

/// A rendering item's code reference: the raw field value and every repo
/// file it resolves to (empty when nothing matches).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRef {
    /// Artifact kind, from the item's template.
    pub kind: CodeKind,
    /// The raw field value (`Path` / `Controller`); empty when the field
    /// is missing or blank.
    pub raw: String,
    /// Matching repo files (forward-slash repo-relative, sorted order of
    /// [`RepoFiles::all`]).
    pub files: Vec<String>,
}

/// Classifies `node` by template and resolves its code files. `None` for
/// items that are not view renderings, controller renderings, or layouts.
///
/// Field lookup precedence (DESIGN.md §6.4): the resolved effective
/// template's field by name first, else a serialized-field hint match —
/// real repos rarely serialize system templates, so hints are the robust
/// path.
pub fn rendering_code(
    graph: &Graph,
    templates: &TemplateIndex,
    node: &ItemNode,
) -> Option<CodeRef> {
    let kind = match node.meta.template {
        Some(t) if t == wellknown::VIEW_RENDERING => CodeKind::View,
        Some(t) if t == wellknown::LAYOUT => CodeKind::Layout,
        Some(t) if t == wellknown::CONTROLLER_RENDERING => CodeKind::Controller,
        _ => return None,
    };
    match kind {
        CodeKind::View | CodeKind::Layout => {
            let raw = field_value(templates, node, "Path").unwrap_or_default();
            let files = if raw.trim().is_empty() {
                Vec::new()
            } else {
                graph
                    .repo_files()
                    .find_suffix(raw.trim())
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            };
            Some(CodeRef { kind, raw, files })
        }
        CodeKind::Controller => {
            let raw = field_value(templates, node, "Controller").unwrap_or_default();
            let files = if raw.trim().is_empty() {
                Vec::new()
            } else {
                find_controller_files(graph, &short_type_name(&raw))
            };
            Some(CodeRef { kind, raw, files })
        }
    }
}

/// Reads a field off an item with the §6.4 precedence: effective-template
/// field name resolution first, then serialized `Hint:` matching.
///
/// Both paths read slots deterministically: shared, then languages
/// alphabetically (unversioned first, then versions ascending).
pub(crate) fn field_value(
    templates: &TemplateIndex,
    node: &ItemNode,
    field_name: &str,
) -> Option<String> {
    if let Some(effective) = node.meta.template.and_then(|t| templates.resolve(t)) {
        if let Some(def) = effective.field_by_name(field_name) {
            if let Some(v) = field_value_by_id(&node.item, def.id) {
                return Some(v);
            }
        }
    }
    field_value_by_hint(&node.item, field_name)
}

fn field_value_by_id(item: &ParsedItem, id: Guid) -> Option<String> {
    item.find_field(id).map(|(_, f)| f.value)
}

fn field_value_by_hint(item: &ParsedItem, hint: &str) -> Option<String> {
    let matches = |f: &FieldRef| {
        f.hint
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(hint))
    };
    if let Some(f) = item.shared_fields().iter().find(|f| matches(f)) {
        return Some(f.value.clone());
    }
    let mut languages = item.languages();
    languages.sort_by(|a, b| a.language.cmp(&b.language));
    for lang in &languages {
        if let Some(f) = lang.unversioned.iter().find(|f| matches(f)) {
            return Some(f.value.clone());
        }
        let mut versions = lang.versions.clone();
        versions.sort_by_key(|(n, _)| *n);
        for (_, fields) in &versions {
            if let Some(f) = fields.iter().find(|f| matches(f)) {
                return Some(f.value.clone());
            }
        }
    }
    None
}

/// `Namespace.Type, Assembly` / `Namespace.Type` / `Type` → `Type`.
fn short_type_name(controller: &str) -> String {
    let type_part = controller.split(',').next().unwrap_or("").trim();
    type_part
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Scans every `.cs` repo file for a `class <Name>` declaration with word
/// boundaries on both sides of the name.
fn find_controller_files(graph: &Graph, class_name: &str) -> Vec<String> {
    if class_name.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for rel in graph.repo_files().with_extension("cs") {
        let Ok(text) = std::fs::read_to_string(graph.root().join(rel)) else {
            continue;
        };
        if declares_class(&text, class_name) {
            out.push(rel.to_string());
        }
    }
    out
}

/// Line scan for `class <Name>` where both `class` and the name sit on
/// identifier boundaries (no regex dependency).
fn declares_class(text: &str, name: &str) -> bool {
    for line in text.lines() {
        let mut rest = line;
        while let Some(pos) = rest.find("class") {
            let before_ok = pos == 0
                || !rest[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = &rest[pos + "class".len()..];
            let after_kw = after.trim_start();
            let trimmed_ws = after.len() != after_kw.len();
            if before_ok && trimmed_ws && after_kw.starts_with(name) {
                let tail = &after_kw[name.len()..];
                let boundary = tail
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if boundary {
                    return true;
                }
            }
            rest = &rest[pos + "class".len()..];
        }
    }
    false
}

/// The set of placeholder names statically exposed by the repo's views.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceholderScan {
    /// Every placeholder name found (deduplicated, sorted).
    pub exposed: BTreeSet<String>,
    /// How many `.cshtml` files were scanned.
    pub files_scanned: usize,
}

/// Scans every `.cshtml` file for `.Placeholder("NAME")` and
/// `DynamicPlaceholder("NAME")` calls (method name matched
/// case-insensitively; NAME is the first string argument).
pub fn scan_placeholders(root: &Path, files: &RepoFiles) -> PlaceholderScan {
    let mut scan = PlaceholderScan::default();
    for rel in files.with_extension("cshtml") {
        scan.files_scanned += 1;
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        collect_placeholders(&text, &mut scan.exposed);
    }
    scan
}

fn collect_placeholders(text: &str, out: &mut BTreeSet<String>) {
    let lower = text.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find("placeholder") {
        let start = from + rel;
        let end = start + "placeholder".len();
        from = end;
        // Identifier must *end* at the match: `.Placeholder` or
        // `DynamicPlaceholder`, nothing else (`Placeholders(` is out).
        let ident_start = lower[..start]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let ident = &lower[ident_start..end];
        if ident != "placeholder" && ident != "dynamicplaceholder" {
            continue;
        }
        // First argument must be a double-quoted string literal.
        let after = text[end..].trim_start();
        let Some(args) = after.strip_prefix('(') else {
            continue;
        };
        let args = args.trim_start();
        let Some(quoted) = args.strip_prefix('"') else {
            continue;
        };
        if let Some(close) = quoted.find('"') {
            out.insert(quoted[..close].to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_name_variants() {
        assert_eq!(short_type_name("NavBarController"), "NavBarController");
        assert_eq!(
            short_type_name("Sample.Controllers.NavBarController"),
            "NavBarController"
        );
        assert_eq!(
            short_type_name("Sample.Controllers.NavBarController, Sample.Web"),
            "NavBarController"
        );
        assert_eq!(short_type_name(""), "");
    }

    #[test]
    fn class_declaration_needs_word_boundaries() {
        assert!(declares_class(
            "public class NavBarController : Controller",
            "NavBarController"
        ));
        assert!(declares_class(
            "class NavBarController{",
            "NavBarController"
        ));
        assert!(declares_class(
            "    internal sealed class  NavBarController\n",
            "NavBarController"
        ));
        assert!(!declares_class(
            "class NavBarControllerBase",
            "NavBarController"
        ));
        assert!(!declares_class(
            "subclass NavBarController",
            "NavBarController"
        ));
        assert!(!declares_class("classNavBarController", "NavBarController"));
        assert!(!declares_class(
            "// mentions NavBarController only",
            "NavBarController"
        ));
    }

    #[test]
    fn placeholder_patterns_match_case_insensitively() {
        let mut out = BTreeSet::new();
        collect_placeholders(
            "@Html.Sitecore().Placeholder(\"main\")\n\
             @Html.Sitecore().placeholder( \"footer\" )\n\
             @Html.Sitecore().DynamicPlaceholder(\"col\")\n\
             @Html.Sitecore().dynamicplaceholder(\"col2\")",
            &mut out,
        );
        let names: Vec<&str> = out.iter().map(String::as_str).collect();
        assert_eq!(names, vec!["col", "col2", "footer", "main"]);
    }

    #[test]
    fn placeholder_scan_ignores_non_calls() {
        let mut out = BTreeSet::new();
        collect_placeholders(
            "var placeholderText = 1;\n\
             GetPlaceholder(\"nope\")\n\
             Placeholders(\"nope\")\n\
             Placeholder(name)\n\
             Placeholder(\"yes\")",
            &mut out,
        );
        let names: Vec<&str> = out.iter().map(String::as_str).collect();
        assert_eq!(names, vec!["yes"]);
    }
}
