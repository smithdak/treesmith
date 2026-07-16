//! Presentation resolution: layout XML parsing, final-renderings delta
//! merge, datasource resolution, and the rendering-to-code-file map
//! (spec §3.1 "resolved renderings", DESIGN.md §6).

pub mod codemap;
pub mod delta;
pub mod layoutxml;

use std::collections::BTreeMap;

use serde::Serialize;
use treesmith_graph::{Graph, ItemNode};
use treesmith_template::TemplateIndex;
use treesmith_types::{wellknown, Guid};

pub use codemap::{rendering_code, scan_placeholders, CodeKind, CodeRef, PlaceholderScan};
pub use delta::{apply_delta, DeltaNote};
pub use layoutxml::{parse_xml, XmlEl, XmlError};

use delta::guid_key;

/// Why presentation resolution failed for an item.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PresentationError {
    /// The requested item is not in the graph.
    #[error("item {0} not found in graph")]
    ItemNotFound(Guid),
    /// A layout layer in the stack failed to parse as XML.
    #[error("malformed layout XML on item {item} ({field}): {error}")]
    MalformedLayoutXml {
        /// The item whose layout field is malformed (the requested item
        /// or a standard-values item in its template chain).
        item: Guid,
        /// `"__Renderings"` or `"__Final Renderings"`.
        field: &'static str,
        /// The underlying XML error (message + byte offset).
        error: XmlError,
    },
}

/// The fully merged presentation of one item at one language/version.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPresentation {
    /// The requested item.
    pub item_id: Guid,
    /// The requested item's `Path`.
    pub item_path: String,
    /// The language actually resolved (requested or defaulted).
    pub language: String,
    /// The version actually resolved (requested or defaulted).
    pub version: u32,
    /// One entry per device in the merged layout, in layout order.
    pub devices: Vec<ResolvedDevice>,
}

/// One device's merged layout.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedDevice {
    /// The device GUID (normalized) or the raw `id=` value.
    pub device_id: String,
    /// The bound layout item, if any (`l=` on the device).
    pub layout: Option<LayoutRef>,
    /// Repo files backing the layout (empty when unresolvable).
    pub layout_code_files: Vec<String>,
    /// Renderings in merged order.
    pub renderings: Vec<ResolvedRendering>,
    /// Delta-merge oddities recorded against this device (gate G2 input).
    pub notes: Vec<DeltaNote>,
}

/// The layout bound to a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutRef {
    /// The layout item GUID (normalized) or the raw `l=` value.
    pub id: String,
    /// The layout item's `Path`, when it is in the graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One rendering in a merged device.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRendering {
    /// The rendering instance uid (normalized), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The rendering definition item GUID (normalized), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendering_id: Option<String>,
    /// The rendering definition item's name, when it is in the graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendering_name: Option<String>,
    /// The raw `ph=` placeholder path (empty when absent).
    pub placeholder: String,
    /// Last placeholder segment, dynamic-placeholder suffix stripped.
    pub placeholder_leaf: String,
    /// Where the rendering's datasource points.
    pub datasource: DatasourceResolution,
    /// `par=` parameters, `&`-split, `=`-split, percent-decoded.
    pub parameters: BTreeMap<String, String>,
    /// Repo files backing the rendering (empty when unresolvable).
    pub code_files: Vec<String>,
    /// `"shared"` when the rendering originates in the shared layout
    /// stack, `"final"` when a final-renderings delta introduced it.
    pub source: String,
}

/// Where a rendering datasource points (DESIGN.md §6.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DatasourceResolution {
    /// Empty datasource: the rendering binds the context item.
    ContextItem,
    /// Resolved to a graph item (GUID, `local:`, or absolute path form).
    Item {
        /// The datasource value as written.
        raw: String,
        /// The resolved item.
        id: Guid,
        /// The resolved item's `Path`.
        path: String,
    },
    /// Unresolvable: GUID/path not in the graph, or unrecognized text.
    Missing {
        /// The datasource value as written.
        raw: String,
    },
    /// A runtime-evaluated datasource (`query:`, `code:`, ...) that
    /// static analysis cannot follow.
    Dynamic {
        /// The datasource value as written.
        raw: String,
        /// The scheme before the first `:`.
        scheme: String,
    },
}

/// Resolves the effective presentation of `item` (DESIGN.md §6.3).
///
/// Layout stacking, base-most first, each layer applied with
/// [`apply_delta`]:
///
/// 1. shared `__Renderings` of standard-values items along
///    `std_values_chain` in *reverse* (base-most template's std values
///    first);
/// 2. the item's own shared `__Renderings`;
/// 3. `__Final Renderings` of the std-values items (requested language,
///    highest version ≤ requested), same reverse order;
/// 4. the item's own `__Final Renderings` for the language/version.
///
/// Defaults: language = first language with versions on the item, else
/// `en`; version = max for that language.
pub fn resolve(
    graph: &Graph,
    templates: &TemplateIndex,
    item: Guid,
    language: Option<&str>,
    version: Option<u32>,
) -> Result<ResolvedPresentation, PresentationError> {
    let node = graph
        .get(item)
        .ok_or(PresentationError::ItemNotFound(item))?;

    let language = language.map(str::to_string).unwrap_or_else(|| {
        node.meta
            .languages
            .iter()
            .find(|(_, versions)| !versions.is_empty())
            .map(|(l, _)| l.clone())
            .unwrap_or_else(|| "en".to_string())
    });
    let version = version.unwrap_or_else(|| {
        node.meta
            .languages
            .iter()
            .find(|(l, _)| l == &language)
            .and_then(|(_, versions)| versions.iter().max().copied())
            .unwrap_or(1)
    });

    let layers = collect_layers(graph, templates, node, &language, version);

    // Shared-only merge, used to mark rendering provenance.
    let shared_merged = merge_layers(layers.iter().filter(|l| l.kind == LayerKind::Shared))?.0;
    let (merged, notes) = merge_layers(layers.iter())?;

    let mut devices = Vec::new();
    if let Some(root) = merged {
        for device in root.children.iter().filter(|c| c.name == "d") {
            devices.push(resolve_device(
                graph,
                templates,
                node,
                device,
                shared_merged.as_ref(),
                &notes,
            ));
        }
    }

    Ok(ResolvedPresentation {
        item_id: item,
        item_path: node.meta.path.clone(),
        language,
        version,
        devices,
    })
}

// ---- layer collection -------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerKind {
    Shared,
    Final,
}

struct Layer {
    kind: LayerKind,
    /// The item the layer's value was read from (error attribution).
    source_item: Guid,
    field: &'static str,
    value: String,
}

const RENDERINGS: &str = "__Renderings";
const FINAL_RENDERINGS: &str = "__Final Renderings";

/// Gathers the non-empty layout values in stacking order (base-most first).
fn collect_layers(
    graph: &Graph,
    templates: &TemplateIndex,
    node: &ItemNode,
    language: &str,
    version: u32,
) -> Vec<Layer> {
    let mut std_values: Vec<Guid> = node
        .meta
        .template
        .map(|t| templates.std_values_chain(t))
        .unwrap_or_default();
    std_values.reverse(); // base-most template's std values first

    let mut layers = Vec::new();
    for sv in &std_values {
        if let Some(sv_node) = graph.get(*sv) {
            if let Some(v) = shared_field(sv_node, wellknown::LAYOUT_FIELD) {
                layers.push(Layer {
                    kind: LayerKind::Shared,
                    source_item: *sv,
                    field: RENDERINGS,
                    value: v,
                });
            }
        }
    }
    if let Some(v) = shared_field(node, wellknown::LAYOUT_FIELD) {
        layers.push(Layer {
            kind: LayerKind::Shared,
            source_item: node.id,
            field: RENDERINGS,
            value: v,
        });
    }
    for sv in &std_values {
        if let Some(sv_node) = graph.get(*sv) {
            if let Some(v) = final_field_at_most(sv_node, language, version) {
                layers.push(Layer {
                    kind: LayerKind::Final,
                    source_item: *sv,
                    field: FINAL_RENDERINGS,
                    value: v,
                });
            }
        }
    }
    if let Some(v) = final_field_exact(node, language, version) {
        layers.push(Layer {
            kind: LayerKind::Final,
            source_item: node.id,
            field: FINAL_RENDERINGS,
            value: v,
        });
    }
    layers.retain(|l| !l.value.trim().is_empty());
    layers
}

fn shared_field(node: &ItemNode, field: Guid) -> Option<String> {
    node.item
        .shared_fields()
        .into_iter()
        .find(|f| f.id == field)
        .map(|f| f.value)
}

/// `__Final Renderings` at the highest version ≤ `version` in `language`.
fn final_field_at_most(node: &ItemNode, language: &str, version: u32) -> Option<String> {
    let languages = node.item.languages();
    let lang = languages.iter().find(|l| l.language == language)?;
    let (_, fields) = lang
        .versions
        .iter()
        .filter(|(n, _)| *n <= version)
        .max_by_key(|(n, _)| *n)?;
    fields
        .iter()
        .find(|f| f.id == wellknown::FINAL_LAYOUT_FIELD)
        .map(|f| f.value.clone())
}

/// `__Final Renderings` at exactly (`language`, `version`).
fn final_field_exact(node: &ItemNode, language: &str, version: u32) -> Option<String> {
    let languages = node.item.languages();
    let lang = languages.iter().find(|l| l.language == language)?;
    let (_, fields) = lang.versions.iter().find(|(n, _)| *n == version)?;
    fields
        .iter()
        .find(|f| f.id == wellknown::FINAL_LAYOUT_FIELD)
        .map(|f| f.value.clone())
}

/// Parses each layer and folds them with [`apply_delta`], base-most first.
fn merge_layers<'a>(
    layers: impl Iterator<Item = &'a Layer>,
) -> Result<(Option<XmlEl>, Vec<DeltaNote>), PresentationError> {
    let mut merged: Option<XmlEl> = None;
    let mut notes = Vec::new();
    for layer in layers {
        let parsed =
            parse_xml(&layer.value).map_err(|error| PresentationError::MalformedLayoutXml {
                item: layer.source_item,
                field: layer.field,
                error,
            })?;
        merged = Some(match merged {
            None => parsed,
            Some(base) => {
                let (next, layer_notes) = apply_delta(&base, &parsed);
                notes.extend(layer_notes);
                next
            }
        });
    }
    Ok((merged, notes))
}

// ---- device / rendering shaping ----------------------------------------------

fn resolve_device(
    graph: &Graph,
    templates: &TemplateIndex,
    page: &ItemNode,
    device: &XmlEl,
    shared_merged: Option<&XmlEl>,
    all_notes: &[DeltaNote],
) -> ResolvedDevice {
    let raw_id = device.attr("id").unwrap_or("");
    let device_key = guid_key(raw_id);

    let layout = device
        .attr("l")
        .filter(|l| !l.is_empty())
        .map(|l| match Guid::parse(l) {
            Ok(g) => LayoutRef {
                id: g.rainbow(),
                path: graph.get(g).map(|n| n.meta.path.clone()),
            },
            Err(_) => LayoutRef {
                id: l.to_string(),
                path: None,
            },
        });
    let layout_code_files = device
        .attr("l")
        .and_then(|l| Guid::parse(l).ok())
        .and_then(|g| graph.get(g))
        .and_then(|n| rendering_code(graph, templates, n))
        .map(|c| c.files)
        .unwrap_or_default();

    // The same device in the shared-only merge, for provenance marking.
    let shared_device = shared_merged.and_then(|root| {
        root.children
            .iter()
            .find(|d| d.name == "d" && guid_key(d.attr("id").unwrap_or("")) == device_key)
    });

    let renderings = device
        .children
        .iter()
        .filter(|c| c.name == "r")
        .map(|r| resolve_rendering(graph, templates, page, r, shared_device))
        .collect();

    let notes = all_notes
        .iter()
        .filter(|n| {
            let dev = match n {
                DeltaNote::UnknownUid { device, .. }
                | DeltaNote::BadPositionRef { device, .. }
                | DeltaNote::DeviceWithoutLayout { device } => device,
            };
            guid_key(dev) == device_key
        })
        .cloned()
        .collect();

    ResolvedDevice {
        device_id: normalize_guidish(raw_id),
        layout,
        layout_code_files,
        renderings,
        notes,
    }
}

fn resolve_rendering(
    graph: &Graph,
    templates: &TemplateIndex,
    page: &ItemNode,
    r: &XmlEl,
    shared_device: Option<&XmlEl>,
) -> ResolvedRendering {
    let uid_raw = r.attr("uid");
    let uid = uid_raw.map(normalize_guidish);
    let rendering_guid = r.attr("id").and_then(|v| Guid::parse(v).ok());
    let rendering_id = match (rendering_guid, r.attr("id")) {
        (Some(g), _) => Some(g.rainbow()),
        (None, Some(v)) if !v.is_empty() => Some(v.to_string()),
        _ => None,
    };
    let rendering_node = rendering_guid.and_then(|g| graph.get(g));
    let placeholder = r.attr("ph").unwrap_or("").to_string();

    let source = if is_from_shared(r, uid_raw, shared_device) {
        "shared"
    } else {
        "final"
    };

    ResolvedRendering {
        uid,
        rendering_id,
        rendering_name: rendering_node.map(|n| n.meta.name.clone()),
        placeholder_leaf: placeholder_leaf(&placeholder),
        placeholder,
        datasource: resolve_datasource(graph, &page.meta.path, r.attr("ds").unwrap_or("")),
        parameters: parse_parameters(r.attr("par").unwrap_or("")),
        code_files: rendering_node
            .and_then(|n| rendering_code(graph, templates, n))
            .map(|c| c.files)
            .unwrap_or_default(),
        source: source.to_string(),
    }
}

/// A rendering is `"shared"` when the shared-only merge already contains
/// it: same uid, or (uid-less) an identical element.
fn is_from_shared(r: &XmlEl, uid: Option<&str>, shared_device: Option<&XmlEl>) -> bool {
    let Some(shared) = shared_device else {
        return false;
    };
    match uid {
        Some(uid) if !uid.is_empty() => {
            let key = guid_key(uid);
            shared
                .children
                .iter()
                .any(|c| c.name == "r" && guid_key(c.attr("uid").unwrap_or("")) == key)
        }
        _ => shared.children.iter().any(|c| c == r),
    }
}

/// GUID-ish attribute values normalize to rainbow form; anything else
/// passes through verbatim.
fn normalize_guidish(raw: &str) -> String {
    match Guid::parse(raw.trim()) {
        Ok(g) => g.rainbow(),
        Err(_) => raw.to_string(),
    }
}

// ---- datasource / placeholder / parameters ------------------------------------

/// Datasource resolution (DESIGN.md §6.3): empty → context item; GUID →
/// graph lookup; `local:X` → page path + `X`; `/sitecore/...` →
/// `find_path`; `scheme:rest` → dynamic; anything else → missing.
pub fn resolve_datasource(graph: &Graph, page_path: &str, raw: &str) -> DatasourceResolution {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DatasourceResolution::ContextItem;
    }
    if let Ok(g) = Guid::parse(trimmed) {
        return match graph.get(g) {
            Some(node) => DatasourceResolution::Item {
                raw: raw.to_string(),
                id: g,
                path: node.meta.path.clone(),
            },
            None => DatasourceResolution::Missing {
                raw: raw.to_string(),
            },
        };
    }
    if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("local:") {
        let rel = &trimmed[6..];
        let full = if rel.starts_with('/') {
            format!("{page_path}{rel}")
        } else {
            format!("{page_path}/{rel}")
        };
        return path_lookup(graph, raw, &full);
    }
    if trimmed.starts_with('/') {
        return path_lookup(graph, raw, trimmed);
    }
    if let Some(colon) = trimmed.find(':') {
        let scheme = &trimmed[..colon];
        if !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric()) {
            return DatasourceResolution::Dynamic {
                raw: raw.to_string(),
                scheme: scheme.to_string(),
            };
        }
    }
    DatasourceResolution::Missing {
        raw: raw.to_string(),
    }
}

fn path_lookup(graph: &Graph, raw: &str, path: &str) -> DatasourceResolution {
    match graph.find_path(path).first() {
        Some(&id) => DatasourceResolution::Item {
            raw: raw.to_string(),
            id,
            path: graph
                .get(id)
                .map(|n| n.meta.path.clone())
                .unwrap_or_default(),
        },
        None => DatasourceResolution::Missing {
            raw: raw.to_string(),
        },
    }
}

/// Segment after the last `/`, with a dynamic-placeholder suffix
/// (`-{36-hex-guid}-<digits>`, braces optional) stripped.
pub fn placeholder_leaf(placeholder: &str) -> String {
    let leaf = placeholder.rsplit('/').next().unwrap_or("");
    strip_dynamic_suffix(leaf).to_string()
}

fn strip_dynamic_suffix(leaf: &str) -> &str {
    let Some(dash) = leaf.rfind('-') else {
        return leaf;
    };
    let digits = &leaf[dash + 1..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return leaf;
    }
    let head = &leaf[..dash];
    for guid_len in [38usize, 36] {
        if head.len() <= guid_len + 1 {
            continue; // need at least `X-` before the guid
        }
        let (base, guid_part) = head.split_at(head.len() - guid_len);
        let Some(base) = base.strip_suffix('-') else {
            continue;
        };
        let inner = if guid_len == 38 {
            let Some(inner) = guid_part
                .strip_prefix('{')
                .and_then(|g| g.strip_suffix('}'))
            else {
                continue;
            };
            inner
        } else {
            guid_part
        };
        if is_hyphenated_guid(inner) {
            return base;
        }
    }
    leaf
}

/// `8-4-4-4-12` hex with hyphens, any case.
fn is_hyphenated_guid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        let is_dash = matches!(i, 8 | 13 | 18 | 23);
        if is_dash != (*b == b'-') || (!is_dash && !b.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// `par=` parsing: split `&`, then `=`, `%XX`-decode both sides. A
/// segment without `=` becomes a key with an empty value.
pub fn parse_parameters(par: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for segment in par.split('&') {
        if segment.is_empty() {
            continue;
        }
        let (k, v) = match segment.split_once('=') {
            Some((k, v)) => (k, v),
            None => (segment, ""),
        };
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- placeholderLeaf (dynamic suffix strip) ------------------------------

    #[test]
    fn placeholder_leaf_takes_last_segment() {
        assert_eq!(placeholder_leaf("main"), "main");
        assert_eq!(placeholder_leaf("/main/content"), "content");
        assert_eq!(placeholder_leaf("page-header/inner"), "inner");
        assert_eq!(placeholder_leaf(""), "");
    }

    #[test]
    fn placeholder_leaf_strips_dynamic_suffix() {
        // Braced guid form.
        assert_eq!(
            placeholder_leaf("main-{2caa6a9b-2c11-4c60-b3a0-a3a123456789}-0"),
            "main"
        );
        // Unbraced guid form.
        assert_eq!(
            placeholder_leaf("/page/col-2caa6a9b-2c11-4c60-b3a0-a3a123456789-12"),
            "col"
        );
    }

    #[test]
    fn placeholder_leaf_keeps_non_dynamic_dashes() {
        assert_eq!(placeholder_leaf("page-header"), "page-header");
        assert_eq!(placeholder_leaf("col-1"), "col-1");
        // Guid-like but not hex.
        assert_eq!(
            placeholder_leaf("main-{zzzz6a9b-2c11-4c60-b3a0-a3a123456789}-0"),
            "main-{zzzz6a9b-2c11-4c60-b3a0-a3a123456789}-0"
        );
        // Digits missing after the guid.
        assert_eq!(
            placeholder_leaf("main-{2caa6a9b-2c11-4c60-b3a0-a3a123456789}"),
            "main-{2caa6a9b-2c11-4c60-b3a0-a3a123456789}"
        );
    }

    // ---- parameters ------------------------------------------------------------

    #[test]
    fn parameters_split_and_percent_decode() {
        let p = parse_parameters("FieldNames=%7BAAA%7D&Styles=wide%20dark&Flag");
        assert_eq!(p.get("FieldNames").map(String::as_str), Some("{AAA}"));
        assert_eq!(p.get("Styles").map(String::as_str), Some("wide dark"));
        assert_eq!(p.get("Flag").map(String::as_str), Some(""));
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn parameters_empty_and_malformed_percent() {
        assert!(parse_parameters("").is_empty());
        let p = parse_parameters("a=100%25&b=%GG&c=%2");
        assert_eq!(p.get("a").map(String::as_str), Some("100%"));
        assert_eq!(p.get("b").map(String::as_str), Some("%GG"), "bad hex kept");
        assert_eq!(p.get("c").map(String::as_str), Some("%2"), "truncated kept");
    }

    // ---- datasource shapes (graph-free variants) --------------------------------

    #[test]
    fn dynamic_and_missing_datasources() {
        // A graph over an empty tempdir: no items, no paths.
        let dir = std::env::temp_dir().join("treesmith-presentation-empty-graph");
        std::fs::create_dir_all(&dir).unwrap();
        let graph = Graph::build(&dir);

        assert_eq!(
            resolve_datasource(&graph, "/sitecore/content/Home", ""),
            DatasourceResolution::ContextItem
        );
        assert_eq!(
            resolve_datasource(&graph, "/sitecore/content/Home", "   "),
            DatasourceResolution::ContextItem
        );
        assert_eq!(
            resolve_datasource(&graph, "/sitecore/content/Home", "query:./child"),
            DatasourceResolution::Dynamic {
                raw: "query:./child".to_string(),
                scheme: "query".to_string(),
            }
        );
        assert_eq!(
            resolve_datasource(&graph, "/sitecore/content/Home", "code:Some.Type"),
            DatasourceResolution::Dynamic {
                raw: "code:Some.Type".to_string(),
                scheme: "code".to_string(),
            }
        );
        // Unknown GUID, unknown path, and plain text are missing.
        assert_eq!(
            resolve_datasource(&graph, "/x", "{99999999-9999-4999-8999-999999999999}"),
            DatasourceResolution::Missing {
                raw: "{99999999-9999-4999-8999-999999999999}".to_string()
            }
        );
        assert_eq!(
            resolve_datasource(&graph, "/x", "/sitecore/content/Nope"),
            DatasourceResolution::Missing {
                raw: "/sitecore/content/Nope".to_string()
            }
        );
        assert_eq!(
            resolve_datasource(&graph, "/x", "just words"),
            DatasourceResolution::Missing {
                raw: "just words".to_string()
            }
        );
        // local: on an empty graph is missing too.
        assert_eq!(
            resolve_datasource(&graph, "/x", "local:Data"),
            DatasourceResolution::Missing {
                raw: "local:Data".to_string()
            }
        );
    }
}
