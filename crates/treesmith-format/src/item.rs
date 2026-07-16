//! Typed view + schema-shaped mutations over a parsed item document
//! (DESIGN.md §3.2).
//!
//! Rainbow item schema (canonical key orders — used when *inserting*;
//! existing order is always preserved):
//!
//! - Top level: `ID`, `Parent`, `Template`, `Path`, `DB`, `BranchID`,
//!   `SharedFields`, `Languages`.
//! - Language item: `Language`, `Fields`, `Versions`.
//! - Version item: `Version`, `Fields`.
//! - Field item: `ID`, `Hint`, `Type`, `BlobID`, `Value`.
//!
//! Insert sorting (VERIFY-P0, mirrors Rainbow's deterministic output):
//! fields by GUID string ascending, languages alphabetically, versions
//! numerically.

use crate::yaml::{scalar_for_new_value, Entry, Scalar, Value, YamlDocument};
use treesmith_types::Guid;

/// A parsed item: the lossless document plus a typed, Rainbow-schema-aware
/// view over it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedItem {
    /// The underlying lossless document.
    pub doc: YamlDocument,
}

/// A field occurrence as serialized (one slot's view of one field).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldRef {
    /// The field definition GUID.
    pub id: Guid,
    /// The `Hint:` (field name) if serialized.
    pub hint: Option<String>,
    /// The `Type:` hint if serialized.
    pub type_hint: Option<String>,
    /// The `BlobID:` if serialized (binary fields).
    pub blob_id: Option<Guid>,
    /// The field value (block scalars joined with `\n`).
    pub value: String,
}

/// One language's serialized fields.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LanguageBlock {
    /// The language code as serialized (e.g. `en`, `da`).
    pub language: String,
    /// Fields in the language's unversioned section.
    pub unversioned: Vec<FieldRef>,
    /// `(version number, fields)` in serialized order.
    pub versions: Vec<(u32, Vec<FieldRef>)>,
}

/// Which storage slot of an item a field value sits in.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldSlot {
    /// The item-wide shared section.
    Shared,
    /// A language's unversioned section.
    Unversioned {
        /// Language code.
        language: String,
    },
    /// A numbered version of a language.
    Versioned {
        /// Language code.
        language: String,
        /// Version number.
        version: u32,
    },
}

const TOP_ORDER: &[&str] = &[
    "ID",
    "Parent",
    "Template",
    "Path",
    "DB",
    "BranchID",
    "SharedFields",
    "Languages",
];
const LANG_ORDER: &[&str] = &["Language", "Fields", "Versions"];
const VERSION_ORDER: &[&str] = &["Version", "Fields"];
const FIELD_ORDER: &[&str] = &["ID", "Hint", "Type", "BlobID", "Value"];

// ---- read helpers over raw entries ----------------------------------------

fn entry_value<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Value> {
    entries.iter().find(|e| e.key == key).map(|e| &e.value)
}

fn scalar_value(entries: &[Entry], key: &str) -> Option<String> {
    match entry_value(entries, key)? {
        Value::Scalar(s) => Some(s.value()),
        _ => None,
    }
}

fn guid_value(entries: &[Entry], key: &str) -> Option<Guid> {
    Guid::parse(&scalar_value(entries, key)?).ok()
}

fn field_from_item(entries: &[Entry]) -> Option<FieldRef> {
    Some(FieldRef {
        id: guid_value(entries, "ID")?,
        hint: scalar_value(entries, "Hint"),
        type_hint: scalar_value(entries, "Type"),
        blob_id: guid_value(entries, "BlobID"),
        value: scalar_value(entries, "Value").unwrap_or_default(),
    })
}

fn fields_of_list(entries: &[Entry], key: &str) -> Vec<FieldRef> {
    match entry_value(entries, key) {
        Some(Value::List(items)) => items.iter().filter_map(|it| field_from_item(it)).collect(),
        _ => Vec::new(),
    }
}

// ---- canonical insertion ---------------------------------------------------

/// Inserts `entry` before the first existing entry whose canonical rank is
/// greater; keys outside `order` are left where they are and never compared.
fn canonical_insert(entries: &mut Vec<Entry>, order: &[&str], entry: Entry) -> usize {
    let rank = |k: &str| order.iter().position(|o| *o == k);
    let mine = rank(&entry.key);
    let pos = entries
        .iter()
        .position(|e| matches!((mine, rank(&e.key)), (Some(m), Some(r)) if r > m))
        .unwrap_or(entries.len());
    entries.insert(pos, entry);
    pos
}

fn set_scalar_entry(entries: &mut Vec<Entry>, order: &[&str], key: &str, scalar: Scalar) {
    if let Some(e) = entries.iter_mut().find(|e| e.key == key) {
        e.value = Value::Scalar(scalar);
    } else {
        canonical_insert(
            entries,
            order,
            Entry {
                key: key.to_string(),
                value: Value::Scalar(scalar),
            },
        );
    }
}

/// Returns the index of the `key` list entry, creating it (empty, at its
/// canonical position) if absent. An existing non-list value is replaced by
/// an empty list (defensive; never happens on well-formed items).
fn ensure_list_entry(entries: &mut Vec<Entry>, order: &[&str], key: &str) -> usize {
    if let Some(i) = entries.iter().position(|e| e.key == key) {
        if !matches!(entries[i].value, Value::List(_)) {
            entries[i].value = Value::List(Vec::new());
        }
        i
    } else {
        canonical_insert(
            entries,
            order,
            Entry {
                key: key.to_string(),
                value: Value::List(Vec::new()),
            },
        )
    }
}

fn list_items_mut(entries: &mut [Entry], idx: usize) -> &mut Vec<Vec<Entry>> {
    match &mut entries[idx].value {
        Value::List(items) => items,
        _ => unreachable!("ensure_list_entry guarantees a list"),
    }
}

fn list_items<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Vec<Vec<Entry>>> {
    match entry_value(entries, key)? {
        Value::List(items) => Some(items),
        _ => None,
    }
}

impl ParsedItem {
    // ---- accessors ----------------------------------------------------------

    /// The item's `ID`, if present and parseable.
    pub fn id(&self) -> Option<Guid> {
        guid_value(&self.doc.root, "ID")
    }

    /// The item's `Parent` GUID.
    pub fn parent_id(&self) -> Option<Guid> {
        guid_value(&self.doc.root, "Parent")
    }

    /// The item's `Template` GUID.
    pub fn template_id(&self) -> Option<Guid> {
        guid_value(&self.doc.root, "Template")
    }

    /// The item's `Path`.
    pub fn path(&self) -> Option<String> {
        scalar_value(&self.doc.root, "Path")
    }

    /// The item's `DB`.
    pub fn db(&self) -> Option<String> {
        scalar_value(&self.doc.root, "DB")
    }

    /// Fields in the shared section, serialized order.
    pub fn shared_fields(&self) -> Vec<FieldRef> {
        fields_of_list(&self.doc.root, "SharedFields")
    }

    /// Language blocks in serialized order.
    pub fn languages(&self) -> Vec<LanguageBlock> {
        let Some(items) = list_items(&self.doc.root, "Languages") else {
            return Vec::new();
        };
        items
            .iter()
            .map(|item| {
                let versions = match entry_value(item, "Versions") {
                    Some(Value::List(vitems)) => vitems
                        .iter()
                        .filter_map(|v| {
                            let num = scalar_value(v, "Version")?.parse::<u32>().ok()?;
                            Some((num, fields_of_list(v, "Fields")))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                LanguageBlock {
                    language: scalar_value(item, "Language").unwrap_or_default(),
                    unversioned: fields_of_list(item, "Fields"),
                    versions,
                }
            })
            .collect()
    }

    /// Deterministic first match for a field id: shared, then languages
    /// alphabetically (unversioned first, then versions ascending).
    pub fn find_field(&self, id: Guid) -> Option<(FieldSlot, FieldRef)> {
        if let Some(f) = self.shared_fields().into_iter().find(|f| f.id == id) {
            return Some((FieldSlot::Shared, f));
        }
        let mut langs = self.languages();
        langs.sort_by(|a, b| a.language.cmp(&b.language));
        for lang in langs {
            if let Some(f) = lang.unversioned.into_iter().find(|f| f.id == id) {
                return Some((
                    FieldSlot::Unversioned {
                        language: lang.language,
                    },
                    f,
                ));
            }
            let mut versions = lang.versions;
            versions.sort_by_key(|(n, _)| *n);
            for (num, fields) in versions {
                if let Some(f) = fields.into_iter().find(|f| f.id == id) {
                    return Some((
                        FieldSlot::Versioned {
                            language: lang.language,
                            version: num,
                        },
                        f,
                    ));
                }
            }
        }
        None
    }

    /// Highest version number serialized for `language`, if any.
    pub fn max_version(&self, language: &str) -> Option<u32> {
        self.languages()
            .into_iter()
            .find(|l| l.language == language)?
            .versions
            .into_iter()
            .map(|(n, _)| n)
            .max()
    }

    // ---- mutations ------------------------------------------------------------

    /// Sets a field value in `slot`, creating the field (and any missing
    /// section / language / version scaffolding, at canonical positions and
    /// canonical sort order) as needed.
    ///
    /// On an existing field only the `Value` scalar is replaced (style via
    /// [`scalar_for_new_value`]) and `Type` updated if `type_hint` is given;
    /// the serialized `Hint` and entry order are preserved.
    pub fn set_field(
        &mut self,
        slot: &FieldSlot,
        id: Guid,
        hint: Option<&str>,
        type_hint: Option<&str>,
        value: &str,
    ) {
        let fields = self.ensure_fields_list(slot);
        let existing = fields
            .iter_mut()
            .find(|item| guid_value(item, "ID") == Some(id));
        if let Some(item) = existing {
            set_scalar_entry(item, FIELD_ORDER, "Value", scalar_for_new_value(value));
            if let Some(t) = type_hint {
                set_scalar_entry(item, FIELD_ORDER, "Type", Scalar::Plain(t.to_string()));
            }
        } else {
            let mut item = vec![Entry {
                key: "ID".to_string(),
                value: Value::Scalar(Scalar::Quoted(id.rainbow())),
            }];
            if let Some(h) = hint {
                item.push(Entry {
                    key: "Hint".to_string(),
                    value: Value::Scalar(Scalar::Plain(h.to_string())),
                });
            }
            if let Some(t) = type_hint {
                item.push(Entry {
                    key: "Type".to_string(),
                    value: Value::Scalar(Scalar::Plain(t.to_string())),
                });
            }
            item.push(Entry {
                key: "Value".to_string(),
                value: Value::Scalar(scalar_for_new_value(value)),
            });
            // Insert before the first existing field with a greater id
            // (GUID string ascending; Guid's Ord matches rainbow order).
            let pos = fields
                .iter()
                .position(|it| matches!(guid_value(it, "ID"), Some(g) if g > id))
                .unwrap_or(fields.len());
            fields.insert(pos, item);
        }
    }

    /// Removes a field from `slot`. Lists/sections left empty are removed
    /// entirely (Rainbow omits empty sections). Returns whether a field was
    /// removed.
    pub fn remove_field(&mut self, slot: &FieldSlot, id: Guid) -> bool {
        match slot {
            FieldSlot::Shared => {
                let removed = remove_field_from_list(&mut self.doc.root, "SharedFields", id);
                prune_empty_list(&mut self.doc.root, "SharedFields");
                removed
            }
            FieldSlot::Unversioned { language } => {
                let Some(li) = self.language_index(language) else {
                    return false;
                };
                let langs = self.languages_list_mut().expect("language item exists");
                let removed = remove_field_from_list(&mut langs[li], "Fields", id);
                self.prune_language(language);
                removed
            }
            FieldSlot::Versioned { language, version } => {
                let Some((li, vi)) = self.version_index(language, *version) else {
                    return false;
                };
                let langs = self.languages_list_mut().expect("language item exists");
                let Some(vidx) = langs[li].iter().position(|e| e.key == "Versions") else {
                    return false;
                };
                let versions = list_items_mut(&mut langs[li], vidx);
                let removed = remove_field_from_list(&mut versions[vi], "Fields", id);
                // Prune the version item if it no longer holds fields.
                if !versions[vi].iter().any(|e| e.key == "Fields") {
                    versions.remove(vi);
                }
                prune_empty_list(&mut langs[li], "Versions");
                self.prune_language(language);
                removed
            }
        }
    }

    /// Ensures a version item exists for `language`/`version`, creating the
    /// language block and version item (in canonical positions) if needed.
    pub fn ensure_version(&mut self, language: &str, version: u32) {
        let li = self.ensure_language_item(language);
        let langs = self.languages_list_mut().expect("Languages exists");
        let vidx = ensure_list_entry(&mut langs[li], LANG_ORDER, "Versions");
        let versions = list_items_mut(&mut langs[li], vidx);
        if version_position(versions, version).is_none() {
            let item = vec![Entry {
                key: "Version".to_string(),
                value: Value::Scalar(Scalar::Plain(version.to_string())),
            }];
            let pos = versions
                .iter()
                .position(|v| matches!(version_number(v), Some(n) if n > version))
                .unwrap_or(versions.len());
            versions.insert(pos, item);
        }
    }

    /// Sets the top-level `Path` (Plain style, canonical position).
    pub fn set_path(&mut self, path: &str) {
        set_scalar_entry(
            &mut self.doc.root,
            TOP_ORDER,
            "Path",
            Scalar::Plain(path.to_string()),
        );
    }

    /// Sets the top-level `Parent` GUID (always `Quoted(guid.rainbow())`).
    pub fn set_parent(&mut self, parent: Guid) {
        set_scalar_entry(
            &mut self.doc.root,
            TOP_ORDER,
            "Parent",
            Scalar::Quoted(parent.rainbow()),
        );
    }

    // ---- mutation plumbing -----------------------------------------------------

    fn languages_list_mut(&mut self) -> Option<&mut Vec<Vec<Entry>>> {
        let idx = self.doc.root.iter().position(|e| e.key == "Languages")?;
        match &mut self.doc.root[idx].value {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    fn language_index(&self, language: &str) -> Option<usize> {
        list_items(&self.doc.root, "Languages")?
            .iter()
            .position(|item| scalar_value(item, "Language").as_deref() == Some(language))
    }

    fn version_index(&self, language: &str, version: u32) -> Option<(usize, usize)> {
        let li = self.language_index(language)?;
        let langs = list_items(&self.doc.root, "Languages")?;
        let versions = list_items(&langs[li], "Versions")?;
        Some((li, version_position(versions, version)?))
    }

    /// Index of the language item, creating it in alphabetical position
    /// (with its `Language` scalar) if missing.
    fn ensure_language_item(&mut self, language: &str) -> usize {
        if let Some(i) = self.language_index(language) {
            return i;
        }
        let idx = ensure_list_entry(&mut self.doc.root, TOP_ORDER, "Languages");
        let items = list_items_mut(&mut self.doc.root, idx);
        let pos = items
            .iter()
            .position(
                |item| matches!(scalar_value(item, "Language"), Some(l) if l.as_str() > language),
            )
            .unwrap_or(items.len());
        items.insert(
            pos,
            vec![Entry {
                key: "Language".to_string(),
                value: Value::Scalar(Scalar::Plain(language.to_string())),
            }],
        );
        pos
    }

    /// The fields list for `slot`, creating all scaffolding as needed.
    fn ensure_fields_list(&mut self, slot: &FieldSlot) -> &mut Vec<Vec<Entry>> {
        match slot {
            FieldSlot::Shared => {
                let idx = ensure_list_entry(&mut self.doc.root, TOP_ORDER, "SharedFields");
                list_items_mut(&mut self.doc.root, idx)
            }
            FieldSlot::Unversioned { language } => {
                let li = self.ensure_language_item(language);
                let langs = self.languages_list_mut().expect("Languages exists");
                let fidx = ensure_list_entry(&mut langs[li], LANG_ORDER, "Fields");
                list_items_mut(&mut langs[li], fidx)
            }
            FieldSlot::Versioned { language, version } => {
                self.ensure_version(language, *version);
                let li = self.language_index(language).expect("language ensured");
                let langs = self.languages_list_mut().expect("Languages exists");
                let vidx = langs[li]
                    .iter()
                    .position(|e| e.key == "Versions")
                    .expect("Versions ensured");
                let versions = list_items_mut(&mut langs[li], vidx);
                let vi = version_position(versions, *version).expect("version ensured");
                let fidx = ensure_list_entry(&mut versions[vi], VERSION_ORDER, "Fields");
                list_items_mut(&mut versions[vi], fidx)
            }
        }
    }

    /// Removes a language item that lost both `Fields` and `Versions`, and
    /// the `Languages` entry itself if no language items remain.
    fn prune_language(&mut self, language: &str) {
        if let Some(li) = self.language_index(language) {
            let langs = self.languages_list_mut().expect("language item exists");
            // Only prune when nothing but the `Language` scalar remains —
            // unknown keys keep the item alive (tolerant-read posture).
            if langs[li].iter().all(|e| e.key == "Language") {
                langs.remove(li);
            }
        }
        if let Some(items) = list_items(&self.doc.root, "Languages") {
            if items.is_empty() {
                self.doc.root.retain(|e| e.key != "Languages");
            }
        }
    }
}

fn version_number(item: &[Entry]) -> Option<u32> {
    scalar_value(item, "Version")?.parse().ok()
}

fn version_position(versions: &[Vec<Entry>], version: u32) -> Option<usize> {
    versions
        .iter()
        .position(|v| version_number(v) == Some(version))
}

/// Removes the field with `id` from the `key` list inside `entries`; drops
/// the list entry entirely when it becomes empty. Returns whether removed.
fn remove_field_from_list(entries: &mut Vec<Entry>, key: &str, id: Guid) -> bool {
    let Some(idx) = entries.iter().position(|e| e.key == key) else {
        return false;
    };
    let Value::List(items) = &mut entries[idx].value else {
        return false;
    };
    let before = items.len();
    items.retain(|item| guid_value(item, "ID") != Some(id));
    let removed = items.len() < before;
    if items.is_empty() {
        entries.remove(idx);
    }
    removed
}

/// Drops the `key` entry if it is an empty list.
fn prune_empty_list(entries: &mut Vec<Entry>, key: &str) {
    if let Some(idx) = entries.iter().position(|e| e.key == key) {
        if matches!(&entries[idx].value, Value::List(items) if items.is_empty()) {
            entries.remove(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::{emit, parse};

    fn g(s: &str) -> Guid {
        Guid::parse(s).unwrap()
    }

    const F_TITLE: &str = "7c1e1c2a-0003-4000-8000-000000000003";
    const F_NAV: &str = "7c1e1c2a-0005-4000-8000-000000000005";
    const F_RELATED: &str = "7c1e1c2a-0006-4000-8000-000000000006";

    /// A realistic Rainbow item: shared, unversioned, and versioned fields
    /// across two languages (deliberately serialized `en` before `da` to
    /// prove find_field sorts alphabetically rather than by file order).
    fn realistic() -> &'static str {
        concat!(
            "---\n",
            "ID: \"c0ffee00-0001-4000-8000-000000000001\"\n",
            "Parent: \"aaaaaaaa-0000-4000-8000-0000000000aa\"\n",
            "Template: \"7c1e1c2a-0020-4000-8000-000000000020\"\n",
            "Path: /sitecore/content/Home\n",
            "DB: master\n",
            "SharedFields:\n",
            "- ID: \"7c1e1c2a-0006-4000-8000-000000000006\"\n",
            "  Hint: RelatedPages\n",
            "  Type: Treelist\n",
            "  Value: |\n",
            "    {C0FFEE00-0003-4000-8000-000000000003}\n",
            "Languages:\n",
            "- Language: en\n",
            "  Fields:\n",
            "  - ID: \"7c1e1c2a-0005-4000-8000-000000000005\"\n",
            "    Hint: NavTitle\n",
            "    Value: Home nav\n",
            "  Versions:\n",
            "  - Version: 1\n",
            "    Fields:\n",
            "    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n",
            "      Hint: Title\n",
            "      Value: Home v1\n",
            "  - Version: 2\n",
            "    Fields:\n",
            "    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n",
            "      Hint: Title\n",
            "      Value: Home v2\n",
            "- Language: da\n",
            "  Versions:\n",
            "  - Version: 1\n",
            "    Fields:\n",
            "    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n",
            "      Hint: Title\n",
            "      Value: Hjem\n",
        )
    }

    fn item(src: &str) -> ParsedItem {
        ParsedItem {
            doc: parse(src.as_bytes()).expect("fixture parses"),
        }
    }

    fn emitted(it: &ParsedItem) -> String {
        String::from_utf8(emit(&it.doc)).unwrap()
    }

    /// Every mutated document must still satisfy emit -> parse -> emit
    /// byte stability (the I3 self-check foundation).
    fn assert_stable(it: &ParsedItem) {
        let bytes = emit(&it.doc);
        let back = parse(&bytes).expect("mutated document re-parses");
        assert_eq!(emit(&back), bytes, "emit(parse(emit(doc))) must be stable");
        assert_eq!(back, it.doc, "re-parsed model must match");
    }

    // ---- accessors -----------------------------------------------------------

    #[test]
    fn accessors_on_realistic_item() {
        let it = item(realistic());
        assert_eq!(it.id(), Some(g("c0ffee00-0001-4000-8000-000000000001")));
        assert_eq!(
            it.parent_id(),
            Some(g("aaaaaaaa-0000-4000-8000-0000000000aa"))
        );
        assert_eq!(
            it.template_id(),
            Some(g("7c1e1c2a-0020-4000-8000-000000000020"))
        );
        assert_eq!(it.path().as_deref(), Some("/sitecore/content/Home"));
        assert_eq!(it.db().as_deref(), Some("master"));

        let shared = it.shared_fields();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].id, g(F_RELATED));
        assert_eq!(shared[0].hint.as_deref(), Some("RelatedPages"));
        assert_eq!(shared[0].type_hint.as_deref(), Some("Treelist"));
        assert_eq!(shared[0].blob_id, None);
        assert_eq!(shared[0].value, "{C0FFEE00-0003-4000-8000-000000000003}");

        let langs = it.languages();
        assert_eq!(langs.len(), 2);
        assert_eq!(langs[0].language, "en"); // serialized order preserved
        assert_eq!(langs[0].unversioned.len(), 1);
        assert_eq!(langs[0].unversioned[0].value, "Home nav");
        assert_eq!(langs[0].versions.len(), 2);
        assert_eq!(langs[0].versions[1].0, 2);
        assert_eq!(langs[0].versions[1].1[0].value, "Home v2");
        assert_eq!(langs[1].language, "da");
        assert!(langs[1].unversioned.is_empty());
    }

    #[test]
    fn blob_id_surfaces_on_field_ref() {
        let it = item(concat!(
            "---\n",
            "ID: \"c0ffee00-0001-4000-8000-000000000001\"\n",
            "SharedFields:\n",
            "- ID: \"7c1e1c2a-0006-4000-8000-000000000006\"\n",
            "  Hint: Blob\n",
            "  BlobID: \"deadbeef-0000-4000-8000-000000000000\"\n",
            "  Value: \"\"\n",
        ));
        let shared = it.shared_fields();
        assert_eq!(
            shared[0].blob_id,
            Some(g("deadbeef-0000-4000-8000-000000000000"))
        );
        assert_eq!(shared[0].value, "");
    }

    #[test]
    fn accessors_absent_on_empty_item() {
        let it = item("---\n");
        assert_eq!(it.id(), None);
        assert_eq!(it.parent_id(), None);
        assert_eq!(it.template_id(), None);
        assert_eq!(it.path(), None);
        assert_eq!(it.db(), None);
        assert!(it.shared_fields().is_empty());
        assert!(it.languages().is_empty());
        assert_eq!(it.find_field(g(F_TITLE)), None);
        assert_eq!(it.max_version("en"), None);
    }

    #[test]
    fn find_field_shared_wins() {
        let it = item(realistic());
        let (slot, f) = it.find_field(g(F_RELATED)).unwrap();
        assert_eq!(slot, FieldSlot::Shared);
        assert_eq!(f.hint.as_deref(), Some("RelatedPages"));
    }

    #[test]
    fn find_field_language_alphabetical_da_before_en() {
        // Title exists in da v1, en v1, en v2; da sorts first even though
        // en is serialized first.
        let it = item(realistic());
        let (slot, f) = it.find_field(g(F_TITLE)).unwrap();
        assert_eq!(
            slot,
            FieldSlot::Versioned {
                language: "da".into(),
                version: 1
            }
        );
        assert_eq!(f.value, "Hjem");
    }

    #[test]
    fn find_field_unversioned_before_versions() {
        // NavTitle only exists unversioned in en.
        let it = item(realistic());
        let (slot, f) = it.find_field(g(F_NAV)).unwrap();
        assert_eq!(
            slot,
            FieldSlot::Unversioned {
                language: "en".into()
            }
        );
        assert_eq!(f.value, "Home nav");

        // A field in both unversioned and versioned of the same language
        // resolves to unversioned first.
        let mut it2 = item(realistic());
        it2.set_field(
            &FieldSlot::Unversioned {
                language: "da".into(),
            },
            g(F_TITLE),
            Some("Title"),
            None,
            "da unversioned",
        );
        let (slot, f) = it2.find_field(g(F_TITLE)).unwrap();
        assert_eq!(
            slot,
            FieldSlot::Unversioned {
                language: "da".into()
            }
        );
        assert_eq!(f.value, "da unversioned");
    }

    #[test]
    fn find_field_versions_ascending() {
        let mut it = item(realistic());
        // Remove the da occurrence so en resolves; v1 must win over v2.
        assert!(it.remove_field(
            &FieldSlot::Versioned {
                language: "da".into(),
                version: 1
            },
            g(F_TITLE)
        ));
        let (slot, f) = it.find_field(g(F_TITLE)).unwrap();
        assert_eq!(
            slot,
            FieldSlot::Versioned {
                language: "en".into(),
                version: 1
            }
        );
        assert_eq!(f.value, "Home v1");
    }

    #[test]
    fn max_version_per_language() {
        let it = item(realistic());
        assert_eq!(it.max_version("en"), Some(2));
        assert_eq!(it.max_version("da"), Some(1));
        assert_eq!(it.max_version("fr"), None);
    }

    // ---- set_field -----------------------------------------------------------

    #[test]
    fn set_field_replaces_value_keeps_hint_and_order() {
        let mut it = item(realistic());
        it.set_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 2,
            },
            g(F_TITLE),
            Some("IgnoredHint"),
            None,
            "New title",
        );
        let src = emitted(&it);
        assert!(src.contains("      Value: New title\n"), "src:\n{src}");
        assert!(src.contains("Hint: Title"), "existing hint preserved");
        assert!(!src.contains("IgnoredHint"), "hint not rewritten");
        assert_stable(&it);
    }

    #[test]
    fn set_field_updates_type_when_supplied() {
        let mut it = item(realistic());
        it.set_field(
            &FieldSlot::Shared,
            g(F_RELATED),
            None,
            Some("TreelistEx"),
            "{C0FFEE00-0002-4000-8000-000000000002}",
        );
        let shared = it.shared_fields();
        assert_eq!(shared[0].type_hint.as_deref(), Some("TreelistEx"));
        assert_eq!(shared[0].value, "{C0FFEE00-0002-4000-8000-000000000002}");
        assert_stable(&it);
    }

    #[test]
    fn set_field_new_value_style_follows_write_rules() {
        let mut it = item(realistic());
        // Braced GUID => quoted (write rule 4).
        it.set_field(
            &FieldSlot::Shared,
            g(F_RELATED),
            None,
            None,
            "{C0FFEE00-0002-4000-8000-000000000002}",
        );
        let src = emitted(&it);
        assert!(
            src.contains("  Value: \"{C0FFEE00-0002-4000-8000-000000000002}\"\n"),
            "src:\n{src}"
        );
        assert_stable(&it);
    }

    #[test]
    fn set_field_inserts_new_field_guid_sorted() {
        let mut it = item(realistic());
        let lo = g("00000000-0000-4000-8000-000000000001");
        let hi = g("ffffffff-ffff-4fff-8fff-ffffffffffff");
        it.set_field(&FieldSlot::Shared, hi, Some("ZField"), None, "z");
        it.set_field(&FieldSlot::Shared, lo, Some("AField"), None, "a");
        let shared = it.shared_fields();
        let ids: Vec<Guid> = shared.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![lo, g(F_RELATED), hi]);
        assert_stable(&it);
    }

    #[test]
    fn set_field_new_field_canonical_entry_order() {
        let mut it = item(realistic());
        let id = g("11111111-1111-4111-8111-111111111111");
        it.set_field(
            &FieldSlot::Shared,
            id,
            Some("Fresh"),
            Some("Single-Line Text"),
            "v",
        );
        let src = emitted(&it);
        let want = "- ID: \"11111111-1111-4111-8111-111111111111\"\n  Hint: Fresh\n  Type: Single-Line Text\n  Value: v\n";
        assert!(src.contains(want), "src:\n{src}");
        assert_stable(&it);
    }

    #[test]
    fn set_field_creates_shared_section_in_canonical_position() {
        let mut it = item(concat!(
            "---\n",
            "ID: \"c0ffee00-0001-4000-8000-000000000001\"\n",
            "Path: /a\n",
            "Languages:\n",
            "- Language: en\n",
            "  Versions:\n",
            "  - Version: 1\n",
            "    Fields:\n",
            "    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n",
            "      Value: t\n",
        ));
        it.set_field(
            &FieldSlot::Shared,
            g(F_RELATED),
            Some("RelatedPages"),
            None,
            "x",
        );
        let keys: Vec<&str> = it.doc.root.iter().map(|e| e.key.as_str()).collect();
        // SharedFields lands after Path, before Languages.
        assert_eq!(keys, ["ID", "Path", "SharedFields", "Languages"]);
        assert_stable(&it);
    }

    #[test]
    fn set_field_creates_language_alphabetically() {
        let mut it = item(realistic());
        it.set_field(
            &FieldSlot::Unversioned {
                language: "de".into(),
            },
            g(F_NAV),
            Some("NavTitle"),
            None,
            "Startseite",
        );
        // The new language inserts before the first existing language whose
        // code is greater ("en"); the pre-existing (non-alphabetical)
        // serialized order of en/da is preserved as parsed.
        let langs = it.languages();
        let order: Vec<&str> = langs.iter().map(|l| l.language.as_str()).collect();
        assert_eq!(order, ["de", "en", "da"], "inserted before first greater");
        assert_eq!(it.languages()[0].unversioned[0].value, "Startseite");
        assert_stable(&it);
    }

    #[test]
    fn set_field_creates_language_and_version_scaffolding() {
        let mut it = item("---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nPath: /a\n");
        it.set_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 1,
            },
            g(F_TITLE),
            Some("Title"),
            None,
            "Hello",
        );
        let src = emitted(&it);
        assert_eq!(
            src,
            concat!(
                "---\n",
                "ID: \"c0ffee00-0001-4000-8000-000000000001\"\n",
                "Path: /a\n",
                "Languages:\n",
                "- Language: en\n",
                "  Versions:\n",
                "  - Version: 1\n",
                "    Fields:\n",
                "    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n",
                "      Hint: Title\n",
                "      Value: Hello\n",
            )
        );
        assert_stable(&it);
    }

    #[test]
    fn set_field_creates_version_numerically_sorted() {
        let mut it = item(realistic());
        it.set_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 4,
            },
            g(F_TITLE),
            Some("Title"),
            None,
            "v4",
        );
        it.set_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 3,
            },
            g(F_TITLE),
            Some("Title"),
            None,
            "v3",
        );
        let langs = it.languages();
        let en = langs.iter().find(|l| l.language == "en").unwrap();
        let nums: Vec<u32> = en.versions.iter().map(|(n, _)| *n).collect();
        assert_eq!(nums, [1, 2, 3, 4]);
        assert_eq!(it.max_version("en"), Some(4));
        assert_stable(&it);
    }

    #[test]
    fn set_field_unversioned_creates_fields_before_versions() {
        let mut it = item(realistic());
        it.set_field(
            &FieldSlot::Unversioned {
                language: "da".into(),
            },
            g(F_NAV),
            Some("NavTitle"),
            None,
            "Hjem nav",
        );
        // da's Fields entry must appear between Language and Versions.
        let src = emitted(&it);
        let da = src.split("- Language: da\n").nth(1).unwrap();
        let fields_pos = da.find("  Fields:").unwrap();
        let versions_pos = da.find("  Versions:").unwrap();
        assert!(fields_pos < versions_pos, "src:\n{src}");
        assert_stable(&it);
    }

    // ---- remove_field / pruning -----------------------------------------------

    #[test]
    fn remove_field_prunes_shared_section() {
        let mut it = item(realistic());
        assert!(it.remove_field(&FieldSlot::Shared, g(F_RELATED)));
        assert!(!it.doc.root.iter().any(|e| e.key == "SharedFields"));
        // second removal reports false
        assert!(!it.remove_field(&FieldSlot::Shared, g(F_RELATED)));
        assert_stable(&it);
    }

    #[test]
    fn remove_field_prunes_version_and_language() {
        let mut it = item(realistic());
        let slot = FieldSlot::Versioned {
            language: "da".into(),
            version: 1,
        };
        assert!(it.remove_field(&slot, g(F_TITLE)));
        // da had only that one field in its only version: whole language gone.
        assert!(it.languages().iter().all(|l| l.language != "da"));
        assert_eq!(it.max_version("da"), None);
        assert_stable(&it);
    }

    #[test]
    fn remove_field_keeps_language_with_other_content() {
        let mut it = item(realistic());
        assert!(it.remove_field(
            &FieldSlot::Unversioned {
                language: "en".into()
            },
            g(F_NAV)
        ));
        let langs = it.languages();
        let en = langs.iter().find(|l| l.language == "en").unwrap();
        assert!(en.unversioned.is_empty());
        assert_eq!(en.versions.len(), 2, "versions untouched");
        assert_stable(&it);
    }

    #[test]
    fn remove_field_prunes_languages_entry_when_all_gone() {
        let mut it = item(realistic());
        it.remove_field(
            &FieldSlot::Versioned {
                language: "da".into(),
                version: 1,
            },
            g(F_TITLE),
        );
        it.remove_field(
            &FieldSlot::Unversioned {
                language: "en".into(),
            },
            g(F_NAV),
        );
        it.remove_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 1,
            },
            g(F_TITLE),
        );
        it.remove_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 2,
            },
            g(F_TITLE),
        );
        assert!(!it.doc.root.iter().any(|e| e.key == "Languages"));
        assert_stable(&it);
    }

    #[test]
    fn remove_field_missing_slot_is_false() {
        let mut it = item(realistic());
        assert!(!it.remove_field(
            &FieldSlot::Unversioned {
                language: "fr".into()
            },
            g(F_NAV)
        ));
        assert!(!it.remove_field(
            &FieldSlot::Versioned {
                language: "en".into(),
                version: 9
            },
            g(F_TITLE)
        ));
        assert_eq!(emitted(&it), realistic(), "no-op removals change nothing");
    }

    // ---- ensure_version ---------------------------------------------------------

    #[test]
    fn ensure_version_creates_language_and_version() {
        let mut it = item("---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\n");
        it.ensure_version("en", 1);
        assert_eq!(
            emitted(&it),
            concat!(
                "---\n",
                "ID: \"c0ffee00-0001-4000-8000-000000000001\"\n",
                "Languages:\n",
                "- Language: en\n",
                "  Versions:\n",
                "  - Version: 1\n",
            )
        );
        assert_stable(&it);
    }

    #[test]
    fn ensure_version_is_idempotent_and_sorted() {
        let mut it = item(realistic());
        it.ensure_version("en", 2); // exists: no change
        assert_eq!(emitted(&it), realistic());
        it.ensure_version("en", 3);
        let langs = it.languages();
        let en = langs.iter().find(|l| l.language == "en").unwrap();
        let nums: Vec<u32> = en.versions.iter().map(|(n, _)| *n).collect();
        assert_eq!(nums, [1, 2, 3]);
        assert_stable(&it);
    }

    // ---- set_path / set_parent ---------------------------------------------------

    #[test]
    fn set_path_replaces_and_creates() {
        let mut it = item(realistic());
        it.set_path("/sitecore/content/Moved");
        assert_eq!(it.path().as_deref(), Some("/sitecore/content/Moved"));
        assert_stable(&it);

        let mut bare = item("---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nDB: master\n");
        bare.set_path("/sitecore/content/New");
        let keys: Vec<&str> = bare.doc.root.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["ID", "Path", "DB"], "Path inserted before DB");
        assert_stable(&bare);
    }

    #[test]
    fn set_parent_quoted_rainbow_form() {
        let mut it = item(realistic());
        let p = g("bbbbbbbb-0000-4000-8000-0000000000bb");
        it.set_parent(p);
        assert_eq!(it.parent_id(), Some(p));
        assert!(emitted(&it).contains("Parent: \"bbbbbbbb-0000-4000-8000-0000000000bb\"\n"));
        assert_stable(&it);

        let mut bare = item("---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nPath: /a\n");
        bare.set_parent(p);
        let keys: Vec<&str> = bare.doc.root.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["ID", "Parent", "Path"], "Parent inserted after ID");
        assert_stable(&bare);
    }
}
