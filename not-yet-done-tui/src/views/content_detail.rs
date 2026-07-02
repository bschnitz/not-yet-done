//! Record-detail split rendering helpers (`record_detail: true` views).
//!
//! A record-detail *follower* pane shows the **selected** row of its source
//! table transposed into a two-column table: field name | field value (the
//! columns of the source record, one per row). The follower reuses the
//! ordinary flat-table render path unchanged — it is only fed synthetic
//! [`NodeSummary`] items (one per source field) plus the two fixed columns
//! produced here, so there is zero duplication of the table builder.
//!
//! The table engine cannot wrap a value *inside* a single cell — intra-cell
//! multi-line rendering is reserved for the chat `row_layout`. With wrap
//! enabled the value is therefore split into width-sized segments rendered as
//! continuation rows whose field cell is blank; with wrap off the value stays
//! on one line and the engine clips it to the column.

use crate::config::view_config::{ColumnDef, ColumnKind};
use not_yet_done_content::{Metadata, MetadataField, NodeSummary, NodeType};

/// Metadata key (and column key) of the synthetic "field name" column.
pub const FIELD_KEY: &str = "field";
/// Metadata key (and column key) of the synthetic "field value" column.
pub const VALUE_KEY: &str = "value";
/// Lower bound for the field-name column width. Also the fallback width when
/// the record has no fields yet, so an empty follower still lays out sanely.
pub const FIELD_COL_MIN: usize = 8;
/// Upper bound for the field-name column width — a pathologically long field
/// label must not crowd the value column off the pane.
pub const FIELD_COL_MAX: usize = 40;
/// `type_id` stamped on synthetic detail rows. Never produced by an adapter;
/// it exists only so the rows carry a valid (inert) [`NodeType`].
const DETAIL_TYPE_ID: &str = "record_detail:field";
/// Render width assumed for a follower that hasn't been drawn yet (its
/// table reports width 0 on the very first frame). Picked so the first
/// wrap pass produces a reasonable layout before the post-draw re-fit
/// learns the true width.
const DEFAULT_RENDER_WIDTH: usize = 80;
/// Columns consumed by the inter-column separator/padding between the
/// field and value columns — subtracted when sizing the value column.
const COLUMN_GAP: usize = 3;
/// Floor for the value column so a very narrow pane still wraps sanely.
const MIN_VALUE_WIDTH: usize = 8;

/// The two fixed columns of a record-detail follower pane: the field-name
/// column (clamped fixed width) and the value column (flex — fills the rest
/// of the pane). `field_width` is the caller's measured longest field label;
/// it is clamped to `[FIELD_COL_MIN, FIELD_COL_MAX]` here.
pub fn detail_columns(field_width: usize) -> Vec<ColumnDef> {
    let w = field_width.clamp(FIELD_COL_MIN, FIELD_COL_MAX);
    vec![
        detail_column(FIELD_KEY, "Field", format!("fixed({w})"), Some("accent")),
        detail_column(VALUE_KEY, "Value", "flex(1)".to_string(), None),
    ]
}

fn detail_column(key: &str, label: &str, sizing: String, style: Option<&str>) -> ColumnDef {
    ColumnDef {
        key: key.to_string(),
        label: Some(label.to_string()),
        source: None,
        collapsed_source: None,
        long_source: None,
        style: style.map(str::to_string),
        sizing,
        markdown: false,
        kind: ColumnKind::Text,
        format: None,
        separator: None,
        elapsed_from: None,
        tree_aggregate: None,
        hidden: false,
    }
}

/// Transpose a source record into the synthetic items the flat render path
/// consumes: one row per source metadata field, in source order. Each item's
/// metadata carries exactly the two keys [`FIELD_KEY`] / [`VALUE_KEY`] that
/// [`detail_columns`] read.
///
/// `value_width` is the rendered width of the value column. With `wrap` true a
/// value longer than that (or carrying hard line breaks) is split into
/// continuation rows — the first row holds the field label, later rows leave
/// the field cell blank — so the whole value is visible. With `wrap` false the
/// value stays on a single line (embedded newlines collapsed to spaces) and
/// the engine clips it to the column.
pub fn detail_items(summary: &NodeSummary, wrap: bool, value_width: usize) -> Vec<NodeSummary> {
    let mut items = Vec::new();
    for (idx, field) in summary.metadata.fields.iter().enumerate() {
        let label = if field.display_label.is_empty() {
            field.key.clone()
        } else {
            field.display_label.clone()
        };
        for (seg_idx, seg) in value_segments(&field.value, wrap, value_width)
            .into_iter()
            .enumerate()
        {
            let field_cell = if seg_idx == 0 {
                label.clone()
            } else {
                String::new()
            };
            items.push(detail_row(format!("{idx}:{seg_idx}"), field_cell, seg));
        }
    }
    items
}

/// Split a field value into the physical lines the detail table renders.
///
/// Wrap off → one line, embedded newlines flattened to spaces. Wrap on →
/// preserve the value's own hard line breaks, then character-wrap each line to
/// `width` (word-unaware on purpose: field values are arbitrary DB/JSON data
/// where greedy char wrapping is the only robust choice). Always returns at
/// least one (possibly empty) segment so an empty value still yields a row.
fn value_segments(value: &str, wrap: bool, width: usize) -> Vec<String> {
    if !wrap {
        return vec![value.replace('\n', " ")];
    }
    let width = width.max(1);
    let mut out = Vec::new();
    for line in value.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            out.push(chars[start..end].iter().collect());
            start = end;
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Longest field *label* (display label, or key as fallback) in a record,
/// in characters — the measure that sizes the field-name column.
fn field_label_width(summary: &NodeSummary) -> usize {
    summary
        .metadata
        .fields
        .iter()
        .map(|f| {
            if f.display_label.is_empty() {
                f.key.chars().count()
            } else {
                f.display_label.chars().count()
            }
        })
        .max()
        .unwrap_or(FIELD_COL_MIN)
}

/// Width available to the value column when wrapping: the follower's render
/// width minus the (clamped) field column and the inter-column gap. Falls
/// back to [`DEFAULT_RENDER_WIDTH`] before the pane has been drawn, and never
/// drops below [`MIN_VALUE_WIDTH`]. Only consulted with wrap on; with wrap off
/// the engine clips and the width is irrelevant.
pub fn value_width(render_width: usize, summary: &NodeSummary) -> usize {
    let total = if render_width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        render_width
    };
    let field = field_label_width(summary).clamp(FIELD_COL_MIN, FIELD_COL_MAX);
    total.saturating_sub(field + COLUMN_GAP).max(MIN_VALUE_WIDTH)
}

fn detail_row(id: String, field: String, value: String) -> NodeSummary {
    NodeSummary {
        id,
        label: String::new(),
        node_type: detail_node_type(),
        metadata: Metadata {
            fields: vec![
                MetadataField {
                    key: FIELD_KEY.to_string(),
                    value: field,
                    display_label: "Field".to_string(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: VALUE_KEY.to_string(),
                    value,
                    display_label: "Value".to_string(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        },
        has_children: Some(false),
    }
}

fn detail_node_type() -> NodeType {
    NodeType {
        type_id: DETAIL_TYPE_ID.to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Field".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, label: &str, value: &str) -> MetadataField {
        MetadataField {
            key: key.to_string(),
            value: value.to_string(),
            display_label: label.to_string(),
            editable: false,
            allowed_values: None,
        }
    }

    fn summary(fields: Vec<MetadataField>) -> NodeSummary {
        NodeSummary {
            id: "row1".to_string(),
            label: "Row 1".to_string(),
            node_type: detail_node_type(),
            metadata: Metadata { fields },
            has_children: Some(false),
        }
    }

    fn cell(item: &NodeSummary, key: &str) -> String {
        item.metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    }

    #[test]
    fn columns_are_field_then_value_clamped() {
        let cols = detail_columns(100);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].key, FIELD_KEY);
        assert_eq!(cols[1].key, VALUE_KEY);
        // Clamped to FIELD_COL_MAX.
        assert_eq!(cols[0].sizing, format!("fixed({FIELD_COL_MAX})"));
        assert_eq!(cols[1].sizing, "flex(1)");
    }

    #[test]
    fn columns_field_width_has_floor() {
        // A zero measured width still produces a usable field column.
        assert_eq!(
            detail_columns(0)[0].sizing,
            format!("fixed({FIELD_COL_MIN})")
        );
    }

    #[test]
    fn items_transpose_one_row_per_field() {
        let s = summary(vec![
            field("name", "Name", "alice"),
            field("age", "Age", "30"),
        ]);
        let items = detail_items(&s, false, 40);
        assert_eq!(items.len(), 2);
        assert_eq!(cell(&items[0], FIELD_KEY), "Name");
        assert_eq!(cell(&items[0], VALUE_KEY), "alice");
        assert_eq!(cell(&items[1], FIELD_KEY), "Age");
        assert_eq!(cell(&items[1], VALUE_KEY), "30");
    }

    #[test]
    fn items_fall_back_to_key_when_label_empty() {
        let s = summary(vec![field("raw_key", "", "v")]);
        let items = detail_items(&s, false, 40);
        assert_eq!(cell(&items[0], FIELD_KEY), "raw_key");
    }

    #[test]
    fn no_wrap_flattens_newlines_to_single_row() {
        let s = summary(vec![field("body", "Body", "line1\nline2")]);
        let items = detail_items(&s, false, 40);
        assert_eq!(items.len(), 1);
        assert_eq!(cell(&items[0], VALUE_KEY), "line1 line2");
    }

    #[test]
    fn wrap_splits_long_value_into_continuation_rows() {
        let s = summary(vec![field("body", "Body", "abcdefghij")]);
        let items = detail_items(&s, true, 4);
        // 10 chars / width 4 → 3 segments.
        assert_eq!(items.len(), 3);
        assert_eq!(cell(&items[0], FIELD_KEY), "Body");
        assert_eq!(cell(&items[0], VALUE_KEY), "abcd");
        // Continuation rows blank the field cell.
        assert_eq!(cell(&items[1], FIELD_KEY), "");
        assert_eq!(cell(&items[1], VALUE_KEY), "efgh");
        assert_eq!(cell(&items[2], VALUE_KEY), "ij");
    }

    #[test]
    fn wrap_preserves_hard_line_breaks() {
        let s = summary(vec![field("body", "Body", "ab\ncd")]);
        let items = detail_items(&s, true, 40);
        assert_eq!(items.len(), 2);
        assert_eq!(cell(&items[0], VALUE_KEY), "ab");
        assert_eq!(cell(&items[1], VALUE_KEY), "cd");
        assert_eq!(cell(&items[1], FIELD_KEY), "");
    }

    #[test]
    fn empty_record_yields_no_items() {
        let s = summary(vec![]);
        assert!(detail_items(&s, false, 40).is_empty());
    }

    #[test]
    fn value_width_subtracts_field_column_and_gap() {
        // Field label "Name" (4) is below the floor (8) → field col = 8.
        // value = 40 - 8 - 3 = 29.
        let s = summary(vec![field("name", "Name", "x")]);
        assert_eq!(value_width(40, &s), 29);
    }

    #[test]
    fn value_width_uses_default_before_first_draw() {
        let s = summary(vec![field("name", "Name", "x")]);
        // render width 0 → DEFAULT_RENDER_WIDTH (80): 80 - 8 - 3 = 69.
        assert_eq!(value_width(0, &s), 69);
    }

    #[test]
    fn value_width_has_floor_on_narrow_pane() {
        let s = summary(vec![field("name", "Name", "x")]);
        // 10 - 8 - 3 would underflow → clamped to MIN_VALUE_WIDTH.
        assert_eq!(value_width(10, &s), MIN_VALUE_WIDTH);
    }
}
