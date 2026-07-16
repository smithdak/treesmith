//! JSON output shapes (DESIGN.md §8): `ItemSummary`, `ItemDetail`,
//! `FieldOut` — built with exactly the documented keys, camelCase.

use std::path::Path;

use serde_json::{json, Value};
use treesmith_format::{FieldRef, FieldSlot};
use treesmith_graph::{Graph, ItemNode};
use treesmith_template::{EffectiveTemplate, TemplateIndex};
use treesmith_types::SectionKind;

/// Root-relative forward-slash path of a file.
pub(crate) fn rel_file(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `ItemSummary` — `{"id","path","name","template","db","languages","file"}`.
pub(crate) fn item_summary(root: &Path, graph: &Graph, tix: &TemplateIndex, n: &ItemNode) -> Value {
    let template = n.meta.template.map(|t| {
        let name: Option<String> = tix
            .get(t)
            .map(|d| d.name.clone())
            .or_else(|| graph.get(t).map(|tn| tn.meta.name.clone()));
        json!({ "id": t, "name": name })
    });
    json!({
        "id": n.id,
        "path": n.meta.path,
        "name": n.meta.name,
        "template": template,
        "db": n.meta.db,
        "languages": n.meta.languages.iter()
            .map(|(l, vs)| json!({ "language": l, "versions": vs }))
            .collect::<Vec<_>>(),
        "file": rel_file(root, &n.file),
    })
}

/// `ItemDetail` — `ItemSummary` plus `templateChain`, `sharedFields`,
/// rich `languages` (with fields), and `fieldsNotInTemplate`.
pub(crate) fn item_detail(root: &Path, graph: &Graph, tix: &TemplateIndex, n: &ItemNode) -> Value {
    let effective = n.meta.template.and_then(|t| tix.resolve(t));
    let eff = effective.as_ref();

    let mut detail = item_summary(root, graph, tix, n);
    let obj = detail.as_object_mut().expect("summary is an object");

    let chain: Vec<Value> = eff
        .map(|e| e.chain.iter().map(|g| json!(g)).collect())
        .unwrap_or_default();
    obj.insert("templateChain".to_string(), Value::Array(chain));

    let shared: Vec<Value> = n
        .item
        .shared_fields()
        .iter()
        .map(|f| field_out(eff, SectionKind::Shared, f))
        .collect();
    obj.insert("sharedFields".to_string(), Value::Array(shared));

    // Rich languages replace the summary's slim list: languages sorted
    // alphabetically, versions ascending (deterministic).
    let mut languages = n.item.languages();
    languages.sort_by(|a, b| a.language.cmp(&b.language));
    let languages: Vec<Value> = languages
        .into_iter()
        .map(|mut lang| {
            lang.versions.sort_by_key(|(v, _)| *v);
            json!({
                "language": lang.language,
                "unversioned": lang.unversioned.iter()
                    .map(|f| field_out(eff, SectionKind::Unversioned, f))
                    .collect::<Vec<_>>(),
                "versions": lang.versions.iter()
                    .map(|(v, fields)| json!({
                        "version": v,
                        "fields": fields.iter()
                            .map(|f| field_out(eff, SectionKind::Versioned, f))
                            .collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    obj.insert("languages".to_string(), Value::Array(languages));

    obj.insert(
        "fieldsNotInTemplate".to_string(),
        Value::Array(fields_not_in_template(eff, n)),
    );
    detail
}

/// `FieldOut` — `{"id","name","type","section","value","definedBy"}`.
///
/// `name`: effective-template name, else hint, else the id string.
/// `type`: template type, else the serialized `Type:` hint, else null.
/// `section`: the slot the value sits in (shared/unversioned/versioned).
fn field_out(eff: Option<&EffectiveTemplate>, section: SectionKind, f: &FieldRef) -> Value {
    let def = eff.and_then(|e| e.field_by_id(f.id));
    let name = def
        .map(|d| d.name.clone())
        .or_else(|| f.hint.clone())
        .unwrap_or_else(|| f.id.rainbow());
    let field_type: Option<String> = def
        .map(|d| d.field_type.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| f.type_hint.clone());
    json!({
        "id": f.id,
        "name": name,
        "type": field_type,
        "section": section.as_str(),
        "value": f.value,
        "definedBy": def.map(|d| d.defined_by),
    })
}

/// Serialized fields whose id is absent from the effective template:
/// `[{"id","hint","slot"}]`, slot rendered as `shared`,
/// `unversioned:<lang>`, or `versioned:<lang>:<version>`.
fn fields_not_in_template(eff: Option<&EffectiveTemplate>, n: &ItemNode) -> Vec<Value> {
    let known = |f: &FieldRef| eff.is_some_and(|e| e.field_by_id(f.id).is_some());
    let mut out = Vec::new();
    for f in n.item.shared_fields() {
        if !known(&f) {
            out.push(json!({ "id": f.id, "hint": f.hint, "slot": "shared" }));
        }
    }
    let mut languages = n.item.languages();
    languages.sort_by(|a, b| a.language.cmp(&b.language));
    for lang in languages {
        for f in &lang.unversioned {
            if !known(f) {
                out.push(json!({
                    "id": f.id,
                    "hint": f.hint,
                    "slot": format!("unversioned:{}", lang.language),
                }));
            }
        }
        let mut versions = lang.versions;
        versions.sort_by_key(|(v, _)| *v);
        for (v, fields) in versions {
            for f in fields {
                if !known(&f) {
                    out.push(json!({
                        "id": f.id,
                        "hint": f.hint,
                        "slot": format!("versioned:{}:{}", lang.language, v),
                    }));
                }
            }
        }
    }
    out
}

/// Renders a [`FieldSlot`] as the compact designator used in messages.
pub(crate) fn slot_string(slot: &FieldSlot) -> String {
    match slot {
        FieldSlot::Shared => "shared".to_string(),
        FieldSlot::Unversioned { language } => format!("unversioned:{language}"),
        FieldSlot::Versioned { language, version } => format!("versioned:{language}:{version}"),
    }
}
