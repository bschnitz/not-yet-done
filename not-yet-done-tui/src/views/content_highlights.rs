//! Painting a level's `highlights:` rules onto a finished table.
//!
//! This runs as a **post-pass**: the rows arrive laid out and fitted, and a
//! highlight only swaps the style slot a cell points at. Nothing here can
//! change a width, a truncation or a row order, which is what makes it safe
//! to hang off both flat build paths (grouped and plain) with one call.
//!
//! # Two styles per cell, not one
//!
//! Every highlighted cell gets a *pair* of slots — one for the ordinary row,
//! one for the row under the cursor (see
//! [`TableWidgetCell::with_style_pair`]). The cursor moves far more often
//! than the table is rebuilt, so the answer to "what does this cell look like
//! while selected" has to be sitting in the row already; recomputing it per
//! keypress would mean rebuilding the whole table on every `j`.
//!
//! # Rows and cells in one pass
//!
//! A rule without `columns:` paints the whole row and lands on the row's own
//! style pair; one that names columns lands on those cells. Both are folded
//! in the same loop, row layer first, because the row's background is the
//! ground a cell's `auto` foreground has to contrast against.
//!
//! # What is not here
//!
//! Card, details and tree surfaces paint elsewhere; [`HighlightMode`] is what
//! keeps a rule out of a surface it was not meant for.

use ratatui::style::{Color, Style};

use not_yet_done_content::{ColumnSchema, NodeSummary};
use not_yet_done_extended_query::rows::{ColumnTypes, SummaryRow};
use not_yet_done_ratatui::widgets::table::{TableWidgetCell, TableWidgetRow};

use crate::config::highlight::{HighlightMode, HighlightRule, StyleLayer};
use crate::config::view_config::{ColumnDef, ColumnKind};
use crate::ui::theme::Theme;

/// Marks a widget row that belongs to no item — a group header or a total.
/// The build paths already use `usize::MAX` for exactly this in their
/// row→item maps, so it travels in without a second convention.
pub const NO_ITEM: usize = usize::MAX;

/// One level's highlight rules, ready to paint a table.
pub struct TableHighlights<'a> {
    /// The columns in render order — a rule names them by `key`.
    pub columns: &'a [ColumnDef],
    /// The level's rules, already resolved by `prepare_view_file`.
    pub rules: &'a [HighlightRule],
    pub theme: &'a Theme,
    /// The background in force under an ordinary row, for an `auto`
    /// foreground to contrast against.
    pub row_bg: Option<Color>,
    /// The same under the cursor row.
    pub selected_bg: Option<Color>,
    /// The first free slot in the caller's style map.
    pub slot_base: usize,
}

/// A rule reduced to what the row loop needs: what it may paint.
struct Prepared<'a> {
    rule: &'a HighlightRule,
    normal: StyleLayer,
    selected: StyleLayer,
    scope: Scope,
}

/// How far a rule reaches once its column keys are resolved against the
/// level: over the whole row, or over these column indices.
enum Scope {
    Row,
    Cells(Vec<usize>),
}

impl Prepared<'_> {
    fn is_row(&self) -> bool {
        matches!(self.scope, Scope::Row)
    }

    fn paints(&self, column: usize) -> bool {
        matches!(&self.scope, Scope::Cells(cols) if cols.contains(&column))
    }
}

impl TableHighlights<'_> {
    /// Paint the rules onto `rows` and return the styles the new slots hold.
    ///
    /// `row_items` maps each row to its index in `items`, [`NO_ITEM`] for a
    /// row that stands for none. The returned styles are appended to the
    /// caller's style map, in slot order starting at `slot_base`.
    pub fn apply(
        &self,
        rows: &mut [TableWidgetRow],
        row_items: &[usize],
        items: &[NodeSummary],
    ) -> Vec<Style> {
        let prepared = self.prepare();
        if prepared.is_empty() {
            return Vec::new();
        }
        let types = column_types(self.columns);
        let mut slots = SlotTable::new(self.slot_base);

        for (row_idx, row) in rows.iter_mut().enumerate() {
            let Some(item) = row_items
                .get(row_idx)
                .filter(|&&i| i != NO_ITEM)
                .and_then(|&i| items.get(i))
            else {
                continue;
            };
            let view = SummaryRow::new(item, &types);
            // Evaluate each rule once per row, not once per cell: `when:`
            // asks about the row, and a regex is the expensive part here.
            let hits: Vec<&Prepared<'_>> =
                prepared.iter().filter(|p| p.rule.matches(&view)).collect();
            if hits.is_empty() {
                continue;
            }
            // The row layer goes on first, and its background becomes the
            // ground the cells sit on — otherwise a cell asking for an `auto`
            // foreground would contrast against a background the row rule has
            // already painted over.
            let mut ground = (self.row_bg, self.selected_bg);
            if let Some((normal, selected)) = stack(hits.iter().copied().filter(|p| p.is_row())) {
                let normal = normal.to_ratatui(self.theme, self.row_bg);
                let selected = selected.to_ratatui(self.theme, self.selected_bg);
                ground = (normal.bg.or(ground.0), selected.bg.or(ground.1));
                row.style_id = Some(slots.intern(normal));
                row.selected_style_id = Some(slots.intern(selected));
            }
            // Long-text mode stacks continuation lines under a row; they
            // hold one wrapped block rather than this level's columns, so
            // only the row's own line is painted.
            let Some(line) = row.lines.first_mut() else {
                continue;
            };
            for (col_idx, cell) in line.cells.iter_mut().enumerate() {
                let Some((normal, selected)) =
                    stack(hits.iter().copied().filter(|p| p.paints(col_idx)))
                else {
                    continue;
                };
                paint(
                    cell,
                    &mut slots,
                    normal.to_ratatui(self.theme, ground.0),
                    selected.to_ratatui(self.theme, ground.1),
                );
            }
        }
        slots.into_styles()
    }

    /// The rules that can paint here at all: resolved, meant for the table,
    /// and — when they name columns — naming at least one this level has.
    fn prepare(&self) -> Vec<Prepared<'_>> {
        self.rules
            .iter()
            .filter_map(|rule| {
                let style = rule.style_for(HighlightMode::Table)?;
                let scope = if rule.columns.is_empty() {
                    Scope::Row
                } else {
                    let columns: Vec<usize> = rule
                        .columns
                        .iter()
                        .filter_map(|key| self.columns.iter().position(|c| &c.key == key))
                        .collect();
                    if columns.is_empty() {
                        return None;
                    }
                    Scope::Cells(columns)
                };
                Some(Prepared {
                    rule,
                    normal: style.normal,
                    selected: style.selected_layer(),
                    scope,
                })
            })
            .collect()
    }
}

/// Lay the rules' layers over one another in declaration order — the last
/// rule to name a field wins it, modifiers accumulate. `None` when nothing
/// in `hits` reaches this far.
fn stack<'p>(hits: impl Iterator<Item = &'p Prepared<'p>>) -> Option<(StyleLayer, StyleLayer)> {
    hits.fold(None, |acc: Option<(StyleLayer, StyleLayer)>, p| {
        Some(match acc {
            Some((n, s)) => (n.layer(p.normal), s.layer(p.selected)),
            None => (p.normal, p.selected),
        })
    })
}

/// Point a cell at the pair of slots holding these two styles.
fn paint(cell: &mut TableWidgetCell, slots: &mut SlotTable, normal: Style, selected: Style) {
    // A cell that already carried an override (the deleted dim, say) loses
    // it here: the rule is the user saying what this cell should look like,
    // and two half-applied colours read worse than one deliberate one.
    cell.style_id = Some(slots.intern(normal));
    cell.selected_style_id = Some(slots.intern(selected));
}

/// The style slots this pass adds, deduplicated.
///
/// A rule that fires on a hundred rows is one style, and the widget looks a
/// slot up per cell per frame — so the table stays as short as the set of
/// distinct looks, not as long as the set of painted cells.
struct SlotTable {
    base: usize,
    styles: Vec<Style>,
}

impl SlotTable {
    fn new(base: usize) -> Self {
        Self {
            base,
            styles: Vec::new(),
        }
    }

    fn intern(&mut self, style: Style) -> usize {
        let idx = match self.styles.iter().position(|s| *s == style) {
            Some(i) => i,
            None => {
                self.styles.push(style);
                self.styles.len() - 1
            }
        };
        self.base + idx
    }

    fn into_styles(self) -> Vec<Style> {
        self.styles
    }
}

/// The column types a `when:` condition is evaluated through.
///
/// The kinds come from the view config rather than the adapter schema on
/// purpose: `when:` names a column the way the level declares it, and that
/// declaration is also what the cell was rendered from.
fn column_types(columns: &[ColumnDef]) -> ColumnTypes {
    let schemas: Vec<ColumnSchema> = columns
        .iter()
        .map(|col| {
            let value_type = match col.kind {
                ColumnKind::Number => "number",
                ColumnKind::Duration | ColumnKind::Elapsed => "duration",
                ColumnKind::Datetime => "datetime",
                ColumnKind::Text | ColumnKind::Path => "text",
            };
            ColumnSchema::new(
                &col.key,
                col.label.clone().unwrap_or_else(|| col.key.clone()),
            )
            .typed(value_type)
        })
        .collect();
    ColumnTypes::new(&schemas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;
    use crate::config::highlight::prepare_view_file;
    use crate::config::view_config::ViewFileConfig;
    use not_yet_done_content::{Metadata, MetadataField, NodeType};
    use not_yet_done_ratatui::widgets::table::TableWidgetCell;

    fn theme() -> Theme {
        Theme::new(ThemeConfig::default())
    }

    /// A one-level view file whose `tickets` level carries `rules` — the
    /// same route the real config takes, so the rules arrive resolved.
    fn level(rules: &str) -> ViewFileConfig {
        let (config, warnings) = level_lax(rules);
        assert!(warnings.is_empty(), "{warnings:?}");
        config
    }

    /// The same, for a file the load has something to say about.
    fn level_lax(rules: &str) -> (ViewFileConfig, Vec<String>) {
        let yaml = format!(
            r#"
tab: {{ name: Demo }}
adapter: {{ type: demo, config_inline: '' }}
styles:
  hot: {{ bg: '#7a1c1c' }}
views:
  - name: tickets
    node_type: 'demo:item'
    columns:
      - {{ key: status }}
      - {{ key: actual, kind: number }}
    highlights:
{rules}
"#
        );
        let mut config: ViewFileConfig =
            serde_yaml::from_str(&yaml).expect("view file should parse");
        let warnings = prepare_view_file(&mut config, &theme());
        (config, warnings)
    }

    fn item(status: &str, actual: &str) -> NodeSummary {
        let field = |key: &str, value: &str| MetadataField {
            key: key.into(),
            value: value.into(),
            display_label: key.into(),
            editable: false,
            allowed_values: None,
        };
        NodeSummary {
            id: status.into(),
            label: status.into(),
            node_type: NodeType {
                type_id: "demo:item".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "item".into(),
            },
            metadata: Metadata {
                fields: vec![field("status", status), field("actual", actual)],
            },
            has_children: None,
        }
    }

    fn row(cells: &[&str]) -> TableWidgetRow {
        TableWidgetRow::new(cells.iter().map(|c| TableWidgetCell::plain(*c)).collect())
    }

    /// The pass as the render path calls it, over the `tickets` level.
    fn run(
        config: &ViewFileConfig,
        rows: &mut [TableWidgetRow],
        row_items: &[usize],
        items: &[NodeSummary],
        theme: &Theme,
    ) -> Vec<Style> {
        let view = &config.views[0];
        TableHighlights {
            columns: &view.columns,
            rules: &view.highlights,
            theme,
            row_bg: Some(Color::Black),
            selected_bg: Some(Color::DarkGray),
            slot_base: 7,
        }
        .apply(rows, row_items, items)
    }

    #[test]
    fn a_cell_rule_paints_only_the_matching_row() {
        let config = level(
            "      - columns: [status]\n        when: { field: status, matches: '^Blocked$' }\n        style: hot",
        );
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        let styles = run(&config, &mut rows, &[0, 1], &items, &theme());

        let cell = |r: usize, c: usize| &rows[r].lines[0].cells[c];
        assert_eq!(cell(0, 0).style_id, Some(7));
        // The untouched column and the non-matching row keep their slot.
        assert_eq!(cell(0, 1).style_id, None);
        assert_eq!(cell(1, 0).style_id, None);
        assert_eq!(styles[0].bg, Some(Color::Rgb(0x7a, 0x1c, 0x1c)));
    }

    #[test]
    fn a_column_rule_without_a_condition_paints_every_row() {
        let config = level("      - columns: [actual]\n        style: hot");
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        run(&config, &mut rows, &[0, 1], &items, &theme());

        assert_eq!(rows[0].lines[0].cells[1].style_id, Some(7));
        assert_eq!(rows[1].lines[0].cells[1].style_id, Some(7));
        assert_eq!(rows[0].lines[0].cells[0].style_id, None);
    }

    #[test]
    fn the_cursor_row_keeps_its_own_background() {
        let config = level("      - columns: [status]\n        style: hot");
        let items = vec![item("Blocked", "3")];
        let mut rows = vec![row(&["Blocked", "3"])];
        let styles = run(&config, &mut rows, &[0], &items, &theme());

        let cell = &rows[0].lines[0].cells[0];
        let normal = styles[cell.style_id.unwrap() - 7];
        let selected = styles[cell.selected_style_id.unwrap() - 7];
        assert_eq!(normal.bg, Some(Color::Rgb(0x7a, 0x1c, 0x1c)));
        // Without an explicit `selected.bg` the table's selection background
        // stays, or the cursor would vanish on exactly the loud rows.
        assert_eq!(selected.bg, None);
    }

    #[test]
    fn later_rules_layer_over_earlier_ones() {
        let config = level(
            "      - columns: [status]\n        style: { fg: '#112233', modifiers: [bold] }\n      - columns: [status]\n        style: { fg: '#445566' }",
        );
        let items = vec![item("Blocked", "3")];
        let mut rows = vec![row(&["Blocked", "3"])];
        let styles = run(&config, &mut rows, &[0], &items, &theme());

        let normal = styles[rows[0].lines[0].cells[0].style_id.unwrap() - 7];
        assert_eq!(normal.fg, Some(Color::Rgb(0x44, 0x55, 0x66)));
        // Modifiers accumulate — the second rule says nothing about bold.
        assert!(normal.add_modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn one_look_costs_one_slot_however_many_cells_wear_it() {
        let config = level("      - columns: [status]\n        style: hot");
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        let styles = run(&config, &mut rows, &[0, 1], &items, &theme());
        assert_eq!(styles.len(), 2, "one normal + one selected style");
    }

    #[test]
    fn a_row_without_an_item_is_left_alone() {
        let config = level("      - columns: [status]\n        style: hot");
        let items = vec![item("Blocked", "3")];
        // Row 0 is a group header, row 1 the item beneath it.
        let mut rows = vec![row(&["── group ", ""]), row(&["Blocked", "3"])];
        run(&config, &mut rows, &[NO_ITEM, 0], &items, &theme());

        assert_eq!(rows[0].lines[0].cells[0].style_id, None);
        assert_eq!(rows[1].lines[0].cells[0].style_id, Some(7));
    }

    #[test]
    fn a_rule_without_columns_paints_the_whole_row() {
        let config =
            level("      - when: { field: status, matches: '^Blocked$' }\n        style: hot");
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        let styles = run(&config, &mut rows, &[0, 1], &items, &theme());

        let normal = styles[rows[0].style_id.unwrap() - 7];
        assert_eq!(normal.bg, Some(Color::Rgb(0x7a, 0x1c, 0x1c)));
        // The row wears it, the cells stay unpainted — the widget lays the
        // row style under them.
        assert_eq!(rows[0].lines[0].cells[0].style_id, None);
        assert_eq!(rows[1].style_id, None);
    }

    #[test]
    fn a_row_rule_and_a_cell_rule_land_on_their_own_slots() {
        let config = level(
            "      - style: { bg: '#101010' }\n      - columns: [actual]\n        style: { fg: '#445566' }",
        );
        let items = vec![item("Open", "1")];
        let mut rows = vec![row(&["Open", "1"])];
        let styles = run(&config, &mut rows, &[0], &items, &theme());

        assert_eq!(
            styles[rows[0].style_id.unwrap() - 7].bg,
            Some(Color::Rgb(0x10, 0x10, 0x10))
        );
        let cell = &rows[0].lines[0].cells[1];
        assert_eq!(
            styles[cell.style_id.unwrap() - 7].fg,
            Some(Color::Rgb(0x44, 0x55, 0x66))
        );
        // The cell rule names no background, so it does not carry the row's.
        assert_eq!(styles[cell.style_id.unwrap() - 7].bg, None);
        assert_eq!(rows[0].lines[0].cells[0].style_id, None);
    }

    #[test]
    fn a_cells_auto_foreground_contrasts_against_the_row_it_sits_on() {
        let theme = theme();
        let config = level(
            "      - style: { bg: '#ffffff' }\n      - columns: [status]\n        style: { fg: auto }",
        );
        let items = vec![item("Open", "1")];
        let mut rows = vec![row(&["Open", "1"])];
        let styles = run(&config, &mut rows, &[0], &items, &theme);

        // White row → the dark candidate, not the one the black table
        // background would have picked.
        let cell = &rows[0].lines[0].cells[0];
        assert_eq!(
            styles[cell.style_id.unwrap() - 7].fg,
            Some(theme.auto_fg_dark())
        );
    }

    #[test]
    fn a_rule_naming_only_unknown_columns_does_not_fall_back_to_the_row() {
        // The load already warned about `nope`; what matters here is that an
        // empty resolved column list is not mistaken for row scope.
        let (config, warnings) = level_lax("      - columns: [nope]\n        style: hot");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let items = vec![item("Blocked", "3")];
        let mut rows = vec![row(&["Blocked", "3"])];
        let styles = run(&config, &mut rows, &[0], &items, &theme());

        assert!(styles.is_empty());
        assert_eq!(rows[0].style_id, None);
    }

    #[test]
    fn a_rule_for_another_surface_stays_out_of_the_table() {
        let config = level("      - columns: [status]\n        style: hot\n        modes: [card]");
        let items = vec![item("Blocked", "3")];
        let mut rows = vec![row(&["Blocked", "3"])];
        assert!(run(&config, &mut rows, &[0], &items, &theme()).is_empty());
    }

    #[test]
    fn a_numeric_condition_compares_as_a_number() {
        let config =
            level("      - columns: [actual]\n        when: [actual, '>', 5]\n        style: hot");
        // Lexically \"10\" < \"9\"; numerically it is not.
        let items = vec![item("Open", "10"), item("Open", "3")];
        let mut rows = vec![row(&["Open", "10"]), row(&["Open", "3"])];
        run(&config, &mut rows, &[0, 1], &items, &theme());

        assert_eq!(rows[0].lines[0].cells[1].style_id, Some(7));
        assert_eq!(rows[1].lines[0].cells[1].style_id, None);
    }
}
