//! Error type returned by generated builders.

use std::fmt;

/// Error returned by a generated `<Name>Builder::build()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A required field was never set and has no default.
    MissingField(&'static str),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::MissingField(name) => {
                write!(f, "required field `{name}` was not set and has no default")
            }
        }
    }
}

impl std::error::Error for BuildError {}
