//! Core vocabulary for treesmith: GUIDs, section kinds, and well-known
//! platform identifiers. Depends on nothing internal (spec §2 rule 4).
//!
//! This crate is the bottom of the dependency stack — every other treesmith
//! crate builds on these types — so the public API follows DESIGN.md §2 and
//! §12 exactly.

pub mod wellknown;

use std::fmt;

/// An item / field / template identifier.
///
/// Wraps a UUID and normalizes the platform's textual forms: hyphenated
/// (`ab86861a-6030-46c5-b394-e8f99e8b87db`), braced
/// (`{AB86861A-6030-46C5-B394-E8F99E8B87DB}`), and simple 32-hex-digit —
/// any case on input.
///
/// Ordering derives from the underlying UUID's byte order, which is
/// identical to lexicographic order of the [`Guid::rainbow`] string (both
/// compare the same hex digits in the same sequence).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Guid(uuid::Uuid);

/// Error returned by [`Guid::parse`] when the input is not a recognizable
/// GUID form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid GUID `{input}`: expected hyphenated, braced {{...}}, or 32-hex-digit form")]
pub struct GuidError {
    /// The rejected input, verbatim (untrimmed).
    pub input: String,
}

impl Guid {
    /// Parses a GUID. Accepts hyphenated, braced `{...}`, and 32-hex-digit
    /// forms, any case. Leading/trailing whitespace is tolerated.
    pub fn parse(s: &str) -> Result<Guid, GuidError> {
        uuid::Uuid::try_parse(s.trim())
            .map(Guid)
            .map_err(|_| GuidError {
                input: s.to_string(),
            })
    }

    /// Rainbow file form: lowercase hyphenated, no braces. Also the
    /// [`fmt::Display`] impl and the serde serialization form.
    pub fn rainbow(&self) -> String {
        self.0.hyphenated().to_string()
    }

    /// Sitecore field-value form: `{UPPERCASE-HYPHENATED}`.
    pub fn braced_upper(&self) -> String {
        let mut buf = uuid::Uuid::encode_buffer();
        let hex = self.0.hyphenated().encode_upper(&mut buf);
        format!("{{{hex}}}")
    }

    /// A fresh random (UUID v4) GUID.
    ///
    /// Used only by `forge` — parse/resolve/gate paths must stay
    /// deterministic (DESIGN.md §1, spec I5).
    pub fn new_random() -> Guid {
        Guid(uuid::Uuid::new_v4())
    }

    /// Splits a raw field value on `|`, `\r`, `\n`; trims each token; skips
    /// empties. Returns `(parsed guids, invalid tokens)` — invalid tokens
    /// are reported trimmed, in input order.
    pub fn parse_list(s: &str) -> (Vec<Guid>, Vec<String>) {
        let mut guids = Vec::new();
        let mut invalid = Vec::new();
        for token in s.split(['|', '\r', '\n']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match Guid::parse(token) {
                Ok(g) => guids.push(g),
                Err(_) => invalid.push(token.to_string()),
            }
        }
        (guids, invalid)
    }
}

impl fmt::Display for Guid {
    /// Rainbow form: lowercase hyphenated, no braces.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

impl serde::Serialize for Guid {
    /// Serializes as the [`Guid::rainbow`] string.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buf = uuid::Uuid::encode_buffer();
        serializer.serialize_str(self.0.hyphenated().encode_lower(&mut buf))
    }
}

impl<'de> serde::Deserialize<'de> for Guid {
    /// Deserializes via [`Guid::parse`] (any accepted form).
    fn deserialize<D>(deserializer: D) -> Result<Guid, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct GuidVisitor;

        impl serde::de::Visitor<'_> for GuidVisitor {
            type Value = Guid;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a GUID string (hyphenated, braced {...}, or 32 hex digits)")
            }

            fn visit_str<E>(self, v: &str) -> Result<Guid, E>
            where
                E: serde::de::Error,
            {
                Guid::parse(v).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(GuidVisitor)
    }
}

/// Which storage section of an item a field lives in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum SectionKind {
    /// One value for the whole item, across all languages and versions.
    Shared,
    /// One value per language.
    Unversioned,
    /// One value per language + numbered version.
    Versioned,
}

impl SectionKind {
    /// Stable lowercase name: `"shared" | "unversioned" | "versioned"`.
    /// Also the serde serialization form.
    pub fn as_str(&self) -> &'static str {
        match self {
            SectionKind::Shared => "shared",
            SectionKind::Unversioned => "unversioned",
            SectionKind::Versioned => "versioned",
        }
    }
}

impl fmt::Display for SectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for SectionKind {
    /// Serializes as the [`SectionKind::as_str`] string.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for SectionKind {
    /// Deserializes from the [`SectionKind::as_str`] string
    /// (case-insensitive, tolerant-read posture).
    fn deserialize<D>(deserializer: D) -> Result<SectionKind, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SectionKindVisitor;

        impl serde::de::Visitor<'_> for SectionKindVisitor {
            type Value = SectionKind;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#""shared", "unversioned", or "versioned""#)
            }

            fn visit_str<E>(self, v: &str) -> Result<SectionKind, E>
            where
                E: serde::de::Error,
            {
                if v.eq_ignore_ascii_case("shared") {
                    Ok(SectionKind::Shared)
                } else if v.eq_ignore_ascii_case("unversioned") {
                    Ok(SectionKind::Unversioned)
                } else if v.eq_ignore_ascii_case("versioned") {
                    Ok(SectionKind::Versioned)
                } else {
                    Err(serde::de::Error::unknown_variant(
                        v,
                        &["shared", "unversioned", "versioned"],
                    ))
                }
            }
        }

        deserializer.deserialize_str(SectionKindVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HYPHENATED: &str = "ab86861a-6030-46c5-b394-e8f99e8b87db";

    fn reference() -> Guid {
        Guid::parse(HYPHENATED).expect("reference GUID parses")
    }

    // ---- parse: every accepted form -------------------------------------

    #[test]
    fn parse_hyphenated_lowercase() {
        assert_eq!(reference().rainbow(), HYPHENATED);
    }

    #[test]
    fn parse_hyphenated_uppercase() {
        let g = Guid::parse("AB86861A-6030-46C5-B394-E8F99E8B87DB").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_hyphenated_mixed_case() {
        let g = Guid::parse("Ab86861a-6030-46C5-b394-E8F99e8b87dB").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_braced_upper() {
        let g = Guid::parse("{AB86861A-6030-46C5-B394-E8F99E8B87DB}").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_braced_lower() {
        let g = Guid::parse("{ab86861a-6030-46c5-b394-e8f99e8b87db}").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_simple_lower() {
        let g = Guid::parse("ab86861a603046c5b394e8f99e8b87db").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_simple_upper() {
        let g = Guid::parse("AB86861A603046C5B394E8F99E8B87DB").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_trims_whitespace() {
        let g = Guid::parse("  {AB86861A-6030-46C5-B394-E8F99E8B87DB}\t").unwrap();
        assert_eq!(g, reference());
    }

    #[test]
    fn parse_rejects_invalid() {
        for bad in [
            "",
            "   ",
            "not-a-guid",
            "ab86861a-6030-46c5-b394",                // too short
            "ab86861a-6030-46c5-b394-e8f99e8b87dbff", // too long
            "zz86861a-6030-46c5-b394-e8f99e8b87db",   // non-hex
            "{ab86861a-6030-46c5-b394-e8f99e8b87db",  // unclosed brace
            "ab86861a-6030-46c5-b394-e8f99e8b87db}",  // unopened brace
            "ab86861a_6030_46c5_b394_e8f99e8b87db",   // wrong separator
            "ab86861a-603046c5-b394-e8f99e8b87db",    // misplaced hyphens
        ] {
            let err = Guid::parse(bad).unwrap_err();
            assert_eq!(err.input, bad, "error preserves the original input");
        }
    }

    #[test]
    fn guid_error_display_names_input() {
        let err = Guid::parse("nope").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope"), "message was: {msg}");
        assert!(msg.contains("invalid GUID"), "message was: {msg}");
    }

    // ---- output forms round-trip -----------------------------------------

    #[test]
    fn rainbow_round_trip() {
        let g = reference();
        assert_eq!(g.rainbow(), HYPHENATED);
        assert_eq!(Guid::parse(&g.rainbow()).unwrap(), g);
    }

    #[test]
    fn braced_upper_round_trip() {
        let g = reference();
        assert_eq!(g.braced_upper(), "{AB86861A-6030-46C5-B394-E8F99E8B87DB}");
        assert_eq!(Guid::parse(&g.braced_upper()).unwrap(), g);
    }

    #[test]
    fn display_is_rainbow() {
        let g = reference();
        assert_eq!(format!("{g}"), g.rainbow());
        assert_eq!(g.to_string(), HYPHENATED);
    }

    #[test]
    fn new_random_is_v4_and_round_trips() {
        let a = Guid::new_random();
        let b = Guid::new_random();
        assert_ne!(a, b, "two random GUIDs should differ");
        assert_eq!(a.0.get_version_num(), 4);
        assert_eq!(Guid::parse(&a.rainbow()).unwrap(), a);
        assert_eq!(Guid::parse(&a.braced_upper()).unwrap(), a);
    }

    #[test]
    fn ord_matches_rainbow_string_order() {
        let mut guids = [
            Guid::parse("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap(),
            Guid::parse("00000000-0000-4000-8000-000000000001").unwrap(),
            reference(),
            Guid::parse("7c1e1c2a-0001-4000-8000-000000000001").unwrap(),
            Guid::parse("7c1e1c2a-0000-4000-8000-000000000000").unwrap(),
        ];
        let mut strings: Vec<String> = guids.iter().map(Guid::rainbow).collect();
        guids.sort();
        strings.sort();
        let sorted_strings: Vec<String> = guids.iter().map(Guid::rainbow).collect();
        assert_eq!(sorted_strings, strings);
    }

    // ---- parse_list -------------------------------------------------------

    #[test]
    fn parse_list_mixed_separators_and_invalid_tokens() {
        let raw = "{AB86861A-6030-46C5-B394-E8F99E8B87DB}|7c1e1c2a-0001-4000-8000-000000000001\r\n  e269fbb5-3750-427a-9149-7aa950b49301  \nnot-a-guid|455a3e98a62740408035e683a0331ac7\n|bogus2";
        let (guids, invalid) = Guid::parse_list(raw);
        assert_eq!(
            guids,
            vec![
                reference(),
                Guid::parse("7c1e1c2a-0001-4000-8000-000000000001").unwrap(),
                Guid::parse("e269fbb5-3750-427a-9149-7aa950b49301").unwrap(),
                Guid::parse("455a3e98-a627-4040-8035-e683a0331ac7").unwrap(),
            ]
        );
        assert_eq!(
            invalid,
            vec!["not-a-guid".to_string(), "bogus2".to_string()]
        );
    }

    #[test]
    fn parse_list_skips_empty_tokens() {
        // `\r\n` produces an empty token between `\r` and `\n`; pipes and
        // whitespace-only tokens are skipped too.
        let (guids, invalid) = Guid::parse_list("||\r\n\n   |\t|");
        assert!(guids.is_empty());
        assert!(invalid.is_empty());

        let (guids, invalid) = Guid::parse_list("");
        assert!(guids.is_empty());
        assert!(invalid.is_empty());
    }

    #[test]
    fn parse_list_crlf_only_separators() {
        let raw = "ab86861a-6030-46c5-b394-e8f99e8b87db\r\n{E269FBB5-3750-427A-9149-7AA950B49301}";
        let (guids, invalid) = Guid::parse_list(raw);
        assert_eq!(guids.len(), 2);
        assert!(invalid.is_empty());
        assert_eq!(guids[1].rainbow(), "e269fbb5-3750-427a-9149-7aa950b49301");
    }

    // ---- serde ------------------------------------------------------------

    #[test]
    fn guid_serializes_as_rainbow_string() {
        let json = serde_json::to_string(&reference()).unwrap();
        assert_eq!(json, format!("\"{HYPHENATED}\""));
    }

    #[test]
    fn guid_deserializes_from_any_form() {
        for form in [
            format!("\"{HYPHENATED}\""),
            "\"{AB86861A-6030-46C5-B394-E8F99E8B87DB}\"".to_string(),
            "\"AB86861A603046C5B394E8F99E8B87DB\"".to_string(),
        ] {
            let g: Guid = serde_json::from_str(&form).unwrap();
            assert_eq!(g, reference());
        }
    }

    #[test]
    fn guid_serde_rejects_bad_input() {
        assert!(serde_json::from_str::<Guid>("\"nope\"").is_err());
        assert!(serde_json::from_str::<Guid>("42").is_err());
        assert!(serde_json::from_str::<Guid>("null").is_err());
    }

    #[test]
    fn guid_serde_round_trip() {
        let g = reference();
        let json = serde_json::to_string(&g).unwrap();
        let back: Guid = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    // ---- SectionKind ------------------------------------------------------

    #[test]
    fn section_kind_as_str() {
        assert_eq!(SectionKind::Shared.as_str(), "shared");
        assert_eq!(SectionKind::Unversioned.as_str(), "unversioned");
        assert_eq!(SectionKind::Versioned.as_str(), "versioned");
    }

    #[test]
    fn section_kind_display_matches_as_str() {
        for kind in [
            SectionKind::Shared,
            SectionKind::Unversioned,
            SectionKind::Versioned,
        ] {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn section_kind_serializes_as_lowercase_string() {
        assert_eq!(
            serde_json::to_string(&SectionKind::Shared).unwrap(),
            "\"shared\""
        );
        assert_eq!(
            serde_json::to_string(&SectionKind::Unversioned).unwrap(),
            "\"unversioned\""
        );
        assert_eq!(
            serde_json::to_string(&SectionKind::Versioned).unwrap(),
            "\"versioned\""
        );
    }

    #[test]
    fn section_kind_deserializes_round_trip() {
        for kind in [
            SectionKind::Shared,
            SectionKind::Unversioned,
            SectionKind::Versioned,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: SectionKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn section_kind_deserialize_is_case_insensitive() {
        let k: SectionKind = serde_json::from_str("\"SHARED\"").unwrap();
        assert_eq!(k, SectionKind::Shared);
        let k: SectionKind = serde_json::from_str("\"Versioned\"").unwrap();
        assert_eq!(k, SectionKind::Versioned);
    }

    #[test]
    fn section_kind_rejects_unknown_and_non_string() {
        assert!(serde_json::from_str::<SectionKind>("\"global\"").is_err());
        assert!(serde_json::from_str::<SectionKind>("1").is_err());
        assert!(serde_json::from_str::<SectionKind>("null").is_err());
    }
}
