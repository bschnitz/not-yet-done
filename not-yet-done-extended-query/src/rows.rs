//! Viewing a [`NodeSummary`] as a filterable, sortable row.
//!
//! A `local_filter` runs against rows the adapter already returned, so this
//! module is the bridge between "a summary with a bag of metadata fields" and
//! the typed columns the shared evaluator expects.
//!
//! Cell lookup *is* [`not_yet_done_content::cell`] — the same function
//! [`not_yet_done_content::apply_sort`] reads through. Filtering and sorting
//! must see the same value: a row that a filter keeps because of what column
//! `x` contains, then sorts as if `x` held something else, would be
//! indefensible. Sharing the lookup is how that is guaranteed; it used to be a
//! copied line and a comment, and the copy grew a label fallback that made
//! every absent cell look occupied.
//!
//! # What is typed, and what is not
//!
//! A summary's cells are strings; the type comes from the column, not the
//! value. [`ColumnTypes`] reads it off the [`ColumnSchema`] list that
//! [`not_yet_done_content::columns_for`] has already unioned — the adapter's
//! own declaration plus whatever a decorator describes. That union is what
//! makes custom columns filterable and sortable here even though the adapter
//! cannot sort them: the extended framework filters and sorts *after* the
//! merge, when the injected cells are long since present.
//!
//! There is no boolean column kind, because neither source has one. A
//! `true`/`false` cell arrives as text, and the evaluator's text path compares
//! it case-insensitively, so `[all_day, "=", true]` still works.

use std::collections::HashMap;

use not_yet_done_content::{ColumnSchema, NodeSummary, SortKind};
use not_yet_done_filter::eval::{Field, RowFields};

/// The value type of each column a query may reference.
#[derive(Debug, Clone, Default)]
pub struct ColumnTypes {
    kinds: HashMap<String, SortKind>,
    /// Insertion order, so error messages list columns the way the user sees
    /// them rather than in hash order.
    order: Vec<String>,
}

impl ColumnTypes {
    /// Read the types off a column list — in practice the one
    /// [`not_yet_done_content::columns_for`] returns, where a decorator's
    /// description has already won over the adapter's declaration for a
    /// shared key.
    pub fn new(columns: &[ColumnSchema]) -> Self {
        let mut types = Self::default();
        for column in columns {
            types.insert(&column.key, column.sort_kind());
        }
        types
    }

    fn insert(&mut self, key: &str, kind: SortKind) {
        if self.kinds.insert(key.to_string(), kind).is_none() {
            self.order.push(key.to_string());
        }
    }

    pub fn kind(&self, column: &str) -> Option<SortKind> {
        self.kinds.get(column).copied()
    }

    /// Every known column key, in the order the sources declared them — the
    /// list a rejected column name is reported against.
    pub fn keys(&self) -> Vec<&str> {
        self.order.iter().map(String::as_str).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// One row: a summary plus the column types it should be read through.
pub struct SummaryRow<'a> {
    pub summary: &'a NodeSummary,
    pub types: &'a ColumnTypes,
}

impl<'a> SummaryRow<'a> {
    pub fn new(summary: &'a NodeSummary, types: &'a ColumnTypes) -> Self {
        Self { summary, types }
    }

    /// The raw cell text for a column, or `None` when the column carries no
    /// value for this row.
    ///
    /// Adapters render an absent value as the empty string, so that is what
    /// "no value" looks like by the time it reaches a row; the shared lookup
    /// treats both the same. That is what keeps `[ended, "<", "today"]` from
    /// sweeping up every still-running entry, and makes `is_null` mean
    /// something.
    fn cell(&self, column: &str) -> Option<&'a str> {
        not_yet_done_content::cell(self.summary, column)
    }
}

impl RowFields for SummaryRow<'_> {
    fn field(&self, column: &str) -> Field<'_> {
        let Some(raw) = self.cell(column) else {
            return Field::Null;
        };
        // An unknown column is text; `validate_columns` rejects those before
        // evaluation, so this only covers a column nobody typed.
        match self.types.kind(column).unwrap_or(SortKind::Text) {
            SortKind::Text => Field::Text(raw.into()),
            // A cell that does not parse under its declared type is absent
            // rather than wrong: sentinels like a literal `running` in a
            // datetime column must not compare as if they were instants.
            SortKind::Number => raw.trim().parse::<f64>().map_or(Field::Null, Field::Number),
            SortKind::DateTime => chrono::DateTime::parse_from_rfc3339(raw.trim())
                .map_or(Field::Null, |dt| Field::DateTime(dt.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{Metadata, MetadataField, NodeType};
    use not_yet_done_filter::{FilterExpr, eval, query_filter};

    fn summary(fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: "1".into(),
            label: "the label".into(),
            node_type: NodeType {
                type_id: "row".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "Row".into(),
            },
            metadata: Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: (*k).into(),
                        value: (*v).into(),
                        display_label: (*k).into(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    fn types() -> ColumnTypes {
        ColumnTypes::new(&[
            ColumnSchema::new("summary", "Summary"),
            ColumnSchema::new("updated", "Updated").typed("datetime"),
            ColumnSchema::new("description", "Description"),
            ColumnSchema::new("effort", "Effort").typed("number"),
        ])
    }

    fn hits(yaml: &str, fields: &[(&str, &str)]) -> bool {
        let resolved = query_filter::resolve_dates(serde_yaml::from_str(yaml).unwrap());
        let expr: FilterExpr = serde_yaml::from_value(resolved).unwrap();
        let types = types();
        let summary = summary(fields);
        eval::matches(&expr, &SummaryRow::new(&summary, &types))
    }

    #[test]
    fn a_described_custom_column_compares_as_the_number_it_is() {
        // The point of the whole exercise: the adapter cannot sort or filter
        // `effort`, because the decorator injects it after `list()` returned.
        assert!(hits("[effort, '>', 3]", &[("effort", "5")]));
        assert!(!hits("[effort, '>', 10]", &[("effort", "5")]));
        // …and numerically, not lexically — 5 < 10 despite "5" > "10".
        assert!(hits("[effort, '<', 10]", &[("effort", "5")]));
    }

    #[test]
    fn a_datetime_column_resolves_natural_language_on_the_right() {
        let fields = &[("updated", "2030-01-15T09:00:00+00:00")];
        assert!(hits("[updated, '>', 2030-01-01]", fields));
        assert!(!hits("[updated, '>', 2030-06-01]", fields));
    }

    #[test]
    fn an_empty_cell_is_absent_not_the_empty_string() {
        assert!(hits("[effort, is_null]", &[("effort", "")]));
        assert!(hits("[effort, is_null]", &[("effort", "   ")]));
        assert!(!hits("[effort, '<', 1]", &[("effort", "")]));
        assert!(hits("[effort, is_not_null]", &[("effort", "2")]));
    }

    #[test]
    fn a_cell_that_contradicts_its_column_type_reads_as_absent() {
        // A duration column rendered as `1h 30m`, or a sentinel in a date
        // column: absent beats a wrong comparison.
        assert!(hits("[effort, is_null]", &[("effort", "1h 30m")]));
        assert!(hits("[updated, is_null]", &[("updated", "running")]));
    }

    #[test]
    fn a_column_the_row_has_no_field_for_is_absent_not_the_label() {
        // The label ("the label") is never a cell. A column with no field on
        // this row is null — which is what makes a custom column, injected
        // only where a value is stored, filterable at all.
        assert!(hits("[description, is_null]", &[("summary", "x")]));
        assert!(!hits("[description, is_not_null]", &[("summary", "x")]));
        assert!(!hits("[description, has, label]", &[("summary", "x")]));
    }

    #[test]
    fn the_column_list_decides_the_type() {
        let types = ColumnTypes::new(&[ColumnSchema::new("size", "Size").typed("number")]);
        assert_eq!(types.kind("size"), Some(SortKind::Number));
        assert_eq!(types.keys(), vec!["size"], "no duplicate entry");
    }

    #[test]
    fn keys_keep_declaration_order_for_error_messages() {
        assert_eq!(
            types().keys(),
            vec!["summary", "updated", "description", "effort"]
        );
    }
}
