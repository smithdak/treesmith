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
    #[error("{0}")]
    Usage(String),

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
    /// Stable class name: `"usage" | "validation" | "tree-fault" | "io"`.
    pub fn class(&self) -> &'static str {
        match self {
            KernelError::Usage(_) => "usage",
            KernelError::Validation { .. } => "validation",
            KernelError::TreeFault(_) => "tree-fault",
            KernelError::Io(_) => "io",
        }
    }

    /// Process exit code per spec §3.2: usage 2, validation 1,
    /// tree-fault 3, io 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            KernelError::Usage(_) => 2,
            KernelError::Validation { .. } => 1,
            KernelError::TreeFault(_) => 3,
            KernelError::Io(_) => 1,
        }
    }

    /// The exact error JSON both surfaces emit:
    /// `{"ok":false,"error":{"class","code","message","details"}}`.
    pub fn to_json(&self) -> Value {
        let (code, message, details) = match self {
            KernelError::Usage(msg) => ("usage".to_string(), msg.clone(), Value::Null),
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
