//! Final-renderings delta merge — Sitecore semantics, T3 posture
//! (report, don't crash). DESIGN.md §6.2, rules quoted verbatim below.
//!
//! Deterministic rules:
//!
//! 1. Result devices = base devices in order. For each delta `<d id=...>`:
//!    - No base device with that id → append the delta device as given
//!      (note `DeviceWithoutLayout` if it lacks `l=`).
//!    - **Replace mode** if the delta device has `l=`, no `p:*` attribute
//!      anywhere within it, and every `<r>` in it has both `id=` and
//!      `ph=` → the delta device replaces the base device wholesale.
//!    - **Patch mode** otherwise: overlay non-`p:` device attributes; for
//!      each delta `<r>`: matching base `uid` → overlay its non-`p:`
//!      attributes; no match but has `id=` → insert (position from
//!      `p:before` / `p:after` with selector `r[@uid='{UID}']`;
//!      unparseable or unknown selector → `BadPositionRef`, append); no
//!      match and no `id=` → `UnknownUid`, skipped. Base renderings absent
//!      from the delta are kept.
//! 2. Notes surface through `resolve` and gate G2; nothing panics on
//!    weird deltas.

use serde::Serialize;
use treesmith_types::Guid;

use crate::layoutxml::XmlEl;

/// A non-fatal oddity found while applying a delta (T3: report, don't
/// crash). `device` carries the delta device's `id=` value as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeltaNote {
    /// A delta `<r>` without `id=` referenced a `uid` absent from the
    /// base device; the rendering was skipped.
    UnknownUid {
        /// The delta device's `id=` value.
        device: String,
        /// The unmatched `uid=` value as written (may be empty).
        uid: String,
    },
    /// A `p:before` / `p:after` selector was unparseable or referenced an
    /// unknown uid; the rendering was appended instead.
    BadPositionRef {
        /// The delta device's `id=` value.
        device: String,
        /// The offending selector expression as written.
        expr: String,
    },
    /// A delta device with no base counterpart was appended without an
    /// `l=` layout reference.
    DeviceWithoutLayout {
        /// The delta device's `id=` value.
        device: String,
    },
}

/// Normalizes a GUID-ish token for comparison: braced/hyphenated/case
/// variants of the same GUID compare equal; anything else compares as its
/// lowercased raw text.
pub(crate) fn guid_key(raw: &str) -> String {
    match Guid::parse(raw.trim()) {
        Ok(g) => g.rainbow(),
        Err(_) => raw.trim().to_ascii_lowercase(),
    }
}

/// Applies one final-renderings delta layer onto a base layout, per the
/// module-level rules. Never fails: oddities become [`DeltaNote`]s.
pub fn apply_delta(base: &XmlEl, delta: &XmlEl) -> (XmlEl, Vec<DeltaNote>) {
    let mut notes = Vec::new();
    let mut out = XmlEl {
        name: base.name.clone(),
        attrs: base.attrs.clone(),
        children: Vec::new(),
    };

    // Rule 1: result devices = base devices in order (non-`d` children of
    // the base root are preserved in place).
    let mut devices: Vec<XmlEl> = base.children.clone();

    for delta_device in delta.children.iter().filter(|c| c.name == "d") {
        let delta_id = delta_device.attr("id").unwrap_or("");
        let key = guid_key(delta_id);
        let base_slot = devices
            .iter()
            .position(|d| d.name == "d" && guid_key(d.attr("id").unwrap_or("")) == key);

        match base_slot {
            None => {
                // No base device with that id → append as given.
                if delta_device.attr("l").is_none_or(str::is_empty) {
                    notes.push(DeltaNote::DeviceWithoutLayout {
                        device: delta_id.to_string(),
                    });
                }
                devices.push(delta_device.clone());
            }
            Some(slot) => {
                if is_replace_mode(delta_device) {
                    // Replace mode: the delta device replaces wholesale.
                    devices[slot] = delta_device.clone();
                } else {
                    patch_device(&mut devices[slot], delta_device, delta_id, &mut notes);
                }
            }
        }
    }

    out.children = devices;
    (out, notes)
}

/// Replace mode: `l=` present, no `p:*` attribute anywhere within the
/// device, and every `<r>` in it has both `id=` and `ph=`.
fn is_replace_mode(device: &XmlEl) -> bool {
    device.attr("l").is_some_and(|l| !l.is_empty())
        && !has_patch_attr(device)
        && renderings_of(device).all(|r| {
            r.attr("id").is_some_and(|v| !v.is_empty())
                && r.attr("ph").is_some_and(|v| !v.is_empty())
        })
}

fn has_patch_attr(el: &XmlEl) -> bool {
    el.attrs.iter().any(|(k, _)| k.starts_with("p:")) || el.children.iter().any(has_patch_attr)
}

fn renderings_of(device: &XmlEl) -> impl Iterator<Item = &XmlEl> {
    // Renderings are the `<r>` element children of the device (the layout
    // subset nests no deeper).
    device.children.iter().filter(|c| c.name == "r")
}

/// Patch mode for one device (see rule 1, patch branch).
fn patch_device(base: &mut XmlEl, delta: &XmlEl, delta_id: &str, notes: &mut Vec<DeltaNote>) {
    // Overlay non-`p:` device attributes.
    overlay_attrs(base, delta);

    for delta_r in delta.children.iter().filter(|c| c.name == "r") {
        let uid = delta_r.attr("uid").unwrap_or("");
        let uid_key = guid_key(uid);
        let matched = base.children.iter_mut().find(|c| {
            c.name == "r" && !uid.is_empty() && guid_key(c.attr("uid").unwrap_or("")) == uid_key
        });

        if let Some(target) = matched {
            // Matching base uid → overlay its non-`p:` attributes.
            overlay_attrs(target, delta_r);
            continue;
        }
        if delta_r.attr("id").is_none_or(str::is_empty) {
            // No match and no id= → note and skip.
            notes.push(DeltaNote::UnknownUid {
                device: delta_id.to_string(),
                uid: uid.to_string(),
            });
            continue;
        }
        // No match but has id= → insert at the requested position.
        let mut inserted = delta_r.clone();
        inserted.attrs.retain(|(k, _)| !k.starts_with("p:"));
        let position = insertion_index(base, delta_r);
        match position {
            Ok(idx) => base.children.insert(idx, inserted),
            Err(Some(expr)) => {
                notes.push(DeltaNote::BadPositionRef {
                    device: delta_id.to_string(),
                    expr,
                });
                base.children.push(inserted);
            }
            Err(None) => base.children.push(inserted), // no p:before/p:after → append
        }
    }
}

/// Copies every non-`p:` attribute of `delta` onto `base`, replacing
/// existing values and appending new keys. Base-only attributes are kept.
fn overlay_attrs(base: &mut XmlEl, delta: &XmlEl) {
    for (k, v) in &delta.attrs {
        if k.starts_with("p:") {
            continue;
        }
        match base.attrs.iter_mut().find(|(bk, _)| bk == k) {
            Some((_, bv)) => *bv = v.clone(),
            None => base.attrs.push((k.clone(), v.clone())),
        }
    }
}

/// Where to insert a delta rendering: `Ok(index)` from a resolvable
/// `p:before` / `p:after`, `Err(Some(expr))` for an unparseable or
/// unknown selector, `Err(None)` when no position attribute is present.
fn insertion_index(base: &XmlEl, delta_r: &XmlEl) -> Result<usize, Option<String>> {
    let (expr, after) = match (delta_r.attr("p:before"), delta_r.attr("p:after")) {
        (Some(e), _) => (e, false),
        (None, Some(e)) => (e, true),
        (None, None) => return Err(None),
    };
    let Some(uid) = parse_selector(expr) else {
        return Err(Some(expr.to_string()));
    };
    let key = guid_key(&uid);
    let found = base
        .children
        .iter()
        .position(|c| c.name == "r" && guid_key(c.attr("uid").unwrap_or("")) == key);
    match found {
        Some(idx) => Ok(if after { idx + 1 } else { idx }),
        None => Err(Some(expr.to_string())),
    }
}

/// Parses the selector form `r[@uid='{UID}']` (braces optional, quotes
/// may be single or double). Returns the UID token.
fn parse_selector(expr: &str) -> Option<String> {
    let rest = expr.trim().strip_prefix("r[@uid=")?;
    let rest = rest.strip_suffix(']')?;
    let inner = if let Some(q) = rest.strip_prefix('\'') {
        q.strip_suffix('\'')?
    } else if let Some(q) = rest.strip_prefix('"') {
        q.strip_suffix('"')?
    } else {
        return None;
    };
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layoutxml::parse_xml;

    const DEV1: &str = "{FE5D7FDF-89C0-4D99-9AA3-B5FBD009C9F3}";
    const UID_A: &str = "{11111111-1111-4111-8111-111111111101}";
    const UID_B: &str = "{11111111-1111-4111-8111-111111111102}";
    const UID_C: &str = "{11111111-1111-4111-8111-111111111103}";

    fn base() -> XmlEl {
        parse_xml(&format!(
            r#"<r>
                 <d id="{DEV1}" l="{{9A11AAAA-0001-4000-8000-000000000001}}">
                   <r id="{{9A11AAAA-0003-4000-8000-000000000003}}" ph="main" uid="{UID_A}" />
                   <r ds="{{C0FFEE00-0002-4000-8000-000000000002}}" id="{{9A11AAAA-0002-4000-8000-000000000002}}" ph="main" uid="{UID_B}" />
                 </d>
               </r>"#
        ))
        .unwrap()
    }

    fn uids(device: &XmlEl) -> Vec<String> {
        device
            .children
            .iter()
            .filter(|c| c.name == "r")
            .map(|c| guid_key(c.attr("uid").unwrap_or("")))
            .collect()
    }

    #[test]
    fn attr_overlay_keeps_base_attrs() {
        // Patch mode (no l= on the delta device): overlay Hero's ds, keep
        // its id/ph, keep the device's l=.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}"><r ds="local:/Data/HeroData" uid="{UID_B}" /></d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        let dev = &merged.children[0];
        assert_eq!(
            dev.attr("l"),
            Some("{9A11AAAA-0001-4000-8000-000000000001}"),
            "base device attrs survive the overlay"
        );
        let hero = dev
            .children
            .iter()
            .find(|c| guid_key(c.attr("uid").unwrap()) == guid_key(UID_B))
            .unwrap();
        assert_eq!(hero.attr("ds"), Some("local:/Data/HeroData"), "overlaid");
        assert_eq!(
            hero.attr("id"),
            Some("{9A11AAAA-0002-4000-8000-000000000002}"),
            "base rendering attrs kept"
        );
        assert_eq!(hero.attr("ph"), Some("main"), "base rendering attrs kept");
    }

    #[test]
    fn p_after_inserts_at_position() {
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" p:after="r[@uid='{UID_A}']" ph="main" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        let dev = &merged.children[0];
        assert_eq!(
            uids(dev),
            vec![guid_key(UID_A), guid_key(UID_C), guid_key(UID_B)],
            "inserted directly after UID_A"
        );
        // p:* attributes are not carried into the merged output.
        let inserted = &dev.children[1];
        assert!(inserted.attr("p:after").is_none());
    }

    #[test]
    fn p_before_inserts_at_position() {
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" p:before="r[@uid='{UID_A}']" ph="main" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            uids(&merged.children[0]),
            vec![guid_key(UID_C), guid_key(UID_A), guid_key(UID_B)]
        );
    }

    #[test]
    fn unknown_uid_notes_and_skips() {
        // No matching base uid and no id= -> UnknownUid, skipped.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}"><r ds="whatever" uid="{UID_C}" /></d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert_eq!(
            notes,
            vec![DeltaNote::UnknownUid {
                device: DEV1.to_string(),
                uid: UID_C.to_string(),
            }]
        );
        assert_eq!(
            uids(&merged.children[0]),
            vec![guid_key(UID_A), guid_key(UID_B)]
        );
    }

    #[test]
    fn bad_position_ref_notes_and_appends() {
        // Unknown selector target.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" p:after="r[@uid='{{99999999-9999-4999-8999-999999999999}}']" ph="main" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert_eq!(
            notes,
            vec![DeltaNote::BadPositionRef {
                device: DEV1.to_string(),
                expr: "r[@uid='{99999999-9999-4999-8999-999999999999}']".to_string(),
            }]
        );
        assert_eq!(
            uids(&merged.children[0]),
            vec![guid_key(UID_A), guid_key(UID_B), guid_key(UID_C)],
            "appended"
        );

        // Unparseable selector.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" p:before="totally bogus" ph="main" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert_eq!(
            notes,
            vec![DeltaNote::BadPositionRef {
                device: DEV1.to_string(),
                expr: "totally bogus".to_string(),
            }]
        );
        assert_eq!(uids(&merged.children[0]).len(), 3, "still appended");
    }

    #[test]
    fn replace_mode_triggers_on_full_device() {
        // l= present, no p:* anywhere, every <r> has id= and ph= -> the
        // delta device replaces the base device wholesale.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}" l="{{3A45A723-64EE-4919-9D41-02FD40FD1466}}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" ph="hero" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        let dev = &merged.children[0];
        assert_eq!(
            dev.attr("l"),
            Some("{3A45A723-64EE-4919-9D41-02FD40FD1466}"),
            "delta device wins wholesale"
        );
        assert_eq!(uids(dev), vec![guid_key(UID_C)]);
        assert_eq!(merged.children.len(), 1, "device count unchanged");
    }

    #[test]
    fn missing_ph_prevents_replace_mode() {
        // Same shape but one <r> lacks ph= -> patch mode: renderings merge
        // instead of replacing.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}" l="{{3A45A723-64EE-4919-9D41-02FD40FD1466}}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, _) = apply_delta(&base(), &delta);
        let dev = &merged.children[0];
        assert_eq!(
            dev.attr("l"),
            Some("{3A45A723-64EE-4919-9D41-02FD40FD1466}"),
            "device attrs still overlaid in patch mode"
        );
        assert_eq!(uids(dev).len(), 3, "base renderings kept + insert appended");
    }

    #[test]
    fn p_attr_anywhere_prevents_replace_mode() {
        // l= present and every <r> complete, but a p:* attribute exists ->
        // patch mode.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV1}" l="{{3A45A723-64EE-4919-9D41-02FD40FD1466}}">
                 <r id="{{9A11AAAA-0004-4000-8000-000000000004}}" p:after="r[@uid='{UID_A}']" ph="main" uid="{UID_C}" />
               </d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            uids(&merged.children[0]),
            vec![guid_key(UID_A), guid_key(UID_C), guid_key(UID_B)],
            "patched, not replaced"
        );
    }

    #[test]
    fn new_device_appends_and_notes_missing_layout() {
        const DEV2: &str = "{46D2F427-4CE5-4E1F-BA10-EF3636F43534}";
        // Without l= -> appended + DeviceWithoutLayout note.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV2}"><r id="{{9A11AAAA-0004-4000-8000-000000000004}}" ph="print" uid="{UID_C}" /></d></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert_eq!(merged.children.len(), 2);
        assert_eq!(
            guid_key(merged.children[1].attr("id").unwrap()),
            guid_key(DEV2)
        );
        assert_eq!(
            notes,
            vec![DeltaNote::DeviceWithoutLayout {
                device: DEV2.to_string(),
            }]
        );

        // With l= -> appended, no note.
        let delta = parse_xml(&format!(
            r#"<r><d id="{DEV2}" l="{{3A45A723-64EE-4919-9D41-02FD40FD1466}}" /></r>"#
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert_eq!(merged.children.len(), 2);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn device_and_uid_matching_is_guid_form_insensitive() {
        // Lowercase unbraced id refers to the same device.
        let delta = parse_xml(&format!(
            r#"<r><d id="fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3"><r ds="x" uid="{}" /></d></r>"#,
            UID_B.to_ascii_lowercase()
        ))
        .unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(merged.children.len(), 1, "matched, not appended");
    }

    #[test]
    fn base_renderings_absent_from_delta_are_kept() {
        let delta = parse_xml(&format!(r#"<r><d id="{DEV1}"></d></r>"#)).unwrap();
        let (merged, notes) = apply_delta(&base(), &delta);
        assert!(notes.is_empty());
        assert_eq!(
            uids(&merged.children[0]),
            vec![guid_key(UID_A), guid_key(UID_B)]
        );
    }

    #[test]
    fn selector_parses_quote_variants() {
        assert_eq!(parse_selector("r[@uid='{ABC}']").as_deref(), Some("{ABC}"));
        assert_eq!(
            parse_selector(r#"r[@uid="{ABC}"]"#).as_deref(),
            Some("{ABC}")
        );
        assert_eq!(parse_selector("div[@uid='{ABC}']"), None);
        assert_eq!(parse_selector("r[@uid='{ABC}'"), None);
    }

    #[test]
    fn notes_serialize_camel_case_tagged() {
        let note = DeltaNote::BadPositionRef {
            device: "d".into(),
            expr: "e".into(),
        };
        let json = serde_json::to_value(&note).unwrap();
        assert_eq!(json["kind"], "badPositionRef");
        assert_eq!(json["expr"], "e");
    }
}
