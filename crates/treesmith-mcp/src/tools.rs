//! `tools/call` dispatch: maps a tool name + camelCase arguments object to
//! a [`Workspace`] operation, then renders the outcome as the text payload
//! the CLI would print, plus the `isError` flag (DESIGN.md §10).
//!
//! `isError` is `true` for kernel errors **and** for `validate` when the
//! gate report has errors. Kernel errors are returned as their
//! `to_json()` payload (machine-readable), never as a protocol error.

use serde_json::Value;
use treesmith_kernel::{ForgeRequest, KernelError, MoveRequest, SetFieldRequest, Workspace};
use treesmith_types::Guid;

/// Runs the named tool against `workspace` with the given `arguments`
/// object, returning `(text, is_error)`.
pub fn dispatch(workspace: &mut Workspace, name: &str, arguments: &Value) -> (String, bool) {
    let outcome = run(workspace, name, arguments);
    match outcome {
        Ok((value, is_error)) => (render(&value), is_error),
        Err(err) => (render(&err.to_json()), true),
    }
}

/// Executes the op, returning `(json, is_error)` on success paths (where a
/// gate failure is still `Ok` with `is_error = true`) or a [`KernelError`].
fn run(workspace: &mut Workspace, name: &str, args: &Value) -> Result<(Value, bool), KernelError> {
    match name {
        "query" => Ok((workspace.query(&req_str(args, "expr")?)?, false)),
        "get" => Ok((workspace.get(&req_str(args, "item")?)?, false)),
        "set_field" => {
            let req = SetFieldRequest {
                item: req_str(args, "item")?,
                field: req_str(args, "field")?,
                value: req_str(args, "value")?,
                language: opt_str(args, "language"),
                version: opt_u32(args, "version")?,
                create_version: opt_bool(args, "createVersion").unwrap_or(true),
            };
            Ok((workspace.set_field(&req)?, false))
        }
        "forge" => {
            let id = match opt_str(args, "id") {
                Some(raw) => Some(
                    Guid::parse(&raw)
                        .map_err(|e| KernelError::Usage(format!("invalid id `{raw}`: {e}")))?,
                ),
                None => None,
            };
            let req = ForgeRequest {
                template: req_str(args, "template")?,
                parent: req_str(args, "parent")?,
                name: req_str(args, "name")?,
                id,
                language: opt_str(args, "language"),
            };
            Ok((workspace.forge(&req)?, false))
        }
        "move" => {
            let req = MoveRequest {
                item: req_str(args, "item")?,
                new_parent: req_str(args, "newParent")?,
                name: opt_str(args, "name"),
            };
            Ok((workspace.move_item(&req)?, false))
        }
        "resolve_presentation" => {
            let value = workspace.resolve_presentation(
                &req_str(args, "item")?,
                opt_str(args, "language").as_deref(),
                opt_u32(args, "version")?,
            )?;
            Ok((value, false))
        }
        "validate" => {
            let gates = opt_str_vec(args, "gates")?;
            let (value, has_errors) = workspace.validate(gates.as_deref())?;
            // isError mirrors the CLI exit-1 posture: validate-with-errors.
            Ok((value, has_errors))
        }
        "census" => Ok((Workspace::census(workspace.root()), false)),
        other => Err(KernelError::Usage(format!("unknown tool `{other}`"))),
    }
}

/// The exact JSON string the CLI prints for a payload. Compact, so the
/// tool text is one machine-readable line.
fn render(value: &Value) -> String {
    serde_json::to_string(value).expect("json value serializes")
}

fn arg_missing(field: &str) -> KernelError {
    KernelError::Usage(format!("missing required argument `{field}`"))
}

fn req_str(args: &Value, field: &str) -> Result<String, KernelError> {
    match args.get(field) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(KernelError::Usage(format!(
            "argument `{field}` must be a string"
        ))),
        None => Err(arg_missing(field)),
    }
}

fn opt_str(args: &Value, field: &str) -> Option<String> {
    match args.get(field) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn opt_bool(args: &Value, field: &str) -> Option<bool> {
    args.get(field).and_then(Value::as_bool)
}

fn opt_u32(args: &Value, field: &str) -> Result<Option<u32>, KernelError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                KernelError::Usage(format!("argument `{field}` must be a non-negative integer"))
            }),
    }
}

fn opt_str_vec(args: &Value, field: &str) -> Result<Option<Vec<String>>, KernelError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    _ => {
                        return Err(KernelError::Usage(format!(
                            "argument `{field}` must be an array of strings"
                        )))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(KernelError::Usage(format!(
            "argument `{field}` must be an array of strings"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_string_extraction() {
        let args = json!({ "expr": "path:/x" });
        assert_eq!(req_str(&args, "expr").unwrap(), "path:/x");
        assert!(req_str(&args, "missing").is_err());
        assert!(req_str(&json!({ "expr": 5 }), "expr").is_err());
    }

    #[test]
    fn optional_version_parsing() {
        assert_eq!(opt_u32(&json!({}), "version").unwrap(), None);
        assert_eq!(
            opt_u32(&json!({ "version": 3 }), "version").unwrap(),
            Some(3)
        );
        assert!(opt_u32(&json!({ "version": -1 }), "version").is_err());
    }

    #[test]
    fn optional_gates_vector() {
        assert_eq!(opt_str_vec(&json!({}), "gates").unwrap(), None);
        assert_eq!(
            opt_str_vec(&json!({ "gates": ["G1", "G2"] }), "gates").unwrap(),
            Some(vec!["G1".to_string(), "G2".to_string()])
        );
        assert!(opt_str_vec(&json!({ "gates": [1] }), "gates").is_err());
    }

    #[test]
    fn create_version_defaults_true_only_via_dispatch_path() {
        assert_eq!(opt_bool(&json!({}), "createVersion"), None);
        assert_eq!(
            opt_bool(&json!({ "createVersion": false }), "createVersion"),
            Some(false)
        );
    }
}
