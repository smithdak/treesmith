//! The schema-aware write path (spec §5, DESIGN.md §8 steps 1–4):
//! template-resolved field ids, section-derived slots, value validation +
//! normalization, and the emit → re-parse → re-emit self-check that runs
//! before any byte reaches disk.

use serde_json::json;
use treesmith_format::valuefmt::{is_multilist_type, is_xml_type, normalize_guid_list};
use treesmith_format::{FieldSlot, ParsedItem, SerializationFormat};
use treesmith_presentation::parse_xml;
use treesmith_template::{validate_value, EffectiveTemplate};
use treesmith_types::{wellknown, Guid, SectionKind};

use crate::error::{validation, KernelError};

/// The field a write resolved to: id, name, type, and defining section.
#[derive(Clone, Debug)]
pub(crate) struct FieldTarget {
    pub id: Guid,
    pub name: String,
    pub field_type: String,
    pub section: SectionKind,
}

/// Well-known system fields writable without a serialized definition
/// (real repos rarely serialize the standard template). Names are the
/// platform's canonical `__` names; sections per platform semantics.
const WELLKNOWN_WRITABLE: &[(Guid, &str, &str, SectionKind)] = &[
    (
        wellknown::LAYOUT_FIELD,
        "__Renderings",
        "Layout",
        SectionKind::Shared,
    ),
    (
        wellknown::FINAL_LAYOUT_FIELD,
        "__Final Renderings",
        "Layout",
        SectionKind::Versioned,
    ),
    (
        wellknown::DISPLAY_NAME_FIELD,
        "__Display name",
        "text",
        SectionKind::Unversioned,
    ),
    (
        wellknown::SORTORDER_FIELD,
        "__Sortorder",
        "text",
        SectionKind::Shared,
    ),
    (
        wellknown::CREATED_FIELD,
        "__Created",
        "Datetime",
        SectionKind::Versioned,
    ),
    (
        wellknown::CREATED_BY_FIELD,
        "__Created by",
        "text",
        SectionKind::Versioned,
    ),
    (
        wellknown::BASE_TEMPLATE_FIELD,
        "__Base template",
        "tree list",
        SectionKind::Shared,
    ),
    (
        wellknown::STANDARD_VALUES_FIELD,
        "__Standard values",
        "Reference",
        SectionKind::Shared,
    ),
];

/// Step 1: resolve the field through the effective template — field ids
/// are never guessed from names. A GUID designator must exist in the
/// effective template unless it is a well-known system field; a name goes
/// through `field_by_name` (then the fixed well-known name table).
pub(crate) fn resolve_field(
    effective: Option<&EffectiveTemplate>,
    designator: &str,
) -> Result<FieldTarget, KernelError> {
    if let Ok(id) = Guid::parse(designator) {
        if let Some(f) = effective.and_then(|e| e.field_by_id(id)) {
            return Ok(FieldTarget {
                id: f.id,
                name: f.name.clone(),
                field_type: f.field_type.clone(),
                section: f.section,
            });
        }
        if let Some(t) = wellknown_target(|(g, _, _, _)| *g == id) {
            return Ok(t);
        }
        return Err(unknown_field(&id.rainbow(), effective));
    }
    if let Some(f) = effective.and_then(|e| e.field_by_name(designator)) {
        return Ok(FieldTarget {
            id: f.id,
            name: f.name.clone(),
            field_type: f.field_type.clone(),
            section: f.section,
        });
    }
    if let Some(t) = wellknown_target(|(_, name, _, _)| name.eq_ignore_ascii_case(designator)) {
        return Ok(t);
    }
    Err(unknown_field(designator, effective))
}

/// Builds an `unknown-field` error that lists the effective template's
/// available field names (and a nearest match when one is close), so a
/// consuming agent doing a blind write gets actionable feedback instead of
/// a bare rejection.
fn unknown_field(designator: &str, effective: Option<&EffectiveTemplate>) -> KernelError {
    let mut available: Vec<String> = effective
        .map(|e| e.fields.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    available.sort_by_key(|n| n.to_ascii_lowercase());
    available.dedup();

    let did_you_mean = nearest_field(designator, &available);
    let hint = match &did_you_mean {
        Some(name) => format!(" — did you mean `{name}`?"),
        None => String::new(),
    };
    validation(
        "unknown-field",
        format!("field `{designator}` is not in the item's effective template{hint}"),
        json!({
            "field": designator,
            "available": available,
            "didYouMean": did_you_mean,
        }),
    )
}

/// Nearest available field name to `needle` by Levenshtein distance,
/// accepting only reasonably close matches (distance ≤ ⌈len/2⌉, min 1).
fn nearest_field(needle: &str, available: &[String]) -> Option<String> {
    let n = needle.to_ascii_lowercase();
    let limit = (needle.chars().count().div_ceil(2)).max(1);
    available
        .iter()
        .map(|cand| (levenshtein(&n, &cand.to_ascii_lowercase()), cand))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, cand)| cand.clone())
}

/// Iterative Levenshtein edit distance (small strings; no deps).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn wellknown_target(
    pred: impl Fn(&(Guid, &str, &str, SectionKind)) -> bool,
) -> Option<FieldTarget> {
    WELLKNOWN_WRITABLE
        .iter()
        .find(|row| pred(row))
        .map(|(id, name, ty, section)| FieldTarget {
            id: *id,
            name: name.to_string(),
            field_type: ty.to_string(),
            section: *section,
        })
}

/// Step 2: the slot comes from the field *definition's* section kind,
/// never from where a value currently sits.
///
/// Shared rejects `--language`/`--version`; Unversioned takes a language
/// (default `en`) and rejects `--version`; Versioned takes language +
/// version (default: max existing; none and `create_version` → 1).
pub(crate) fn slot_for(
    target: &FieldTarget,
    item: &ParsedItem,
    language: Option<&str>,
    version: Option<u32>,
    create_version: bool,
) -> Result<FieldSlot, KernelError> {
    match target.section {
        SectionKind::Shared => {
            if language.is_some() || version.is_some() {
                return Err(validation(
                    "wrong-slot-for-section",
                    format!(
                        "field `{}` is shared; --language/--version do not apply",
                        target.name
                    ),
                    json!({ "field": target.id.rainbow(), "section": "shared" }),
                ));
            }
            Ok(FieldSlot::Shared)
        }
        SectionKind::Unversioned => {
            if version.is_some() {
                return Err(validation(
                    "wrong-slot-for-section",
                    format!(
                        "field `{}` is unversioned; --version does not apply",
                        target.name
                    ),
                    json!({ "field": target.id.rainbow(), "section": "unversioned" }),
                ));
            }
            Ok(FieldSlot::Unversioned {
                language: language.unwrap_or("en").to_string(),
            })
        }
        SectionKind::Versioned => {
            let language = language.unwrap_or("en").to_string();
            let version = match version.or_else(|| item.max_version(&language)) {
                Some(v) => v,
                None if create_version => 1,
                None => {
                    return Err(validation(
                        "no-version",
                        format!(
                        "language `{language}` has no versions and --no-create-version was given"
                    ),
                        json!({ "field": target.id.rainbow(), "language": language }),
                    ))
                }
            };
            Ok(FieldSlot::Versioned { language, version })
        }
    }
}

/// Step 3: validate + normalize the value for the resolved field type.
/// Returns `(stored value, Type: hint to stamp)`.
///
/// - newlines are normalized to `\n` (the emitter rejects `\r` in values);
/// - multilist values are normalized to braced-upper newline-joined GUIDs;
/// - `validate_value` runs on the normalized value;
/// - XML-family (layout) values must `parse_xml` cleanly;
/// - formatter-covered types (multilist + XML families) get `Type:` stamped.
pub(crate) fn prepare_value(
    target: &FieldTarget,
    raw: &str,
) -> Result<(String, Option<String>), KernelError> {
    let mut value = raw.replace("\r\n", "\n").replace('\r', "\n");

    if is_multilist_type(&target.field_type) {
        value = normalize_guid_list(&value).map_err(|e| {
            validation(
                "invalid-value",
                format!("field `{}`: {e}", target.name),
                json!({ "field": target.id.rainbow(), "type": target.field_type }),
            )
        })?;
    }

    if let Err(e) = validate_value(&target.field_type, &value) {
        return Err(validation(
            "invalid-value",
            format!("field `{}`: {e}", target.name),
            json!({ "field": target.id.rainbow(), "type": target.field_type }),
        ));
    }

    if is_xml_type(&target.field_type) && !value.trim().is_empty() {
        if let Err(e) = parse_xml(&value) {
            return Err(validation(
                "malformed-layout-xml",
                format!(
                    "field `{}`: {} at offset {}",
                    target.name, e.message, e.offset
                ),
                json!({
                    "field": target.id.rainbow(),
                    "error": e.message,
                    "offset": e.offset,
                }),
            ));
        }
    }

    // A trailing newline encodes as a block scalar with a trailing blank line,
    // which the codec can only round-trip when the field happens to sit at end
    // of document. Rejecting uniformly keeps identical mutations behaving
    // identically regardless of document position (review finding W1).
    if value.ends_with('\n') {
        return Err(validation(
            "trailing-newline-unsupported",
            format!(
                "field `{}`: value ends with a newline, which cannot be stored \
                 position-independently in the serialized form — trim it",
                target.name
            ),
            json!({ "field": target.id.rainbow(), "type": target.field_type }),
        ));
    }

    let stamp = (is_multilist_type(&target.field_type) || is_xml_type(&target.field_type))
        && !target.field_type.is_empty();
    let type_hint = stamp.then(|| target.field_type.clone());
    Ok((value, type_hint))
}

/// Step 4: emit candidate bytes → re-parse → re-emit must be byte-equal.
/// Runs before any disk write; failure means nothing is written.
pub(crate) fn self_check(
    fmt: &dyn SerializationFormat,
    item: &ParsedItem,
) -> Result<Vec<u8>, KernelError> {
    let bytes = fmt.emit(item);
    let reparsed = fmt.parse(&bytes).map_err(|f| {
        validation(
            "self-check-failed",
            format!(
                "emitted bytes failed to re-parse: line {}: {}",
                f.line, f.message
            ),
            json!({ "line": f.line, "kind": f.kind.as_str() }),
        )
    })?;
    if fmt.emit(&reparsed) != bytes {
        return Err(validation(
            "self-check-failed",
            "emit → re-parse → re-emit was not byte-identical",
            serde_json::Value::Null,
        ));
    }
    Ok(bytes)
}

/// Reads the value of `field` in `slot` off an item (the set-field
/// read-back check).
pub(crate) fn read_slot_value(item: &ParsedItem, slot: &FieldSlot, field: Guid) -> Option<String> {
    let fields = match slot {
        FieldSlot::Shared => item.shared_fields(),
        FieldSlot::Unversioned { language } => item
            .languages()
            .into_iter()
            .find(|l| &l.language == language)
            .map(|l| l.unversioned)
            .unwrap_or_default(),
        FieldSlot::Versioned { language, version } => item
            .languages()
            .into_iter()
            .find(|l| &l.language == language)
            .and_then(|l| l.versions.into_iter().find(|(v, _)| v == version))
            .map(|(_, fs)| fs)
            .unwrap_or_default(),
    };
    fields.into_iter().find(|f| f.id == field).map(|f| f.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("title", "title"), 0);
        assert_eq!(levenshtein("titel", "title"), 2);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn nearest_field_suggests_close_match_only() {
        let available = vec![
            "Body".to_string(),
            "NavTitle".to_string(),
            "Title".to_string(),
        ];
        // One transposition away from Title → suggested.
        assert_eq!(
            nearest_field("Titel", &available),
            Some("Title".to_string())
        );
        // Far from everything → no suggestion.
        assert_eq!(nearest_field("NoSuchField", &available), None);
        // Case-insensitive.
        assert_eq!(
            nearest_field("title", &available),
            Some("Title".to_string())
        );
    }

    #[test]
    fn unknown_field_error_carries_available_list() {
        let err = unknown_field("Bogus", None);
        match err {
            KernelError::Validation { code, details, .. } => {
                assert_eq!(code, "unknown-field");
                assert!(details.get("available").is_some());
                assert_eq!(details.get("didYouMean"), Some(&json!(null)));
            }
            other => panic!("expected validation, got {other:?}"),
        }
    }
}
