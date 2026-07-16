//! The seven gate implementations (DESIGN.md §7). Each pushes findings
//! into a shared vector; the driver sorts and dedups afterwards, so the
//! gates only need per-item determinism.

use serde_json::json;
use treesmith_presentation::{
    parse_xml, rendering_code, CodeKind, DatasourceResolution, DeltaNote, PresentationError,
};
use treesmith_types::Guid;

use crate::{
    all_fields, effective_field_type, field_display_name, finding_for, for_each_resolution,
    is_general_link_type, is_reference_type, is_wellknown_field, is_wellknown_template, path_under,
    slot_label, slot_section, Finding, GateCtx, Severity,
};

/// G1 — datasources: every rendering's datasource in every layout layer
/// (shared + final, all languages/versions) must resolve.
///
/// Codes: `g1.missing-datasource` (error), `g1.dynamic-datasource` (info).
pub(crate) fn g1_datasources(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for_each_resolution(ctx, |node, result| {
        let Ok(rp) = result else {
            return; // malformed layout XML is G2's finding
        };
        for device in &rp.devices {
            for rendering in &device.renderings {
                let rendering_label = rendering
                    .rendering_name
                    .as_deref()
                    .or(rendering.rendering_id.as_deref())
                    .or(rendering.uid.as_deref())
                    .unwrap_or("<anonymous>")
                    .to_string();
                match &rendering.datasource {
                    DatasourceResolution::Missing { raw } => out.push(finding_for(
                        ctx,
                        "G1",
                        "g1.missing-datasource",
                        Severity::Error,
                        Some(node.id),
                        format!(
                            "rendering {rendering_label} in device {} has an unresolvable \
                             datasource `{raw}`",
                            device.device_id
                        ),
                        json!({
                            "device": device.device_id,
                            "uid": rendering.uid,
                            "renderingId": rendering.rendering_id,
                            "datasource": raw,
                        }),
                    )),
                    DatasourceResolution::Dynamic { raw, scheme } => out.push(finding_for(
                        ctx,
                        "G1",
                        "g1.dynamic-datasource",
                        Severity::Info,
                        Some(node.id),
                        format!(
                            "rendering {rendering_label} in device {} has a dynamic `{scheme}:` \
                             datasource `{raw}` that static analysis cannot follow",
                            device.device_id
                        ),
                        json!({
                            "device": device.device_id,
                            "uid": rendering.uid,
                            "renderingId": rendering.rendering_id,
                            "datasource": raw,
                            "scheme": scheme,
                        }),
                    )),
                    DatasourceResolution::ContextItem | DatasourceResolution::Item { .. } => {}
                }
            }
        }
    });
}

/// G2 — layout XML: layers must parse, and delta merges must be clean.
///
/// Codes: `g2.malformed-xml` (error), `g2.unknown-uid` (error),
/// `g2.bad-position-ref` (error), `g2.device-without-layout` (warning).
pub(crate) fn g2_layout_xml(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for_each_resolution(ctx, |node, result| match result {
        Err(PresentationError::MalformedLayoutXml { item, field, error }) => {
            out.push(finding_for(
                ctx,
                "G2",
                "g2.malformed-xml",
                Severity::Error,
                Some(*item),
                format!("{field} does not parse as layout XML: {}", error.message),
                json!({ "field": field, "offset": error.offset }),
            ));
        }
        Err(PresentationError::ItemNotFound(_)) => {}
        Ok(rp) => {
            for device in &rp.devices {
                for note in &device.notes {
                    out.push(delta_note_finding(ctx, node.id, note));
                }
            }
        }
    });
}

fn delta_note_finding(ctx: &GateCtx, item: Guid, note: &DeltaNote) -> Finding {
    match note {
        DeltaNote::UnknownUid { device, uid } => finding_for(
            ctx,
            "G2",
            "g2.unknown-uid",
            Severity::Error,
            Some(item),
            format!(
                "final-renderings delta targets uid `{uid}` in device {device}, but no such \
                 rendering exists in the shared layout"
            ),
            json!({ "device": device, "uid": uid }),
        ),
        DeltaNote::BadPositionRef { device, expr } => finding_for(
            ctx,
            "G2",
            "g2.bad-position-ref",
            Severity::Error,
            Some(item),
            format!(
                "final-renderings delta in device {device} has an unresolvable position \
                 selector `{expr}`; the rendering was appended"
            ),
            json!({ "device": device, "expr": expr }),
        ),
        DeltaNote::DeviceWithoutLayout { device } => finding_for(
            ctx,
            "G2",
            "g2.device-without-layout",
            Severity::Warning,
            Some(item),
            format!("delta device {device} has no shared counterpart and no `l=` layout"),
            json!({ "device": device }),
        ),
    }
}

/// G3 — code files: view/layout `Path` must match a repo `.cshtml`;
/// controller class must be declared in a repo `.cs`.
///
/// Codes: `g3.missing-view` (error), `g3.missing-controller` (warning),
/// `g3.empty-path` (warning).
pub(crate) fn g3_code_files(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for id in ctx.graph.ids_by_path() {
        let Some(node) = ctx.graph.get(id) else {
            continue;
        };
        let Some(code) = rendering_code(ctx.graph, ctx.templates, node) else {
            continue;
        };
        let kind_str = match code.kind {
            CodeKind::View => "view",
            CodeKind::Controller => "controller",
            CodeKind::Layout => "layout",
        };
        if code.raw.trim().is_empty() {
            let field = match code.kind {
                CodeKind::Controller => "Controller",
                CodeKind::View | CodeKind::Layout => "Path",
            };
            out.push(finding_for(
                ctx,
                "G3",
                "g3.empty-path",
                Severity::Warning,
                Some(id),
                format!("{kind_str} rendering has an empty `{field}` field"),
                json!({ "kind": kind_str, "raw": code.raw }),
            ));
            continue;
        }
        if !code.files.is_empty() {
            continue;
        }
        match code.kind {
            CodeKind::View | CodeKind::Layout => out.push(finding_for(
                ctx,
                "G3",
                "g3.missing-view",
                Severity::Error,
                Some(id),
                format!(
                    "{kind_str} file `{}` does not exist in the repository",
                    code.raw.trim()
                ),
                json!({ "kind": kind_str, "raw": code.raw }),
            )),
            CodeKind::Controller => out.push(finding_for(
                ctx,
                "G3",
                "g3.missing-controller",
                Severity::Warning,
                Some(id),
                format!(
                    "no repository `.cs` file declares controller class `{}`",
                    code.raw.trim()
                ),
                json!({ "kind": kind_str, "raw": code.raw }),
            )),
        }
    }
}

/// G4 — placeholders: every placeholder a rendering binds to must be
/// exposed by some scanned view (static-analysis confidence → warning).
///
/// Code: `g4.placeholder-not-exposed` (warning).
pub(crate) fn g4_placeholders(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for_each_resolution(ctx, |node, result| {
        let Ok(rp) = result else {
            return;
        };
        for device in &rp.devices {
            for rendering in &device.renderings {
                let leaf = &rendering.placeholder_leaf;
                if leaf.is_empty() || ctx.placeholders.exposed.contains(leaf) {
                    continue;
                }
                let rendering_label = rendering
                    .rendering_name
                    .as_deref()
                    .or(rendering.rendering_id.as_deref())
                    .or(rendering.uid.as_deref())
                    .unwrap_or("<anonymous>")
                    .to_string();
                out.push(finding_for(
                    ctx,
                    "G4",
                    "g4.placeholder-not-exposed",
                    Severity::Warning,
                    Some(node.id),
                    format!(
                        "rendering {rendering_label} binds placeholder `{leaf}` (path `{}`), \
                         which no scanned view exposes",
                        rendering.placeholder
                    ),
                    json!({
                        "device": device.device_id,
                        "uid": rendering.uid,
                        "renderingId": rendering.rendering_id,
                        "placeholder": rendering.placeholder,
                        "leaf": leaf,
                    }),
                ));
            }
        }
    });
}

/// G5 — field references: reference-family values (and internal General
/// Link `id=`s) must point at items in the graph.
///
/// Codes: `g5.broken-reference` (error), `g5.invalid-guid-token` (error).
pub(crate) fn g5_field_refs(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for id in ctx.graph.ids_by_path() {
        let Some(node) = ctx.graph.get(id) else {
            continue;
        };
        let effective = node.meta.template.and_then(|t| ctx.templates.resolve(t));
        for (slot, field) in all_fields(&node.item) {
            let Some(field_type) = effective_field_type(effective.as_ref(), &field) else {
                continue;
            };
            let def_name = effective
                .as_ref()
                .and_then(|e| e.field_by_id(field.id))
                .map(|d| d.name.clone());
            let name = field_display_name(def_name.as_deref(), &field);
            let slot = slot_label(&slot);
            if is_reference_type(field_type) {
                let (guids, invalid) = Guid::parse_list(&field.value);
                for token in invalid {
                    out.push(finding_for(
                        ctx,
                        "G5",
                        "g5.invalid-guid-token",
                        Severity::Error,
                        Some(id),
                        format!("field `{name}` contains a token that is not a GUID: `{token}`"),
                        json!({ "field": field.id, "name": name, "slot": slot, "token": token }),
                    ));
                }
                for target in guids {
                    if ctx.graph.get(target).is_none() {
                        out.push(finding_for(
                            ctx,
                            "G5",
                            "g5.broken-reference",
                            Severity::Error,
                            Some(id),
                            format!(
                                "field `{name}` references item {} which is not serialized",
                                target.rainbow()
                            ),
                            json!({
                                "field": field.id,
                                "name": name,
                                "slot": slot,
                                "target": target,
                            }),
                        ));
                    }
                }
            } else if is_general_link_type(field_type) {
                check_general_link(ctx, out, id, &name, &slot, &field);
            }
        }
    }
}

/// Internal General Link: `<link linktype="internal" id="{...}">` — the
/// `id` must parse and resolve.
fn check_general_link(
    ctx: &GateCtx,
    out: &mut Vec<Finding>,
    item: Guid,
    name: &str,
    slot: &str,
    field: &treesmith_format::FieldRef,
) {
    let value = field.value.trim();
    if value.is_empty() {
        return;
    }
    let Ok(link) = parse_xml(value) else {
        return; // not our gate's concern; malformed link XML stays quiet
    };
    if link.attr("linktype") != Some("internal") {
        return;
    }
    let Some(raw_id) = link.attr("id").filter(|v| !v.is_empty()) else {
        return;
    };
    match Guid::parse(raw_id) {
        Err(_) => out.push(finding_for(
            ctx,
            "G5",
            "g5.invalid-guid-token",
            Severity::Error,
            Some(item),
            format!("field `{name}` has an internal link whose id `{raw_id}` is not a GUID"),
            json!({ "field": field.id, "name": name, "slot": slot, "token": raw_id }),
        )),
        Ok(target) => {
            if ctx.graph.get(target).is_none() {
                out.push(finding_for(
                    ctx,
                    "G5",
                    "g5.broken-reference",
                    Severity::Error,
                    Some(item),
                    format!(
                        "field `{name}` has an internal link to item {} which is not serialized",
                        target.rainbow()
                    ),
                    json!({ "field": field.id, "name": name, "slot": slot, "target": target }),
                ));
            }
        }
    }
}

/// G6 — template conformance: serialized fields must be defined by the
/// effective template, sit in their declared section, appear once per
/// slot, and hold values valid for their type.
///
/// Codes: `g6.unknown-field` (error), `g6.wrong-section` (error),
/// `g6.duplicate-field` (error), `g6.invalid-value` (error),
/// `g6.unresolved-template` (warning), `g6.unresolved-base` (warning).
pub(crate) fn g6_conformance(ctx: &GateCtx, out: &mut Vec<Finding>) {
    for id in ctx.graph.ids_by_path() {
        let Some(node) = ctx.graph.get(id) else {
            continue;
        };
        let Some(template) = node.meta.template else {
            continue; // a missing Template key is a graph-level concern
        };
        if is_wellknown_template(template) {
            continue; // platform templates are never serialized (§7 note)
        }
        let Some(effective) = ctx.templates.resolve(template) else {
            out.push(finding_for(
                ctx,
                "G6",
                "g6.unresolved-template",
                Severity::Warning,
                Some(id),
                format!(
                    "template {} is not serialized; conformance cannot be checked",
                    template.rainbow()
                ),
                json!({ "template": template }),
            ));
            continue;
        };
        for base in &effective.unresolved_bases {
            if is_wellknown_template(*base) {
                continue;
            }
            out.push(finding_for(
                ctx,
                "G6",
                "g6.unresolved-base",
                Severity::Warning,
                Some(id),
                format!(
                    "template `{}` inherits base template {} which is not serialized",
                    effective.name,
                    base.rainbow()
                ),
                json!({ "template": effective.id, "base": base }),
            ));
        }
        let mut seen: Vec<(String, Guid)> = Vec::new();
        for (slot, field) in all_fields(&node.item) {
            if is_wellknown_field(field.id) {
                continue;
            }
            let slot_str = slot_label(&slot);
            let key = (slot_str.clone(), field.id);
            let duplicate = seen.contains(&key);
            seen.push(key);
            let Some(def) = effective.field_by_id(field.id) else {
                out.push(finding_for(
                    ctx,
                    "G6",
                    "g6.unknown-field",
                    Severity::Error,
                    Some(id),
                    format!(
                        "field {} ({}) is not defined by effective template `{}`",
                        field.id.rainbow(),
                        field_display_name(None, &field),
                        effective.name
                    ),
                    json!({ "field": field.id, "hint": field.hint, "slot": slot_str }),
                ));
                continue;
            };
            if duplicate {
                out.push(finding_for(
                    ctx,
                    "G6",
                    "g6.duplicate-field",
                    Severity::Error,
                    Some(id),
                    format!(
                        "field `{}` is serialized more than once in slot {slot_str}",
                        def.name
                    ),
                    json!({ "field": field.id, "name": def.name, "slot": slot_str }),
                ));
            }
            let actual = slot_section(&slot);
            if def.section != actual {
                out.push(finding_for(
                    ctx,
                    "G6",
                    "g6.wrong-section",
                    Severity::Error,
                    Some(id),
                    format!(
                        "field `{}` is declared {} but serialized in the {} slot ({slot_str})",
                        def.name,
                        def.section.as_str(),
                        actual.as_str()
                    ),
                    json!({
                        "field": field.id,
                        "name": def.name,
                        "declared": def.section.as_str(),
                        "actual": actual.as_str(),
                        "slot": slot_str,
                    }),
                ));
            }
            if let Err(reason) = treesmith_template::validate_value(&def.field_type, &field.value) {
                out.push(finding_for(
                    ctx,
                    "G6",
                    "g6.invalid-value",
                    Severity::Error,
                    Some(id),
                    format!(
                        "field `{}` has an invalid `{}` value: {reason}",
                        def.name, def.field_type
                    ),
                    json!({
                        "field": field.id,
                        "name": def.name,
                        "type": def.field_type,
                        "slot": slot_str,
                        "reason": reason,
                    }),
                ));
            }
        }
    }
}

/// G7 — language policy: items under the configured paths that carry at
/// least one version must carry a version in every required language.
/// The driver already skipped this gate when no policy is configured.
///
/// Code: `g7.missing-language` (error).
pub(crate) fn g7_languages(ctx: &GateCtx, out: &mut Vec<Finding>) {
    let Some(required) = ctx.config.required_languages.as_ref() else {
        return;
    };
    for id in ctx.graph.ids_by_path() {
        let Some(node) = ctx.graph.get(id) else {
            continue;
        };
        if !ctx
            .config
            .language_paths
            .iter()
            .any(|prefix| path_under(&node.meta.path, prefix))
        {
            continue;
        }
        let has_any_version = node
            .meta
            .languages
            .iter()
            .any(|(_, versions)| !versions.is_empty());
        if !has_any_version {
            continue;
        }
        for language in required {
            let present = node
                .meta
                .languages
                .iter()
                .any(|(l, versions)| l == language && !versions.is_empty());
            if !present {
                out.push(finding_for(
                    ctx,
                    "G7",
                    "g7.missing-language",
                    Severity::Error,
                    Some(id),
                    format!("item has no version in required language `{language}`"),
                    json!({ "language": language }),
                ));
            }
        }
    }
}
