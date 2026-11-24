//! The public module contract. A module is trusted compile-time Rust code that
//! an app registers explicitly. The manifest is metadata only: it grants and
//! restricts nothing, and it is not a sandbox.
//!
//! All operations in S02 are read-only by design. S04 must add explicit
//! authorization and approval boundaries before any mutating or privileged
//! operation exists.

use crate::executor::{CancelToken, ExecError, Executor};
use crate::ident::ValidationError;
use async_trait::async_trait;
use std::fmt;

/// Accepts `[a-z][a-z0-9-]{0,62}`.
fn validate_id(field: &'static str, input: &str) -> Result<(), ValidationError> {
    let b = input.as_bytes();
    if b.is_empty() {
        return Err(ValidationError::new(field, "must not be empty"));
    }
    if b.len() > 63 {
        return Err(ValidationError::new(field, "must be at most 63 characters"));
    }
    if !b[0].is_ascii_lowercase() {
        return Err(ValidationError::new(
            field,
            "must start with a lowercase letter",
        ));
    }
    if !b
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    {
        return Err(ValidationError::new(
            field,
            "only a-z, 0-9 and '-' are allowed",
        ));
    }
    Ok(())
}

macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident, $field:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(input: &str) -> Result<Self, ValidationError> {
                validate_id($field, input)?;
                Ok(Self(input.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = ValidationError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }
    };
}

id_newtype!(
    /// Identifier of a module, `[a-z][a-z0-9-]{0,62}`. Validated on construction.
    ModuleId,
    "moduleId"
);
id_newtype!(
    /// Identifier of an operation of a module. Same rules as [`ModuleId`].
    OperationId,
    "operationId"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleManifest {
    pub id: ModuleId,
    pub name: String,
    /// Version of the module crate.
    pub version: String,
    pub description: String,
    pub dependencies: Vec<ModuleId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationInfo {
    pub id: &'static str,
    pub description: &'static str,
}

/// Everything an operation may use. There is no other way to reach a server.
pub struct OperationContext<'a> {
    pub exec: &'a dyn Executor,
    pub cancel: &'a CancelToken,
    /// RFC 3339 timestamp, set by the dispatcher.
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    /// The module rejected the arguments.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error(transparent)]
    Exec(#[from] ExecError),
    /// A short, redacted message. Never raw command output.
    #[error("{0}")]
    Failed(String),
}

#[async_trait]
pub trait Module: Send + Sync {
    fn manifest(&self) -> ModuleManifest;

    fn operations(&self) -> &'static [OperationInfo];

    /// Called only with an `operation` listed in [`Module::operations`].
    /// The module validates `args` itself.
    async fn run(
        &self,
        operation: &str,
        args: serde_json::Value,
        ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, OperationError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_accept_valid_input() {
        for ok in ["a", "linux-inventory", "ports", "a1-b2"] {
            assert_eq!(ModuleId::parse(ok).unwrap().as_str(), ok);
        }
        assert!(ModuleId::parse(&"a".repeat(63)).is_ok());
    }

    #[test]
    fn module_ids_reject_bad_input() {
        let long = "a".repeat(64);
        for bad in [
            "",
            "Ports",
            "poRts",
            "a b",
            "a..b",
            "..",
            "../x",
            "1abc",
            "-a",
            "a_b",
            "a/b",
            "a\n",
            long.as_str(),
        ] {
            assert!(ModuleId::parse(bad).is_err(), "{bad:?}");
            assert!(OperationId::parse(bad).is_err(), "{bad:?}");
        }
    }
}
