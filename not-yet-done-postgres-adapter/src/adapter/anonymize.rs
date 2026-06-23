//! Postgres realism anonymizer.
//!
//! The content layer already wraps every adapter in a safe
//! [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer) when
//! `NYD_ANON` is set, so catalogue names are *never* leaked even without this.
//! This override only buys **realism** for a screenshot: a database/schema/table
//! name is replaced by an `<adjective>_<noun>` placeholder (`big_database`,
//! `nifty_schema`, `mellow_table`) so the viewer can still tell *what kind of
//! object* each tree node is — instead of every name collapsing into neutral
//! pool words.
//!
//! The structural group nodes ("Schemas", "Tables", "DB Scripts") and the
//! `db_script_dir` folders are kept **verbatim**: they carry no customer data
//! and are the tree's signposts. Row cell values fall back to the safe standard
//! scrub. Mapping is deterministic (see [`not_yet_done_content::anonymize`]).

use not_yet_done_content::anonymize::pseudo_labeled;
use not_yet_done_content::{Anonymizer, NodeType, StandardAnonymizer};

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresAnonymizer {
    std: StandardAnonymizer,
}

impl Anonymizer for PostgresAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match key {
            // Catalogue identifiers carried as metadata fields — keep them
            // recognisable as a database/schema/table.
            "database" => pseudo_labeled(value, "database"),
            "schema" => pseudo_labeled(value, "schema"),
            "table" => pseudo_labeled(value, "table"),
            // Row cells (arbitrary column names) and anything else → safe scrub.
            _ => self.std.scrub_value(key, value),
        }
    }

    fn scrub_label(&self, node_type: &NodeType, label: &str) -> String {
        match node_type.type_id.as_str() {
            // Real catalogue names → "<adjective>_<noun>" so the kind stays legible.
            "postgres:database" => pseudo_labeled(label, "database"),
            "postgres:schema" => pseudo_labeled(label, "schema"),
            "postgres:table" => pseudo_labeled(label, "table"),
            "postgres:db_script" => pseudo_labeled(label, "script"),
            // Structural signposts — no customer data, keep them verbatim so the
            // tree still reads "Schemas / Tables / DB Scripts".
            "postgres:root"
            | "postgres:schemas"
            | "postgres:tables"
            | "postgres:db_scripts"
            | "postgres:db_script_dir" => label.to_string(),
            // Row labels / anything new → safe free-text scrub.
            _ => self.std.scrub_value("label", label),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nt(type_id: &str) -> NodeType {
        NodeType {
            type_id: type_id.into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: String::new(),
            display_name: type_id.into(),
        }
    }

    #[test]
    fn catalogue_labels_become_adjective_noun_and_are_deterministic() {
        let a = PostgresAnonymizer::default();
        let db = a.scrub_label(&nt("postgres:database"), "customer_prod");
        assert!(db.ends_with("_database"), "kind must stay legible: {db}");
        assert!(!db.contains("customer"), "real name leaked: {db}");
        assert_eq!(db, a.scrub_label(&nt("postgres:database"), "customer_prod"));

        assert!(a.scrub_label(&nt("postgres:schema"), "billing").ends_with("_schema"));
        assert!(a.scrub_label(&nt("postgres:table"), "invoices").ends_with("_table"));
        assert!(a.scrub_label(&nt("postgres:db_script"), "monthly.sql").ends_with("_script"));
    }

    #[test]
    fn structural_signposts_stay_verbatim() {
        let a = PostgresAnonymizer::default();
        for (ty, label) in [
            ("postgres:schemas", "Schemas"),
            ("postgres:tables", "Tables"),
            ("postgres:db_scripts", "DB Scripts"),
            ("postgres:db_script_dir", "reports"),
        ] {
            assert_eq!(a.scrub_label(&nt(ty), label), label);
        }
    }

    #[test]
    fn metadata_catalogue_fields_match_their_labels() {
        let a = PostgresAnonymizer::default();
        assert!(a.scrub_value("schema", "billing").ends_with("_schema"));
        // A field-keyed scrub and a label-keyed scrub of the same name agree, so
        // a row's `schema` cell matches its parent schema node's label.
        assert_eq!(
            a.scrub_value("schema", "billing"),
            a.scrub_label(&nt("postgres:schema"), "billing")
        );
    }
}
