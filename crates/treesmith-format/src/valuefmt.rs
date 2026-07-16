//! Field formatter table (DESIGN.md §3.5).
//!
//! Rainbow field formatters change how values are *stored* in YAML vs the
//! raw platform value, and stamp `Type:` on the field so the transform is
//! reversible (VERIFY-P0):
//!
//! - Multilist family: stored one braced-uppercase GUID per line (block
//!   literal); raw form is `|`-separated.
//! - XML family (`Layout`, `Tracking`, `Rules`): stored as-is — Rainbow
//!   pretty-prints, we do not re-pretty-print on write (diff churn only,
//!   still valid).

use treesmith_types::Guid;

const MULTILIST_TYPES: &[&str] = &[
    "checklist",
    "multilist",
    "multilist with search",
    "treelist",
    "treelistex",
    "tree list",
];

const XML_TYPES: &[&str] = &["layout", "tracking", "rules"];

/// Whether `t` is a multilist-family field type (case-insensitive).
pub fn is_multilist_type(t: &str) -> bool {
    MULTILIST_TYPES.iter().any(|m| t.eq_ignore_ascii_case(m))
}

/// Whether `t` is an XML-family field type (case-insensitive).
pub fn is_xml_type(t: &str) -> bool {
    XML_TYPES.iter().any(|m| t.eq_ignore_ascii_case(m))
}

/// Normalizes a GUID-list value to Rainbow storage form: accepts `|` or
/// newline separated tokens in any GUID form; emits braced-uppercase,
/// newline-joined. `Err` names the first bad token.
pub fn normalize_guid_list(raw: &str) -> Result<String, String> {
    let (guids, invalid) = Guid::parse_list(raw);
    if let Some(bad) = invalid.first() {
        return Err(format!("invalid GUID token `{bad}`"));
    }
    Ok(guids
        .iter()
        .map(Guid::braced_upper)
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multilist_family_case_insensitive() {
        for t in [
            "Checklist",
            "Multilist",
            "Multilist with Search",
            "Treelist",
            "TreelistEx",
            "tree list",
            "TREELIST",
            "multilist WITH search",
        ] {
            assert!(is_multilist_type(t), "{t} should be multilist family");
        }
        for t in ["Droplink", "Single-Line Text", "Treelist Extended", ""] {
            assert!(!is_multilist_type(t), "{t} should not be multilist family");
        }
    }

    #[test]
    fn xml_family_case_insensitive() {
        for t in ["Layout", "Tracking", "Rules", "layout", "RULES"] {
            assert!(is_xml_type(t), "{t} should be xml family");
        }
        for t in ["Rich Text", "Layout Path", ""] {
            assert!(!is_xml_type(t), "{t} should not be xml family");
        }
    }

    #[test]
    fn normalize_guid_list_pipe_and_newline_input() {
        let raw = "c0ffee00-0001-4000-8000-000000000001|{C0FFEE00-0002-4000-8000-000000000002}\nC0FFEE0000034000800000000000000３";
        // note: full-width character makes the last token invalid
        assert!(normalize_guid_list(raw).is_err());

        let raw = "c0ffee00-0001-4000-8000-000000000001|{C0FFEE00-0002-4000-8000-000000000002}\nc0ffee00000340008000000000000003";
        assert_eq!(
            normalize_guid_list(raw).unwrap(),
            "{C0FFEE00-0001-4000-8000-000000000001}\n{C0FFEE00-0002-4000-8000-000000000002}\n{C0FFEE00-0003-4000-8000-000000000003}"
        );
    }

    #[test]
    fn normalize_guid_list_empty_and_whitespace() {
        assert_eq!(normalize_guid_list("").unwrap(), "");
        assert_eq!(normalize_guid_list("||\n").unwrap(), "");
    }

    #[test]
    fn normalize_guid_list_err_names_bad_token() {
        let err = normalize_guid_list("c0ffee00-0001-4000-8000-000000000001|not-a-guid|also-bad")
            .unwrap_err();
        assert!(err.contains("not-a-guid"), "err was: {err}");
    }
}
