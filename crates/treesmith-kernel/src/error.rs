//! Kernel error taxonomy (DESIGN.md §8): four classes mapping 1:1 to the
//! spec §3.2 exit codes, each serializable to the machine-readable error
//! JSON shape both surfaces print.

use serde_json::{json, Value};
use treesmith_graph::TreeFault;

/// Why a kernel operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum KernelError {
    /// The request itself is malformed (bad designator, unknown gate,
    /// ambiguous path, ...). Exit-2 class.
    ///
    /// `code` is a distinct machine-readable reason (e.g. `unknown-path`,
    /// `unknown-item`, `invalid-designator`, `ambiguous-path`,
    /// `unknown-template`, `unknown-gate`) so a consuming agent can branch
    /// without string-parsing `message`; `"usage"` is the generic default.
    /// This stays within DESIGN.md §8's contract (the class is `usage`; the
    /// exit code is 2), just at finer granularity than the class alone.
    #[error("{message}")]
    Usage {
        /// Machine-readable reason code; `"usage"` when unspecialized.
        code: String,
        /// Human-readable description.
        message: String,
        /// Code-dependent specifics (candidate lists, offending token, ...).
        details: Value,
    },

    /// A write or resolve was rejected with a machine-readable reason
    /// (spec I3/I5). Exit-1 class.
    #[error("{code}: {message}")]
    Validation {
        /// Machine-readable reason code, e.g. `unknown-field`,
        /// `wrong-slot-for-section`, `invalid-value`,
        /// `malformed-layout-xml`, `blob-unsupported`, `self-check-failed`.
        code: String,
        /// Human-readable description.
        message: String,
        /// Code-dependent specifics.
        details: Value,
    },

    /// The tree has recorded faults; every op except `census` refuses to
    /// run on it (spec §3.4). Exit-3 class.
    #[error("tree has {} fault(s)", .0.len())]
    TreeFault(Vec<TreeFault>),

    /// Filesystem trouble outside the tree-fault taxonomy. Exit-1 class.
    #[error("{0}")]
    Io(String),
}

impl KernelError {
    /// Builds a usage error with a distinct machine `code` and `details`
    /// (class `usage`, exit 2). Surfaces use this to report their own
    /// argument errors with the same code granularity the kernel uses.
    pub fn usage(code: &str, message: impl Into<String>, details: Value) -> KernelError {
        KernelError::Usage {
            code: code.to_string(),
            message: message.into(),
            details,
        }
    }

    /// Stable class name: `"usage" | "validation" | "tree-fault" | "io"`.
    pub fn class(&self) -> &'static str {
        match self {
            KernelError::Usage { .. } => "usage",
            KernelError::Validation { .. } => "validation",
            KernelError::TreeFault(_) => "tree-fault",
            KernelError::Io(_) => "io",
        }
    }

    /// Process exit code per spec §3.2: usage 2, validation 1,
    /// tree-fault 3, io 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            KernelError::Usage { .. } => 2,
            KernelError::Validation { .. } => 1,
            KernelError::TreeFault(_) => 3,
            KernelError::Io(_) => 1,
        }
    }

    /// The exact error JSON both surfaces emit:
    /// `{"ok":false,"error":{"class","code","message","details"}}`.
    pub fn to_json(&self) -> Value {
        let (code, message, details) = match self {
            KernelError::Usage {
                code,
                message,
                details,
            } => (code.clone(), message.clone(), details.clone()),
            KernelError::Validation {
                code,
                message,
                details,
            } => (code.clone(), message.clone(), details.clone()),
            KernelError::TreeFault(faults) => (
                "tree-fault".to_string(),
                format!("tree has {} fault(s); run census for details", faults.len()),
                json!({ "faults": faults }),
            ),
            KernelError::Io(msg) => ("io".to_string(), msg.clone(), Value::Null),
        };
        json!({
            "ok": false,
            "error": {
                "class": self.class(),
                "code": code,
                "message": message,
                "details": details,
            }
        })
    }
}

/// Shorthand for building a [`KernelError::Validation`].
pub(crate) fn validation(code: &str, message: impl Into<String>, details: Value) -> KernelError {
    KernelError::Validation {
        code: code.to_string(),
        message: message.into(),
        details,
    }
}

/// A generic usage error (machine code `"usage"`, null details) — for
/// cases with no useful programmatic branch.
pub(crate) fn usage(message: impl Into<String>) -> KernelError {
    KernelError::Usage {
        code: "usage".to_string(),
        message: message.into(),
        details: Value::Null,
    }
}

/// A usage error with a distinct machine code and details, so agents can
/// branch (`unknown-path` vs `unknown-item` vs `invalid-designator` ...)
/// without parsing the human message.
pub(crate) fn usage_coded(code: &str, message: impl Into<String>, details: Value) -> KernelError {
    KernelError::Usage {
        code: code.to_string(),
        message: message.into(),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_usage_keeps_class_and_exit_but_uses_default_code() {
        let err = usage("bad request");
        assert_eq!(err.class(), "usage");
        assert_eq!(err.exit_code(), 2);
        assert_eq!(err.to_json()["error"]["code"], json!("usage"));
    }

    #[test]
    fn coded_usage_surfaces_its_distinct_code_and_details() {
        let err = usage_coded(
            "unknown-path",
            "no such path",
            json!({ "path": "/sitecore/x" }),
        );
        // Still class `usage`, exit 2 — DESIGN.md §8 contract preserved.
        assert_eq!(err.class(), "usage");
        assert_eq!(err.exit_code(), 2);
        let j = err.to_json();
        assert_eq!(j["error"]["code"], json!("unknown-path"));
        assert_eq!(j["error"]["details"]["path"], json!("/sitecore/x"));
    }
}
