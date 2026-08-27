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
//! # Two sources, one stack
//!
//! Besides the level's `highlights:` rules a `load`-hook script may answer
//! with highlights of its own ([`ScriptHighlights`]). They layer on top: the
//! script looked at the actual row, so it holds the more specific
//! information. Both sources are folded into the same slot table.
//!
//! # What is not here
//!
//! Card, details and tree surfaces paint elsewhere; [`HighlightMode`] is what
//! keeps a rule out of a surface it was not meant for.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Style};

use not_yet_done_content::{ColumnSchema, NodeSummary};
use not_yet_done_extended_query::rows::{ColumnTypes, SummaryRow};
use not_yet_done_ratatui::widgets::table::{TableWidgetCell, TableWidgetRow};

use crate::config::highlight::{
    HighlightMode, HighlightRule, ResolvedStyle, StyleLayer, StyleResolver, StyleSpec,
};
use crate::config::view_config::{ColumnDef, ColumnKind};
use crate::ui::theme::Theme;

/// Marks a widget row that belongs to no item — a group header or a total.
/// The build paths already use `usize::MAX` for exactly this in their
/// row→item maps, so it travels in without a second convention.
pub const NO_ITEM: usize = usize::MAX;

/// Reserved key in both axes of a script's map: every row / every column.
const ANY: &str = "*";

/// What a `load`-hook script asked to have painted, keyed the way its answer
/// is: `row id → column key → style`, with `"*"` reserved in both axes.
///
/// Kept beside the rows, not on them. [`NodeSummary`] is the adapter contract
/// and is built by struct literal in over a hundred places, so it cannot grow
/// a field for this; the map instead lives exactly as long as the rows the
/// load produced, and every load refills it.
#[derive(Debug, Default, Clone)]
pub struct ScriptHighlights {
    rows: HashMap<String, HashMap<String, ResolvedStyle>>,
}

impl ScriptHighlights {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Read the `highlights` key of a hook answer, resolving every style
    /// through `resolver`.
    ///
    /// `Err` is for an answer whose *shape* is wrong — that is the script
    /// saying something the format cannot carry. A single unusable style is
    /// not: it is returned as a warning and costs only its own entry, the
    /// same bargain the rest of the highlight machinery makes.
    pub fn parse(
        answer: &serde_json::Value,
        resolver: &StyleResolver<'_>,
    ) -> Result<(Self, Vec<String>), String> {
        let Some(value) = answer.get("highlights") else {
            return Ok((Self::default(), Vec::new()));
        };
        let map = value
            .as_object()
            .ok_or_else(|| "`highlights` must be an object keyed by row id".to_string())?;

        let mut out = Self::default();
        let mut warnings = Vec::new();
        for (row_id, columns) in map {
            let columns = columns.as_object().ok_or_else(|| {
                format!("highlights['{row_id}'] must be an object keyed by column")
            })?;
            for (column, spec) in columns {
                let spec: StyleSpec = match serde_json::from_value(spec.clone()) {
                    Ok(spec) => spec,
                    Err(e) => {
                        warnings.push(format!("highlights['{row_id}']['{column}']: {e}"));
                        continue;
                    }
                };
                match resolver.resolve(&spec) {
                    Ok(style) => {
                        out.rows
                            .entry(row_id.clone())
                            .or_default()
                            .insert(column.clone(), style);
                    }
                    Err(e) => warnings.push(format!("highlights['{row_id}']['{column}']: {e}")),
                }
            }
        }
        Ok((out, warnings))
    }

    /// Fold `other` in on top. Several scripts run in name order on one load,
    /// and the later one has the last word per (row, column) — the same order
    /// their `cells` patches take effect in.
    pub fn merge(&mut self, other: ScriptHighlights) {
        for (row_id, columns) in other.rows {
            self.rows.entry(row_id).or_default().extend(columns);
        }
    }

    /// How many row ids here belong to no row of this load. Reported and
    /// never fatal, exactly as the `cells` patch treats them. `"*"` is not a
    /// row id and does not count.
    pub fn unknown_rows(&self, items: &[NodeSummary]) -> usize {
        let known: HashSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
        self.rows
            .keys()
            .filter(|id| id.as_str() != ANY && !known.contains(id.as_str()))
            .count()
    }

    /// Whether anything here can reach this row at all — its own entry or the
    /// table-wide one.
    fn touches(&self, row_id: &str) -> bool {
        self.rows.contains_key(ANY) || self.rows.contains_key(row_id)
    }

    /// The style at exactly this address, if it paints on the table.
    fn at(&self, row_id: &str, column: &str) -> Option<(StyleLayer, StyleLayer)> {
        self.rows
            .get(row_id)?
            .get(column)
            .filter(|style| style.applies_to(HighlightMode::Table))
            .map(|style| (style.normal, style.selected_layer()))
    }

    /// The whole-row layers, least specific first: the table-wide entry, then
    /// this row's own.
    fn row_layers(&self, row_id: &str) -> Option<(StyleLayer, StyleLayer)> {
        stack(
            [self.at(ANY, ANY), self.at(row_id, ANY)]
                .into_iter()
                .flatten(),
        )
    }

    /// The same for one cell: the whole-column entry, then this row's.
    fn cell_layers(&self, row_id: &str, column: &str) -> Option<(StyleLayer, StyleLayer)> {
        stack(
            [self.at(ANY, column), self.at(row_id, column)]
                .into_iter()
                .flatten(),
        )
    }
}

/// One level's highlight rules, ready to paint a table.
pub struct TableHighlights<'a> {
    /// The columns in render order — a rule names them by `key`.
    pub columns: &'a [ColumnDef],
    /// The level's rules, already resolved by `prepare_view_file`.
    pub rules: &'a [HighlightRule],
    /// What the `load` hook asked for on these very rows.
    pub script: &'a ScriptHighlights,
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
        if prepared.is_empty() && self.script.is_empty() {
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
            let script_row = self.script.row_layers(&item.id);
            if hits.is_empty() && !self.script.touches(&item.id) {
                continue;
            }
            // The row layer goes on first, and its background becomes the
            // ground the cells sit on — otherwise a cell asking for an `auto`
            // foreground would contrast against a background the row rule has
            // already painted over.
            let mut ground = (self.row_bg, self.selected_bg);
            let row_layers = stack(
                hits.iter()
                    .filter(|p| p.is_row())
                    .map(|p| (p.normal, p.selected))
                    .chain(script_row),
            );
            if let Some((normal, selected)) = row_layers {
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
                let script_cell = self
                    .columns
                    .get(col_idx)
                    .and_then(|col| self.script.cell_layers(&item.id, &col.key));
                let Some((normal, selected)) = stack(
                    hits.iter()
                        .filter(|p| p.paints(col_idx))
                        .map(|p| (p.normal, p.selected))
                        .chain(script_cell),
                ) else {
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

/// Lay the layer pairs over one another in the order they arrive — the last
/// one to name a field wins it, modifiers accumulate. `None` when the
/// iterator is empty, i.e. nothing paints here.
fn stack(
    layers: impl Iterator<Item = (StyleLayer, StyleLayer)>,
) -> Option<(StyleLayer, StyleLayer)> {
    layers.fold(None, |acc: Option<(StyleLayer, StyleLayer)>, (n, s)| {
        Some(match acc {
            Some((an, asel)) => (an.layer(n), asel.layer(s)),
            None => (n, s),
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
        run_with(
            config,
            rows,
            row_items,
            items,
            theme,
            &ScriptHighlights::default(),
        )
    }

    /// The same, with a script's answer alongside the level's rules.
    fn run_with(
        config: &ViewFileConfig,
        rows: &mut [TableWidgetRow],
        row_items: &[usize],
        items: &[NodeSummary],
        theme: &Theme,
        script: &ScriptHighlights,
    ) -> Vec<Style> {
        let view = &config.views[0];
        TableHighlights {
            columns: &view.columns,
            rules: &view.highlights,
            script,
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

    // ── the script channel ───────────────────────────────────────────────

    /// A hook answer parsed against the same file the level comes from, so a
    /// script naming `hot` resolves through the file's `styles:`.
    fn script(config: &ViewFileConfig, answer: &str) -> ScriptHighlights {
        let theme = theme();
        let resolver = StyleResolver::new(&config.styles, theme.styles(), &theme);
        let answer: serde_json::Value = serde_json::from_str(answer).expect("answer parses");
        let (parsed, warnings) = ScriptHighlights::parse(&answer, &resolver).expect("shape ok");
        assert!(warnings.is_empty(), "{warnings:?}");
        parsed
    }

    #[test]
    fn a_script_paints_one_cell_by_row_id_and_column() {
        let config = level("      []");
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        let hl = script(
            &config,
            r##"{"highlights": {"Blocked": {"actual": "hot"}}}"##,
        );
        let styles = run_with(&config, &mut rows, &[0, 1], &items, &theme(), &hl);

        let cell = &rows[0].lines[0].cells[1];
        assert_eq!(
            styles[cell.style_id.unwrap() - 7].bg,
            Some(Color::Rgb(0x7a, 0x1c, 0x1c))
        );
        assert_eq!(rows[1].lines[0].cells[1].style_id, None);
        assert_eq!(
            rows[0].style_id, None,
            "a cell address is not a row address"
        );
    }

    #[test]
    fn the_star_axes_address_a_row_a_column_and_the_table() {
        let config = level("      []");
        let items = vec![item("Blocked", "3"), item("Open", "1")];
        let mut rows = vec![row(&["Blocked", "3"]), row(&["Open", "1"])];
        let hl = script(
            &config,
            r##"{"highlights": {
                 "*": {"*": {"bg": "#101010"}, "status": {"fg": "#445566"}},
                 "Blocked": {"*": {"bg": "#202020"}}
               }}"##,
        );
        let styles = run_with(&config, &mut rows, &[0, 1], &items, &theme(), &hl);
        let at = |id: usize| styles[id - 7];

        // Whole table, then the row's own on top of it.
        assert_eq!(
            at(rows[1].style_id.unwrap()).bg,
            Some(Color::Rgb(0x10, 0x10, 0x10))
        );
        assert_eq!(
            at(rows[0].style_id.unwrap()).bg,
            Some(Color::Rgb(0x20, 0x20, 0x20))
        );
        // Whole column, on every row.
        for r in &rows {
            let cell = &r.lines[0].cells[0];
            assert_eq!(
                at(cell.style_id.unwrap()).fg,
                Some(Color::Rgb(0x44, 0x55, 0x66))
            );
        }
    }

    #[test]
    fn a_script_layers_over_the_levels_own_rule() {
        // The rule sets the background, the script only the foreground —
        // they combine rather than one replacing the other.
        let config = level("      - columns: [actual]\n        style: hot");
        let items = vec![item("Open", "1")];
        let mut rows = vec![row(&["Open", "1"])];
        let hl = script(
            &config,
            r##"{"highlights": {"Open": {"actual": {"fg": "#445566"}}}}"##,
        );
        let styles = run_with(&config, &mut rows, &[0], &items, &theme(), &hl);

        let normal = styles[rows[0].lines[0].cells[1].style_id.unwrap() - 7];
        assert_eq!(
            normal.bg,
            Some(Color::Rgb(0x7a, 0x1c, 0x1c)),
            "from the rule"
        );
        assert_eq!(
            normal.fg,
            Some(Color::Rgb(0x44, 0x55, 0x66)),
            "from the script"
        );
    }

    #[test]
    fn a_later_script_has_the_last_word_on_the_same_address() {
        let config = level("      []");
        let mut first = script(
            &config,
            r##"{"highlights": {"Open": {"actual": {"bg": "#101010"}}}}"##,
        );
        let second = script(
            &config,
            r##"{"highlights": {"Open": {"actual": {"bg": "#202020"}}}}"##,
        );
        first.merge(second);

        let items = vec![item("Open", "1")];
        let mut rows = vec![row(&["Open", "1"])];
        let styles = run_with(&config, &mut rows, &[0], &items, &theme(), &first);
        let normal = styles[rows[0].lines[0].cells[1].style_id.unwrap() - 7];
        assert_eq!(normal.bg, Some(Color::Rgb(0x20, 0x20, 0x20)));
    }

    #[test]
    fn a_row_id_from_another_load_is_counted_not_painted() {
        let config = level("      []");
        let items = vec![item("Open", "1")];
        let hl = script(
            &config,
            r##"{"highlights": {"Gone": {"actual": "hot"}, "*": {"actual": "hot"}}}"##,
        );
        // `*` is an address, not a row id, and must not be counted.
        assert_eq!(hl.unknown_rows(&items), 1);

        let mut rows = vec![row(&["Open", "1"])];
        run_with(&config, &mut rows, &[0], &items, &theme(), &hl);
        assert_eq!(rows[0].lines[0].cells[1].style_id, Some(7));
    }

    #[test]
    fn an_unusable_style_costs_its_own_entry_and_nothing_else() {
        let config = level("      []");
        let theme = theme();
        let resolver = StyleResolver::new(&config.styles, theme.styles(), &theme);
        let answer: serde_json::Value = serde_json::from_str(
            r##"{"highlights": {"Open": {"actual": "nope", "status": "hot"}}}"##,
        )
        .unwrap();
        let (parsed, warnings) = ScriptHighlights::parse(&answer, &resolver).unwrap();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let items = vec![item("Open", "1")];
        let mut rows = vec![row(&["Open", "1"])];
        run_with(&config, &mut rows, &[0], &items, &theme, &parsed);
        assert_eq!(
            rows[0].lines[0].cells[0].style_id,
            Some(7),
            "the good one still paints"
        );
        assert_eq!(rows[0].lines[0].cells[1].style_id, None);
    }

    #[test]
    fn a_misshapen_highlights_key_is_refused_whole() {
        let config = level("      []");
        let theme = theme();
        let resolver = StyleResolver::new(&config.styles, theme.styles(), &theme);
        let answer: serde_json::Value =
            serde_json::from_str(r##"{"highlights": ["not", "an", "object"]}"##).unwrap();
        assert!(ScriptHighlights::parse(&answer, &resolver).is_err());
    }
}
