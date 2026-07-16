//! The pure JSON-RPC 2.0 protocol layer (DESIGN.md §10): request routing,
//! error codes, and the `initialize` protocol-version echo, all decided
//! from `serde_json::Value`s with no I/O and no `Workspace`. Kept pure so
//! the routing, error, and version logic is unit-testable without spawning
//! a process or touching disk.

use serde_json::{json, Value};

/// Protocol versions this server understands. `initialize` echoes the
/// client's requested version when it is one of these, else falls back to
/// [`LATEST_PROTOCOL_VERSION`].
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// The version returned when the client requests an unknown (or missing)
/// protocol version.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error code: method not found.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error code: parse error (malformed JSON).
pub const PARSE_ERROR: i64 = -32700;

/// The name reported in `serverInfo`.
pub const SERVER_NAME: &str = "treesmith";

/// Chooses the `protocolVersion` to return from an `initialize` request:
/// echo the client's value when supported, else the latest.
pub fn negotiate_protocol_version(params: &Value) -> &'static str {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|v| *v == requested)
        .unwrap_or(LATEST_PROTOCOL_VERSION)
}

/// The `initialize` result payload (DESIGN.md §10).
pub fn initialize_result(params: &Value) -> Value {
    json!({
        "protocolVersion": negotiate_protocol_version(params),
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// The `tools/list` result: the eight tools with their JSON-Schema input
/// schemas (camelCase properties, DESIGN.md §10).
pub fn tools_list_result() -> Value {
    json!({ "tools": tool_definitions() })
}

fn obj_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn str_prop() -> Value {
    json!({ "type": "string" })
}

fn tool_definitions() -> Vec<Value> {
    let int_prop = json!({ "type": "integer" });
    let bool_prop = json!({ "type": "boolean" });
    vec![
        tool(
            "query",
            "Path/template/field predicates over the item graph.",
            obj_schema(json!({ "expr": str_prop() }), &["expr"]),
        ),
        tool(
            "get",
            "Fetch an item with its resolved effective fields.",
            obj_schema(json!({ "item": str_prop() }), &["item"]),
        ),
        tool(
            "set_field",
            "Single-field, template-validated mutation.",
            obj_schema(
                json!({
                    "item": str_prop(),
                    "field": str_prop(),
                    "value": str_prop(),
                    "language": str_prop(),
                    "version": int_prop,
                    "createVersion": bool_prop,
                }),
                &["item", "field", "value"],
            ),
        ),
        tool(
            "forge",
            "Create an item from a template (GUID-safe, section-correct).",
            obj_schema(
                json!({
                    "template": str_prop(),
                    "parent": str_prop(),
                    "name": str_prop(),
                    "id": str_prop(),
                    "language": str_prop(),
                }),
                &["template", "parent", "name"],
            ),
        ),
        tool(
            "move",
            "Structure-safe relocation with path/reference updates.",
            obj_schema(
                json!({
                    "item": str_prop(),
                    "newParent": str_prop(),
                    "name": str_prop(),
                }),
                &["item", "newParent"],
            ),
        ),
        tool(
            "resolve_presentation",
            "Resolve the placeholder/rendering tree with datasources and code files.",
            obj_schema(
                json!({
                    "item": str_prop(),
                    "language": str_prop(),
                    "version": json!({ "type": "integer" }),
                }),
                &["item"],
            ),
        ),
        tool(
            "validate",
            "Run the deterministic gate engine.",
            obj_schema(
                json!({
                    "gates": { "type": "array", "items": str_prop() },
                }),
                &[],
            ),
        ),
        tool(
            "census",
            "Round-trip fidelity census over the tree.",
            obj_schema(json!({}), &[]),
        ),
    ]
}

/// A successful JSON-RPC response envelope for request `id`.
pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC error response envelope for request `id`.
pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

/// A parse-error response (`-32700`, null id) for a line that is not valid
/// JSON at all.
pub fn parse_error() -> Value {
    error(Value::Null, PARSE_ERROR, "Parse error")
}

/// What the router decided a single incoming message becomes.
#[derive(Debug, PartialEq)]
pub enum Routed {
    /// Send this response value back to the client.
    Respond(Value),
    /// A notification (no `id`) — never answered.
    Ignore,
    /// A `tools/call` request; the caller must run the tool and build the
    /// tool-result envelope for `id`.
    ToolCall {
        /// The request id to answer.
        id: Value,
        /// The requested tool name.
        name: String,
        /// The `arguments` object (defaults to an empty object).
        arguments: Value,
    },
}

/// Routes one already-parsed JSON-RPC message. Pure: `initialize`,
/// `notifications/initialized`, `ping`, `tools/list`, and unknown-method
/// (`-32601`) are decided here; `tools/call` is surfaced for the I/O layer
/// to execute against the workspace.
///
/// A message with no `id` is a notification and is never answered
/// (DESIGN.md §10) — including an unknown-method notification.
pub fn route(message: &Value) -> Routed {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let is_notification = id.is_none();

    match method {
        "initialize" => match id {
            Some(id) => Routed::Respond(success(id, initialize_result(&params))),
            None => Routed::Ignore,
        },
        "notifications/initialized" => Routed::Ignore,
        "ping" => match id {
            Some(id) => Routed::Respond(success(id, json!({}))),
            None => Routed::Ignore,
        },
        "tools/list" => match id {
            Some(id) => Routed::Respond(success(id, tools_list_result())),
            None => Routed::Ignore,
        },
        "tools/call" => match id {
            Some(id) => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Routed::ToolCall {
                    id,
                    name,
                    arguments,
                }
            }
            None => Routed::Ignore,
        },
        _ => {
            if is_notification {
                Routed::Ignore
            } else {
                Routed::Respond(error(
                    id.expect("checked is_notification"),
                    METHOD_NOT_FOUND,
                    &format!("Method not found: {method}"),
                ))
            }
        }
    }
}

/// Wraps a tool's produced JSON string into the `tools/call` result
/// envelope (DESIGN.md §10): a single text-content block plus `isError`.
pub fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_supported_version_by_echo() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let p = json!({ "protocolVersion": v });
            assert_eq!(negotiate_protocol_version(&p), *v);
        }
    }

    #[test]
    fn falls_back_to_latest_for_unknown_or_missing_version() {
        assert_eq!(
            negotiate_protocol_version(&json!({ "protocolVersion": "1999-01-01" })),
            LATEST_PROTOCOL_VERSION
        );
        assert_eq!(
            negotiate_protocol_version(&json!({})),
            LATEST_PROTOCOL_VERSION
        );
    }

    #[test]
    fn initialize_reports_server_identity() {
        let r = initialize_result(&json!({ "protocolVersion": "2025-06-18" }));
        assert_eq!(r["protocolVersion"], "2025-06-18");
        assert_eq!(r["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(r["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(r["capabilities"]["tools"]["listChanged"], false);
    }

    #[test]
    fn tools_list_has_the_eight_named_tools() {
        let r = tools_list_result();
        let tools = r["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("tool name"))
            .collect();
        assert_eq!(
            names,
            [
                "query",
                "get",
                "set_field",
                "forge",
                "move",
                "resolve_presentation",
                "validate",
                "census",
            ]
        );
        // Every tool carries an object input schema.
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        // A required-args tool declares its requireds.
        let set_field = tools
            .iter()
            .find(|t| t["name"] == "set_field")
            .expect("set_field tool");
        let required: Vec<&str> = set_field["inputSchema"]["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().expect("required entry"))
            .collect();
        assert_eq!(required, ["item", "field", "value"]);
    }

    #[test]
    fn routes_initialize_to_a_response() {
        let msg = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        });
        match route(&msg) {
            Routed::Respond(v) => {
                assert_eq!(v["id"], 1);
                assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
            }
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn ping_answers_empty_object() {
        let msg = json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" });
        match route(&msg) {
            Routed::Respond(v) => assert_eq!(v["result"], json!({})),
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn notifications_are_never_answered() {
        // `notifications/initialized` and any id-less message are ignored.
        let init = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(route(&init), Routed::Ignore);
        let bare_ping = json!({ "jsonrpc": "2.0", "method": "ping" });
        assert_eq!(route(&bare_ping), Routed::Ignore);
        let unknown_notif = json!({ "jsonrpc": "2.0", "method": "nope/nope" });
        assert_eq!(route(&unknown_notif), Routed::Ignore);
    }

    #[test]
    fn unknown_method_request_is_method_not_found() {
        let msg = json!({ "jsonrpc": "2.0", "id": 3, "method": "does/not/exist" });
        match route(&msg) {
            Routed::Respond(v) => {
                assert_eq!(v["id"], 3);
                assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
            }
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn tools_call_is_surfaced_with_name_and_arguments() {
        let msg = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": { "name": "query", "arguments": { "expr": "path:/x" } }
        });
        match route(&msg) {
            Routed::ToolCall {
                id,
                name,
                arguments,
            } => {
                assert_eq!(id, json!(9));
                assert_eq!(name, "query");
                assert_eq!(arguments["expr"], "path:/x");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn tools_call_defaults_missing_arguments_to_empty_object() {
        let msg = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": { "name": "census" }
        });
        match route(&msg) {
            Routed::ToolCall { arguments, .. } => assert_eq!(arguments, json!({})),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_uses_minus_32700_and_null_id() {
        let e = parse_error();
        assert_eq!(e["error"]["code"], PARSE_ERROR);
        assert!(e["id"].is_null());
    }

    #[test]
    fn tool_result_wraps_text_and_is_error_flag() {
        let r = tool_result("{\"ok\":true}".to_string(), false);
        assert_eq!(r["content"][0]["type"], "text");
        assert_eq!(r["content"][0]["text"], "{\"ok\":true}");
        assert_eq!(r["isError"], false);
    }
}
