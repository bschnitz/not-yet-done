//! Render a [`TypeSchema`] as a fillable, commented YAML template.

use crate::schema::{EnumTag, FieldSchema, Kind, StructSchema, TypeSchema, VariantKind};

/// Render a commented YAML template for a schema.
///
/// Doc comments become `#` lines. A field shows its default (a working value)
/// or an angle-bracket `<placeholder>` when it has none. Optional scalar fields
/// are commented out (`#key: …`); optional nested blocks are flagged with an
/// `# optional` note above them.
///
/// Enum and list shapes are rendered roughly — an enum shows a `# one of:`
/// hint and expands its first variant as a starting point; a list shows a
/// single `-` item. These are scaffolds to edit, not exhaustive documentation.
pub fn yaml_template(schema: &TypeSchema) -> String {
    let mut out = String::new();
    if let Some(doc) = schema.doc() {
        for line in doc.lines() {
            out.push_str("# ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    match schema {
        TypeSchema::Struct(s) => render_fields(s, 0, &mut out),
        TypeSchema::Enum(_) => render_kind(&Kind::Nested(schema.clone()), 0, &mut out),
    }
    out
}

fn render_fields(schema: &StructSchema, indent: usize, out: &mut String) {
    for field in &schema.fields {
        render_field(field, indent, out);
    }
}

fn render_field(field: &FieldSchema, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);

    if let Some(doc) = field.doc {
        for line in doc.lines() {
            out.push_str(&pad);
            out.push_str("# ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    if field.optional {
        out.push_str(&pad);
        out.push_str("# optional\n");
    }

    match &field.kind {
        Kind::Scalar(hint) => {
            // Optional scalars are commented out so the file stays valid as-is.
            let lead = if field.optional { "#" } else { "" };
            let value = field.default.unwrap_or_else(|| hint.placeholder());
            out.push_str(&pad);
            out.push_str(lead);
            out.push_str(field.key);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        }
        Kind::Nested(_) | Kind::List(_) => {
            // Nested headers stay uncommented (the `# optional` note carries the
            // intent); commenting a whole indented block reads worse than a hint.
            out.push_str(&pad);
            out.push_str(field.key);
            out.push_str(":\n");
            render_kind(&field.kind, indent + 1, out);
        }
    }
}

/// Render the body of a nested value (below its `key:` line, or at top level).
fn render_kind(kind: &Kind, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match kind {
        Kind::Scalar(hint) => {
            out.push_str(&pad);
            out.push_str(hint.placeholder());
            out.push('\n');
        }
        Kind::Nested(TypeSchema::Struct(sub)) => render_fields(sub, indent, out),
        Kind::Nested(TypeSchema::Enum(e)) => {
            let names: Vec<&str> = e.variants.iter().map(|v| v.name).collect();
            out.push_str(&pad);
            out.push_str("# one of: ");
            out.push_str(&names.join(" | "));
            out.push('\n');
            if let Some(first) = e.variants.first() {
                if let EnumTag::Internal(tag) = e.tag {
                    out.push_str(&pad);
                    out.push_str(tag);
                    out.push_str(": ");
                    out.push_str(first.name);
                    out.push('\n');
                }
                match &first.kind {
                    VariantKind::Unit => {
                        if let EnumTag::External = e.tag {
                            out.push_str(&pad);
                            out.push_str(first.name);
                            out.push('\n');
                        }
                    }
                    VariantKind::Newtype(inner) => match e.tag {
                        EnumTag::External => {
                            out.push_str(&pad);
                            out.push_str(first.name);
                            out.push_str(":\n");
                            render_kind(inner, indent + 1, out);
                        }
                        EnumTag::Internal(_) => render_kind(inner, indent, out),
                    },
                    VariantKind::Struct(fields) => {
                        let sub = StructSchema {
                            name: first.name,
                            doc: None,
                            fields: fields.clone(),
                        };
                        match e.tag {
                            EnumTag::External => {
                                out.push_str(&pad);
                                out.push_str(first.name);
                                out.push_str(":\n");
                                render_fields(&sub, indent + 1, out);
                            }
                            EnumTag::Internal(_) => render_fields(&sub, indent, out),
                        }
                    }
                }
            }
        }
        Kind::List(inner) => {
            let ipad = "  ".repeat(indent);
            out.push_str(&ipad);
            out.push_str("- ");
            // Inline scalars; block-render nested/list items on following lines.
            match inner.as_ref() {
                Kind::Scalar(hint) => {
                    out.push_str(hint.placeholder());
                    out.push('\n');
                }
                _ => {
                    out.push('\n');
                    render_kind(inner, indent + 1, out);
                }
            }
        }
    }
}
