//! Human-readable renderings for TTY output (spec §3.2). Each function
//! consumes the exact kernel JSON `Value` (DESIGN.md §8) and writes plain
//! lines to stdout; JSON mode bypasses these entirely. Renderings are
//! best-effort and lossy on purpose — the machine contract is the JSON, so
//! these read defensively (missing keys degrade gracefully rather than
//! panic).

use std::io::Write;

use serde_json::Value;

/// Borrow a string field, or `""` when absent.
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Borrow an integer field, or `0` when absent.
fn n(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// `query` → `{"count","items":[ItemSummary]}`.
pub(crate) fn query(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    let items = v.get("items").and_then(Value::as_array).unwrap_or(&empty);
    for item in items {
        writeln!(w, "{}  {}", s(item, "id"), s(item, "path"))?;
    }
    writeln!(w, "{} item(s)", n(v, "count"))
}

/// `get` → `{"item":ItemDetail}`.
pub(crate) fn get(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let item = v.get("item").cloned().unwrap_or(Value::Null);
    writeln!(w, "id       {}", s(&item, "id"))?;
    writeln!(w, "path     {}", s(&item, "path"))?;
    writeln!(w, "name     {}", s(&item, "name"))?;
    if let Some(template) = item.get("template").filter(|t| !t.is_null()) {
        let name = template
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        writeln!(w, "template {} ({})", name, s(template, "id"))?;
    }
    if let Some(db) = item.get("db").and_then(Value::as_str) {
        writeln!(w, "db       {db}")?;
    }
    render_fields(&item, w)
}

/// The field breakdown shared by `get`: shared, then each language's
/// unversioned + versioned fields.
fn render_fields(item: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    let shared = item
        .get("sharedFields")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if !shared.is_empty() {
        writeln!(w, "shared fields:")?;
        for f in shared {
            render_field(f, w)?;
        }
    }
    let languages = item
        .get("languages")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for lang in languages {
        writeln!(w, "language {}:", s(lang, "language"))?;
        let unversioned = lang
            .get("unversioned")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        for f in unversioned {
            render_field(f, w)?;
        }
        let versions = lang
            .get("versions")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        for ver in versions {
            writeln!(w, "  v{}:", n(ver, "version"))?;
            let fields = ver
                .get("fields")
                .and_then(Value::as_array)
                .unwrap_or(&empty);
            for f in fields {
                render_field(f, w)?;
            }
        }
    }
    Ok(())
}

/// One `FieldOut` line: `  Name = value`.
fn render_field(f: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    writeln!(w, "  {} = {}", s(f, "name"), s(f, "value"))
}

/// `mutate` → `{"changedFiles":[..],"selfCheck","item":ItemDetail}`.
pub(crate) fn mutate(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    let changed = v
        .get("changedFiles")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    writeln!(w, "self-check {}", s(v, "selfCheck"))?;
    for file in changed {
        writeln!(w, "changed {}", file.as_str().unwrap_or(""))?;
    }
    if let Some(item) = v.get("item").filter(|i| !i.is_null()) {
        writeln!(w, "item {} {}", s(item, "id"), s(item, "path"))?;
    }
    Ok(())
}

/// `validate` → one line per finding plus a summary (spec §3.2 human mode).
pub(crate) fn validate(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    let findings = v
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for f in findings {
        // `G1 error g1.missing-datasource /sitecore/... — message`.
        let path = f
            .get("itemPath")
            .and_then(Value::as_str)
            .or_else(|| f.get("file").and_then(Value::as_str))
            .unwrap_or("");
        writeln!(
            w,
            "{} {} {} {} — {}",
            s(f, "gate"),
            s(f, "severity"),
            s(f, "code"),
            path,
            s(f, "message"),
        )?;
    }
    let skipped = v.get("skipped").and_then(Value::as_array).unwrap_or(&empty);
    for sk in skipped {
        writeln!(w, "{} skipped: {}", s(sk, "gate"), s(sk, "reason"))?;
    }
    writeln!(
        w,
        "{}: {} error(s), {} warning(s), {} info(s)",
        if v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            "ok"
        } else {
            "FAIL"
        },
        n(v, "errors"),
        n(v, "warnings"),
        n(v, "infos"),
    )
}

/// `census` → summary plus one line per fault / mismatch.
pub(crate) fn census(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    writeln!(
        w,
        "{} files, {} items, {} round-tripped ({} ms)",
        n(v, "files"),
        n(v, "items"),
        n(v, "roundTripOk"),
        n(v, "elapsedMs"),
    )?;
    let faults = v.get("faults").and_then(Value::as_array).unwrap_or(&empty);
    for f in faults {
        writeln!(
            w,
            "fault {} line {}: {} ({})",
            s(f, "file"),
            n(f, "line"),
            s(f, "message"),
            s(f, "kind"),
        )?;
    }
    let mismatches = v
        .get("mismatches")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for m in mismatches {
        writeln!(
            w,
            "mismatch {} first-diff line {}",
            s(m, "file"),
            n(m, "firstDiffLine"),
        )?;
    }
    writeln!(
        w,
        "{}",
        if v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            "census clean"
        } else {
            "census FAILED"
        }
    )
}

/// `resolve-presentation` → device / rendering tree summary.
pub(crate) fn resolve_presentation(v: &Value, w: &mut dyn Write) -> std::io::Result<()> {
    let empty = Vec::new();
    writeln!(
        w,
        "item {} {}  language {} version {}",
        s(v, "itemId"),
        s(v, "itemPath"),
        s(v, "language"),
        n(v, "version"),
    )?;
    let devices = v.get("devices").and_then(Value::as_array).unwrap_or(&empty);
    for device in devices {
        writeln!(w, "device {}:", s(device, "deviceId"))?;
        let renderings = device
            .get("renderings")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        for r in renderings {
            let name = r
                .get("renderingName")
                .and_then(Value::as_str)
                .or_else(|| r.get("renderingId").and_then(Value::as_str))
                .unwrap_or("<rendering>");
            writeln!(w, "  {} @ {}", name, s(r, "placeholder"))?;
        }
    }
    Ok(())
}
