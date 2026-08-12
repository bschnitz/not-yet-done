//! Runtime reflection of a type — the single source of truth, projected.
//!
//! `#[derive(Buildable)]` emits a [`TypeSchema`] describing a config type: for
//! a struct its fields (docs, defaults, optionality, shape), for an enum its
//! variants and serde tagging. Every frontend (YAML templates, interactive
//! builders, …) consumes this schema, so the type definition never has a rival
//! source of truth.

/// A reflected type produced by `#[derive(Buildable)]`.
#[derive(Debug, Clone)]
pub enum TypeSchema {
    /// A product type — a fixed set of named fields.
    Struct(StructSchema),
    /// A sum type — a choice between named variants.
    Enum(EnumSchema),
}

impl TypeSchema {
    /// The type's Rust name.
    pub fn name(&self) -> &'static str {
        match self {
            TypeSchema::Struct(s) => s.name,
            TypeSchema::Enum(e) => e.name,
        }
    }

    /// Doc comment on the type, if any.
    pub fn doc(&self) -> Option<&'static str> {
        match self {
            TypeSchema::Struct(s) => s.doc,
            TypeSchema::Enum(e) => e.doc,
        }
    }

    /// Borrow as a struct schema, if this is one.
    pub fn as_struct(&self) -> Option<&StructSchema> {
        match self {
            TypeSchema::Struct(s) => Some(s),
            TypeSchema::Enum(_) => None,
        }
    }

    /// Borrow as an enum schema, if this is one.
    pub fn as_enum(&self) -> Option<&EnumSchema> {
        match self {
            TypeSchema::Enum(e) => Some(e),
            TypeSchema::Struct(_) => None,
        }
    }
}

/// A reflected struct.
#[derive(Debug, Clone)]
pub struct StructSchema {
    /// The struct's Rust name.
    pub name: &'static str,
    /// Doc comment on the struct, if any (lines joined with `\n`).
    pub doc: Option<&'static str>,
    /// Fields in declaration order.
    pub fields: Vec<FieldSchema>,
}

/// One field of a reflected struct.
#[derive(Debug, Clone)]
pub struct FieldSchema {
    /// Serialized key — honours `#[serde(rename)]` / `#[serde(rename_all)]`.
    pub key: &'static str,
    /// Doc comment (a `///` line or a `#[builder(doc = "…")]` override).
    pub doc: Option<&'static str>,
    /// `true` when the field is `Option<_>`: absence is valid and defaults to
    /// `None` unless a default is given.
    pub optional: bool,
    /// Canonical string form of the default, if any. Parseable via the field's
    /// `FromStr`; doubles as the placeholder shown in templates and prompts.
    pub default: Option<&'static str>,
    /// The field's concrete Rust type name, `Option`/`Vec`-unwrapped — e.g.
    /// `u32`, `String`, `DbConfig`. [`ScalarHint`] collapses every integer to
    /// `Int`, so this preserves the exact type to show next to the key in
    /// interactive prompts (`port (u32)`).
    pub type_name: &'static str,
    /// The field's shape.
    pub kind: Kind,
}

/// A reflected enum.
#[derive(Debug, Clone)]
pub struct EnumSchema {
    /// The enum's Rust name.
    pub name: &'static str,
    /// Doc comment on the enum, if any.
    pub doc: Option<&'static str>,
    /// How serde discriminates the variants on the wire.
    pub tag: EnumTag,
    /// Variants in declaration order.
    pub variants: Vec<VariantSchema>,
}

/// How an enum's variant is discriminated in serialized form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumTag {
    /// Serde's default: `{ variant: data }` (or the bare variant name for a
    /// unit variant).
    External,
    /// `#[serde(tag = "…")]`: the variant name rides inside the object under
    /// this key alongside the variant's own fields.
    Internal(&'static str),
}

/// One variant of a reflected enum.
#[derive(Debug, Clone)]
pub struct VariantSchema {
    /// Serialized name — honours `#[serde(rename)]` / `#[serde(rename_all)]`.
    pub name: &'static str,
    /// Doc comment on the variant, if any.
    pub doc: Option<&'static str>,
    /// The variant's payload shape.
    pub kind: VariantKind,
}

/// The payload shape of an enum variant.
#[derive(Debug, Clone)]
pub enum VariantKind {
    /// No payload: `Env`.
    Unit,
    /// A single unnamed field: `Wrapped(String)`.
    Newtype(Box<Kind>),
    /// Named fields: `Command { script, timeout_secs }`.
    Struct(Vec<FieldSchema>),
}

/// The shape of a value.
#[derive(Debug, Clone)]
pub enum Kind {
    /// A leaf entered as a single line; the type implements `FromStr`.
    Scalar(ScalarHint),
    /// A nested `Buildable` type — recurse into its own schema.
    Nested(TypeSchema),
    /// A homogeneous sequence — repeat the inner shape.
    List(Box<Kind>),
}

/// A hint about a scalar's underlying type, for prompts and placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarHint {
    /// A textual value (`String`).
    Str,
    /// A boolean.
    Bool,
    /// An integer.
    Int,
    /// A floating-point number.
    Float,
    /// A `FromStr` type flagged `#[builder(leaf)]` that is not a known
    /// primitive; carries the type name for display.
    Other(&'static str),
}

impl ScalarHint {
    /// A short angle-bracket placeholder for templates, e.g. `<string>`.
    pub fn placeholder(&self) -> &'static str {
        match self {
            ScalarHint::Str => "<string>",
            ScalarHint::Bool => "<true|false>",
            ScalarHint::Int => "<int>",
            ScalarHint::Float => "<float>",
            ScalarHint::Other(_) => "<value>",
        }
    }
}
