//! Template inheritance resolution — the semantic core (spec §4,
//! DESIGN.md §5).
//!
//! Template items are graph items whose `Template` GUID equals
//! [`wellknown::TEMPLATE_TEMPLATE`]; sections are their children with
//! [`wellknown::TEMPLATE_SECTION`]; field definitions are section children
//! with [`wellknown::TEMPLATE_FIELD`].
//!
//! [`TemplateIndex::build`] extracts every [`TemplateDef`] from a
//! [`Graph`]; [`TemplateIndex::resolve`] walks base chains (DFS,
//! self-first, dedup keep-first, cycle-guarded) into an
//! [`EffectiveTemplate`] where the first definition of a field ID wins.
//! [`validate_value`] is the type-aware value check shared by the kernel's
//! write path (I3) and gate G6.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use treesmith_format::valuefmt::is_multilist_type;
use treesmith_format::ParsedItem;
use treesmith_graph::Graph;
use treesmith_types::{wellknown, Guid, SectionKind};

/// A template definition extracted from the graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TemplateDef {
    /// The template item's GUID.
    pub id: Guid,
    /// The template's name (last `Path` segment).
    pub name: String,
    /// The template item's full `Path`.
    pub path: String,
    /// Base template GUIDs in listed order (from `__Base template`).
    pub bases: Vec<Guid>,
    /// Own field definitions (sections then fields, each in child order).
    pub fields: Vec<FieldDef>,
    /// The standard-values item, from the `__Standard values` field, else a
    /// child named `__Standard Values`.
    pub standard_values: Option<Guid>,
}

/// One field definition on a template.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FieldDef {
    /// The field definition item's GUID.
    pub id: Guid,
    /// The field's name (last `Path` segment of the definition item).
    pub name: String,
    /// The field's `Type` value (empty when the definition omits it).
    pub field_type: String,
    /// Storage section: Shared if the `Shared` checkbox is `"1"`, else
    /// Unversioned if `Unversioned` is `"1"`, else Versioned.
    pub section: SectionKind,
    /// The name of the section the definition sits under.
    pub section_name: String,
}

/// A template with its base chain resolved into an effective field set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectiveTemplate {
    /// The requested template's GUID.
    pub id: Guid,
    /// The requested template's name.
    pub name: String,
    /// The full inheritance chain: DFS, self first, bases in listed order,
    /// dedup keep-first, cycle-guarded.
    pub chain: Vec<Guid>,
    /// Base GUIDs referenced anywhere in the chain but unknown to the
    /// index (dedup keep-first, discovery order).
    pub unresolved_bases: Vec<Guid>,
    /// Effective fields: chain walked in order, first definition of a
    /// field ID wins.
    pub fields: Vec<EffectiveField>,
}

/// One field in an effective field set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectiveField {
    /// The field definition item's GUID.
    pub id: Guid,
    /// The field's name.
    pub name: String,
    /// The field's `Type` value (empty when the definition omits it).
    pub field_type: String,
    /// Storage section kind.
    pub section: SectionKind,
    /// Name of the defining section.
    pub section_name: String,
    /// The chain template that contributed this definition.
    pub defined_by: Guid,
}

/// All template definitions extracted from one graph, with a name index.
#[derive(Clone, Debug, Default)]
pub struct TemplateIndex {
    defs: HashMap<Guid, TemplateDef>,
    /// lowercase name -> template ids, ordered by (path, id).
    by_name: BTreeMap<String, Vec<Guid>>,
}

impl TemplateIndex {
    /// Extracts every template definition from the graph.
    ///
    /// Deterministic: templates are visited in the graph's canonical
    /// (path, id) order; sections and fields in child order (name, id).
    pub fn build(graph: &Graph) -> TemplateIndex {
        let mut index = TemplateIndex::default();
        for template_id in graph.by_template(wellknown::TEMPLATE_TEMPLATE) {
            let Some(node) = graph.get(template_id) else {
                continue;
            };
            let def = extract_template(graph, node.id, &node.item, &node.meta.path);
            index
                .by_name
                .entry(def.name.to_lowercase())
                .or_default()
                .push(def.id);
            index.defs.insert(def.id, def);
        }
        index
    }

    /// The raw definition for a template id, if known.
    pub fn get(&self, id: Guid) -> Option<&TemplateDef> {
        self.defs.get(&id)
    }

    /// Template ids whose name matches case-insensitively, ordered by
    /// (path, id).
    pub fn find_by_name(&self, name: &str) -> Vec<Guid> {
        self.by_name
            .get(&name.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// Resolves the base chain into an effective template. `None` if `id`
    /// is not a known template.
    pub fn resolve(&self, id: Guid) -> Option<EffectiveTemplate> {
        let root = self.defs.get(&id)?;
        let mut chain = Vec::new();
        let mut unresolved = Vec::new();
        let mut visited = HashSet::new();
        self.walk_chain(id, &mut chain, &mut unresolved, &mut visited);

        let mut fields = Vec::new();
        let mut seen_fields = HashSet::new();
        for template in &chain {
            let def = &self.defs[template];
            for f in &def.fields {
                if seen_fields.insert(f.id) {
                    fields.push(EffectiveField {
                        id: f.id,
                        name: f.name.clone(),
                        field_type: f.field_type.clone(),
                        section: f.section,
                        section_name: f.section_name.clone(),
                        defined_by: def.id,
                    });
                }
            }
        }
        Some(EffectiveTemplate {
            id,
            name: root.name.clone(),
            chain,
            unresolved_bases: unresolved,
            fields,
        })
    }

    /// Standard-values item ids along the chain, derived-first (self, then
    /// bases in chain order). Empty if `template` is unknown.
    pub fn std_values_chain(&self, template: Guid) -> Vec<Guid> {
        let Some(effective) = self.resolve(template) else {
            return Vec::new();
        };
        effective
            .chain
            .iter()
            .filter_map(|t| self.defs[t].standard_values)
            .collect()
    }

    /// DFS, self first, bases in listed order. The `visited` set is both
    /// the dedup (keep-first) and the cycle guard; unknown bases land in
    /// `unresolved` (also dedup keep-first).
    fn walk_chain(
        &self,
        id: Guid,
        chain: &mut Vec<Guid>,
        unresolved: &mut Vec<Guid>,
        visited: &mut HashSet<Guid>,
    ) {
        if !visited.insert(id) {
            return;
        }
        chain.push(id);
        let bases = self.defs[&id].bases.clone();
        for base in bases {
            if self.defs.contains_key(&base) {
                self.walk_chain(base, chain, unresolved, visited);
            } else if visited.insert(base) {
                unresolved.push(base);
            }
        }
    }
}

impl EffectiveTemplate {
    /// The effective field with this definition GUID.
    pub fn field_by_id(&self, id: Guid) -> Option<&EffectiveField> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// The first effective field (chain order) whose name matches
    /// case-insensitively.
    pub fn field_by_name(&self, name: &str) -> Option<&EffectiveField> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }
}

// ---- extraction -------------------------------------------------------------

fn extract_template(graph: &Graph, id: Guid, item: &ParsedItem, path: &str) -> TemplateDef {
    let bases = match definition_value(item, wellknown::BASE_TEMPLATE_FIELD) {
        Some(raw) => Guid::parse_list(&raw).0,
        None => Vec::new(),
    };

    let mut standard_values = definition_value(item, wellknown::STANDARD_VALUES_FIELD)
        .and_then(|raw| Guid::parse(&raw).ok());

    let mut fields = Vec::new();
    for section_id in graph.children(id) {
        let Some(section) = graph.get(section_id) else {
            continue;
        };
        if standard_values.is_none() && section.meta.name.eq_ignore_ascii_case("__Standard Values")
        {
            standard_values = Some(section.id);
        }
        if section.meta.template != Some(wellknown::TEMPLATE_SECTION) {
            continue;
        }
        for field_id in graph.children(section_id) {
            let Some(field) = graph.get(field_id) else {
                continue;
            };
            if field.meta.template != Some(wellknown::TEMPLATE_FIELD) {
                continue;
            }
            fields.push(FieldDef {
                id: field.id,
                name: field.meta.name.clone(),
                field_type: definition_value(&field.item, wellknown::FIELD_TYPE_FIELD)
                    .unwrap_or_default(),
                section: section_kind_of(&field.item),
                section_name: section.meta.name.clone(),
            });
        }
    }

    TemplateDef {
        id,
        name: last_segment(path),
        path: path.to_string(),
        bases,
        fields,
        standard_values,
    }
}

fn last_segment(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").to_string()
}

/// Reads a field value off a definition item with the deterministic
/// precedence of DESIGN.md §5: shared → unversioned (languages
/// alphabetical) → versioned (languages alphabetical, highest version).
fn definition_value(item: &ParsedItem, field: Guid) -> Option<String> {
    if let Some(f) = item.shared_fields().into_iter().find(|f| f.id == field) {
        return Some(f.value);
    }
    let mut languages = item.languages();
    languages.sort_by(|a, b| a.language.cmp(&b.language));
    for lang in &languages {
        if let Some(f) = lang.unversioned.iter().find(|f| f.id == field) {
            return Some(f.value.clone());
        }
    }
    for lang in &languages {
        let Some((_, fields)) = lang.versions.iter().max_by_key(|(n, _)| *n) else {
            continue;
        };
        if let Some(f) = fields.iter().find(|f| f.id == field) {
            return Some(f.value.clone());
        }
    }
    None
}

/// Section kind from the definition's checkboxes: Shared if `Shared` is
/// `"1"`, else Unversioned if `Unversioned` is `"1"`, else Versioned.
fn section_kind_of(item: &ParsedItem) -> SectionKind {
    if definition_value(item, wellknown::FIELD_SHARED_FIELD).as_deref() == Some("1") {
        SectionKind::Shared
    } else if definition_value(item, wellknown::FIELD_UNVERSIONED_FIELD).as_deref() == Some("1") {
        SectionKind::Unversioned
    } else {
        SectionKind::Versioned
    }
}

// ---- value validation --------------------------------------------------------

/// Type-aware value validation shared by the kernel's write path (I3) and
/// gate G6 (DESIGN.md §5). Type names match case-insensitively. Types
/// outside the table are accepted (passthrough).
///
/// - `Checkbox` → `"1"` or `""`.
/// - `Integer` / `Number` → empty, or optional `-` followed by digits.
/// - `Date` / `Datetime` → empty, or `\d{8}T\d{6}Z?`.
/// - Multilist family → every token a GUID.
/// - `Droplink` / `Droptree` / `Reference` / `Grouped Droplink` → empty or
///   a single GUID.
pub fn validate_value(field_type: &str, value: &str) -> Result<(), String> {
    if field_type.eq_ignore_ascii_case("Checkbox") {
        return match value {
            "1" | "" => Ok(()),
            other => Err(format!(
                "checkbox value must be \"1\" or empty, got `{other}`"
            )),
        };
    }
    if field_type.eq_ignore_ascii_case("Integer") || field_type.eq_ignore_ascii_case("Number") {
        let digits = value.strip_prefix('-').unwrap_or(value);
        if value.is_empty() || (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())) {
            return Ok(());
        }
        return Err(format!(
            "{} value must be empty or an optionally-signed integer, got `{value}`",
            field_type.to_ascii_lowercase()
        ));
    }
    if field_type.eq_ignore_ascii_case("Date") || field_type.eq_ignore_ascii_case("Datetime") {
        if value.is_empty() || is_iso_stamp(value) {
            return Ok(());
        }
        return Err(format!(
            "date value must be empty or `yyyymmddThhmmss[Z]`, got `{value}`"
        ));
    }
    if is_multilist_type(field_type) {
        let (_, invalid) = Guid::parse_list(value);
        if let Some(bad) = invalid.first() {
            return Err(format!("multilist token is not a GUID: `{bad}`"));
        }
        return Ok(());
    }
    if is_single_reference_type(field_type) {
        if value.is_empty() || Guid::parse(value).is_ok() {
            return Ok(());
        }
        return Err(format!(
            "reference value must be empty or a single GUID, got `{value}`"
        ));
    }
    Ok(())
}

/// `\d{8}T\d{6}Z?` — the Sitecore ISO field stamp, no regex dependency.
fn is_iso_stamp(v: &str) -> bool {
    let bytes = v.as_bytes();
    let ok_len = match bytes.len() {
        15 => true,
        16 => bytes[15] == b'Z',
        _ => false,
    };
    ok_len
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
}

fn is_single_reference_type(t: &str) -> bool {
    ["Droplink", "Droptree", "Reference", "Grouped Droplink"]
        .iter()
        .any(|k| k.eq_ignore_ascii_case(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- validate_value table (DESIGN.md §5) --------------------------------

    #[test]
    fn checkbox_accepts_one_and_empty() {
        assert!(validate_value("Checkbox", "1").is_ok());
        assert!(validate_value("Checkbox", "").is_ok());
        assert!(validate_value("checkbox", "1").is_ok(), "case-insensitive");
        assert!(validate_value("Checkbox", "0").is_err());
        assert!(validate_value("Checkbox", "true").is_err());
    }

    #[test]
    fn integer_and_number_accept_signed_digits_or_empty() {
        for t in ["Integer", "Number", "integer", "NUMBER"] {
            assert!(validate_value(t, "").is_ok());
            assert!(validate_value(t, "0").is_ok());
            assert!(validate_value(t, "42").is_ok());
            assert!(validate_value(t, "-7").is_ok());
            assert!(validate_value(t, "-").is_err());
            assert!(validate_value(t, "4.2").is_err());
            assert!(validate_value(t, "42x").is_err());
            assert!(validate_value(t, " 42").is_err());
            assert!(validate_value(t, "--1").is_err());
        }
    }

    #[test]
    fn datetime_accepts_iso_stamp_or_empty() {
        for t in ["Date", "Datetime", "datetime"] {
            assert!(validate_value(t, "").is_ok());
            assert!(validate_value(t, "20260715T120000").is_ok());
            assert!(validate_value(t, "20260715T120000Z").is_ok());
            assert!(validate_value(t, "20260715").is_err());
            assert!(validate_value(t, "20260715T1200").is_err());
            assert!(validate_value(t, "20260715T120000ZZ").is_err());
            assert!(validate_value(t, "2026-07-15T12:00:00Z").is_err());
            assert!(validate_value(t, "yyyymmddThhmmss").is_err());
        }
    }

    #[test]
    fn multilist_family_requires_guid_tokens() {
        const A: &str = "7c1e1c2a-0001-4000-8000-000000000001";
        const B: &str = "{7C1E1C2A-0010-4000-8000-000000000010}";
        for t in [
            "Multilist",
            "Checklist",
            "Treelist",
            "TreelistEx",
            "Multilist with Search",
        ] {
            assert!(validate_value(t, "").is_ok());
            assert!(validate_value(t, A).is_ok());
            assert!(validate_value(t, &format!("{A}|{B}")).is_ok());
            assert!(validate_value(t, &format!("{A}\n{B}")).is_ok());
            let err = validate_value(t, &format!("{A}|nope")).unwrap_err();
            assert!(err.contains("nope"), "error names the bad token: {err}");
        }
    }

    #[test]
    fn single_reference_types_accept_one_guid_or_empty() {
        const A: &str = "7c1e1c2a-0001-4000-8000-000000000001";
        for t in [
            "Droplink",
            "Droptree",
            "Reference",
            "Grouped Droplink",
            "droplink",
        ] {
            assert!(validate_value(t, "").is_ok());
            assert!(validate_value(t, A).is_ok());
            assert!(validate_value(t, "{7C1E1C2A-0001-4000-8000-000000000001}").is_ok());
            assert!(validate_value(t, "not-a-guid").is_err());
            assert!(
                validate_value(t, &format!("{A}|{A}")).is_err(),
                "two guids are not a single reference"
            );
        }
    }

    #[test]
    fn unknown_types_pass_through() {
        assert!(validate_value("Single-Line Text", "anything at all").is_ok());
        assert!(validate_value("Rich Text", "<p>html</p>").is_ok());
        assert!(validate_value("", "raw").is_ok());
        assert!(validate_value("General Link", "<link ...>").is_ok());
    }

    // ---- iso stamp helper ----------------------------------------------------

    #[test]
    fn iso_stamp_edges() {
        assert!(is_iso_stamp("00000101T000000"));
        assert!(is_iso_stamp("99991231T235959Z"));
        assert!(!is_iso_stamp("0000010T0000000")); // T misplaced
        assert!(!is_iso_stamp(""));
    }
}
