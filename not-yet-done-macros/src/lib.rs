//! Proc-macro crate for `not-yet-done-core`.
//!
//! Currently exports one derive macro:
//!
//! ## `#[derive(ColumnRegistry)]`
//!
//! Generates a `<Model>ColumnRegistry` struct and implements
//! `not_yet_done_core::filter::ColumnRegistry` for it, mapping each field
//! name of the annotated SeaORM model struct to the corresponding
//! `<Entity>::Column` variant.
//!
//! ### Requirements
//!
//! The derive target must be a SeaORM model struct (`Model`) whose sibling
//! `Column` enum follows SeaORM conventions (snake_case field name →
//! PascalCase variant). The entity module must be in scope where the macro
//! is used.
//!
//! ### Example
//!
//! ```rust,ignore
//! // entity/task.rs
//! use not_yet_done_macros::ColumnRegistry;
//!
//! #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, ColumnRegistry)]
//! #[sea_orm(table_name = "task")]
//! pub struct Model {
//!     pub id: Uuid,
//!     pub description: String,
//!     pub priority: i32,
//!     // ...
//! }
//! ```
//!
//! This generates:
//!
//! ```rust,ignore
//! pub struct TaskColumnRegistry;
//!
//! impl not_yet_done_core::filter::ColumnRegistry for TaskColumnRegistry {
//!     fn resolve(&self, table: Option<&str>, column: &str)
//!         -> Option<sea_orm::sea_query::ColumnRef>
//!     {
//!         // table qualifier is ignored for single-entity registries
//!         use sea_orm::sea_query::IntoColumnRef;
//!         match column {
//!             "id"          => Some(Column::Id.into_column_ref()),
//!             "description" => Some(Column::Description.into_column_ref()),
//!             "priority"    => Some(Column::Priority.into_column_ref()),
//!             // ...
//!             _ => None,
//!         }
//!     }
//! }
//! ```
//!
//! The registry name is derived from the *module* name: the macro looks for
//! a `#[sea_orm(table_name = "...")]` attribute and converts the table name
//! to PascalCase, appending `ColumnRegistry`.  If the attribute is absent,
//! the struct name itself (minus the `Model` suffix) is used.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    parse_macro_input, Data, DeriveInput, Fields, Ident, Lit
};

#[proc_macro_derive(ColumnRegistry, attributes(sea_orm))]
pub fn derive_column_registry(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_column_registry(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn impl_column_registry(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    // -----------------------------------------------------------------------
    // 1. Extract field names from the struct
    // -----------------------------------------------------------------------
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => &f.named,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "ColumnRegistry can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                Span::call_site(),
                "ColumnRegistry can only be derived for structs",
            ))
        }
    };

    let field_names: Vec<String> = fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
        .collect();

    // -----------------------------------------------------------------------
    // 2. Determine the registry name from #[sea_orm(table_name = "...")] or
    //    from the struct name (strip trailing "Model").
    // -----------------------------------------------------------------------
    let registry_name = registry_ident(&input)?;

    // -----------------------------------------------------------------------
    // 3. Build match arms: "field_name" => Some(Column::FieldName.into_column_ref())
    // -----------------------------------------------------------------------
    let arms = field_names.iter().map(|name| {
        let variant = snake_to_pascal(name);
        let variant_ident = Ident::new(&variant, Span::call_site());
        quote! {
            #name => Some(Column::#variant_ident.into_column_ref()),
        }
    });

    // -----------------------------------------------------------------------
    // 4. Emit the registry struct + ColumnRegistry impl
    // -----------------------------------------------------------------------
    Ok(quote! {
        /// Auto-generated column registry for use with
        /// `not_yet_done_core::filter::FilterBuilder`.
        pub struct #registry_name;

        impl crate::filter::ColumnRegistry for #registry_name {
            fn resolve(
                &self,
                _table: Option<&str>,
                column: &str,
            ) -> Option<sea_orm::sea_query::ColumnRef> {
                use sea_orm::sea_query::IntoColumnRef;
                match column {
                    #(#arms)*
                    _ => None,
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Determine the registry identifier.
///
/// Priority:
/// 1. `#[sea_orm(table_name = "some_table")]` → `SomeTableColumnRegistry`
/// 2. Struct name ending in `Model`           → strip suffix, append `ColumnRegistry`
/// 3. Struct name otherwise                   → append `ColumnRegistry`
fn registry_ident(input: &DeriveInput) -> syn::Result<Ident> {
    // Try to find table_name from #[sea_orm(table_name = "...")]
    for attr in &input.attrs {
        if !attr.path().is_ident("sea_orm") {
            continue;
        }
        // Parse the attribute as a list of meta items
        let result = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table_name") {
                let value = meta.value()?; // the `=`
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = lit {
                    return Err(syn::Error::new(s.span(), s.value()));
                }
            }
            Ok(())
        });
        // We smuggle the table name out via an Err — extract it here.
        if let Err(e) = result {
            let table_name = e.to_string();
            // Validate it looks like a real table name, not a real error.
            if !table_name.contains(' ') {
                let pascal = snake_to_pascal(&table_name);
                return Ok(Ident::new(&format!("{pascal}ColumnRegistry"), Span::call_site()));
            }
        }
    }

    // Fallback: derive from struct name
    let struct_name = input.ident.to_string();
    let base = struct_name
        .strip_suffix("Model")
        .unwrap_or(&struct_name);
    Ok(Ident::new(&format!("{base}ColumnRegistry"), Span::call_site()))
}

/// Convert `snake_case` to `PascalCase`.
fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests (logic only — proc macros can't be integration-tested in the same crate)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_to_pascal() {
        assert_eq!(snake_to_pascal("description"), "Description");
        assert_eq!(snake_to_pascal("created_at"), "CreatedAt");
        assert_eq!(snake_to_pascal("parent_id"), "ParentId");
        assert_eq!(snake_to_pascal("id"), "Id");
    }
}
