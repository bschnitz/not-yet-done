//! **fieldsmith** — reflect, template, and build config structs from one derive.
//!
//! Rust has no runtime reflection: a `#[derive(Deserialize)]` struct can *read*
//! a config but cannot *describe* itself — you cannot ask it for its field
//! names, types, or docs. fieldsmith fills that gap at compile time.
//!
//! `#[derive(Buildable)]` projects a type — a struct's fields (types, docs,
//! defaults, serde renames) or an enum's variants — into a runtime
//! [`TypeSchema`]. Every frontend is a consumer of that one schema, so the type
//! definition stays the single source of truth and no view can drift from it:
//!
//! - [`yaml_template`] renders a fillable, commented YAML skeleton.
//! - a generated `<Name>Builder` gives typed setters and a checked `build()`.
//! - with the `stdin` feature, [`build_stdin`] drives an interactive,
//!   recursive builder over the same schema.
//!
//! # Example
//!
//! ```ignore
//! use fieldsmith::{Buildable, yaml_template};
//! use serde::Deserialize;
//!
//! /// Jira adapter configuration.
//! #[derive(Buildable, Deserialize)]
//! struct JiraConfig {
//!     /// Base URL of your Jira instance.
//!     #[builder(default = "https://your-jira.example.com")]
//!     url: String,
//!     /// Trust self-signed TLS certificates.
//!     #[serde(default)]
//!     #[builder(default = false)]
//!     accept_invalid_certs: bool,
//! }
//!
//! let yaml = yaml_template(&JiraConfig::schema());
//! let cfg = JiraConfigBuilder::new().url("https://jira.acme.test").build().unwrap();
//! ```
//!
//! `#[derive(Buildable)]` is meant to sit alongside `#[derive(Deserialize)]`:
//! it reads the same `#[serde(...)]` attributes so the emitted keys match what
//! Deserialize expects.

mod error;
mod schema;
mod template;

#[cfg(feature = "stdin")]
mod driver;

pub use error::BuildError;
pub use schema::{
    EnumSchema, EnumTag, FieldSchema, Kind, ScalarHint, StructSchema, TypeSchema, VariantKind,
    VariantSchema,
};
pub use template::yaml_template;

#[cfg(feature = "stdin")]
pub use driver::{
    Answer, DialoguerPrompter, DriverError, Prompter, ScriptedPrompter, build_stdin,
    build_value_stdin, build_value_with, build_value_with_ctx, build_with,
};

/// A type that can describe itself as a [`TypeSchema`].
///
/// Implemented by `#[derive(Buildable)]`. For a struct the derive additionally
/// generates a `<Name>Builder` with typed setters and a `build()` returning
/// `Result<Self, BuildError>`; an enum reflects into a schema only.
pub trait Buildable: Sized {
    /// The reflected schema for this type.
    fn schema() -> TypeSchema;
}

pub use fieldsmith_derive::Buildable;
