//! Card layout — render one logical row as a bordered card whose fields are
//! arranged in a grid.
//!
//! Where [`compute_table`](crate::layout::compute_table) puts every column of
//! a row on one line and [`compute_multiline_table`](crate::layout::compute_multiline_table)
//! distributes them over hand-listed lines, a card takes a flat field list and
//! *derives* the line stack from one number: how many fields sit next to each
//! other ([`CardSpec::columns`]). Six fields at three per line are two lines;
//! the line count is never configured, only implied.
//!
//! The result is a stack of [`ComputedCardLine`]s per row, each a sequence of
//! typed [`CardSpan`]s (border glyph, padding, field label, field value). The
//! rendering layer maps one span to one styled cell — the core stays
//! framework-agnostic and every glyph/color decision (which border style, which
//! theme color) is the caller's.
//!
//! Widths: all cards share one grid, so field *i* starts at the same column in
//! every card. Each grid column gets a weighted share of the inner width
//! (equal shares unless [`CardSpec::weights`] says otherwise) and the last
//! column absorbs the rounding remainder, so every content line is exactly as
//! wide as the card — the right border always lines up.

use std::hash::Hash;
use std::ops::Range;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cell::{fit_to_width, fit_to_width_with_highlights};
use crate::column::ColumnId;
use crate::row::Row;

/// Border drawn around each card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardBorder {
    /// No border. Cards are separated by [`CardSpec::gap`] alone.
    None,
    /// Square corners (`+`-free box drawing: `┌─┐`).
    Plain,
    /// Rounded corners (`╭─╮`). The default.
    #[default]
    Rounded,
}

/// The six glyphs a bordered card needs.
struct BorderGlyphs {
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
    horizontal: char,
    vertical: char,
}

impl CardBorder {
    fn glyphs(self) -> Option<BorderGlyphs> {
        match self {
            CardBorder::None => None,
            CardBorder::Plain => Some(BorderGlyphs {
                top_left: '┌',
                top_right: '┐',
                bottom_left: '└',
                bottom_right: '┘',
                horizontal: '─',
                vertical: '│',
            }),
            CardBorder::Rounded => Some(BorderGlyphs {
                top_left: '╭',
                top_right: '╮',
                bottom_left: '╰',
                bottom_right: '╯',
                horizontal: '─',
                vertical: '│',
            }),
        }
    }
}

/// Where a field's label is drawn relative to its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardLabels {
    /// No labels — values only.
    None,
    /// `Label: value` on one line. The default.
    #[default]
    Inline,
    /// Labels on their own line above the values, so each grid row of the
    /// card becomes two physical lines.
    Above,
}

/// One field of a card: the column it reads and the label it shows.
#[derive(Debug, Clone)]
pub struct CardField {
    pub column: ColumnId,
    /// Display label. Empty suppresses the label for this field even under
    /// [`CardLabels::Inline`] / [`CardLabels::Above`].
    pub label: String,
}

impl CardField {
    pub fn new(column: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            column: ColumnId::new(column),
            label: label.into(),
        }
    }
}

/// How a card is built. `fields` in reading order, `columns` fields per line.
#[derive(Debug, Clone)]
pub struct CardSpec {
    /// Fields in reading order; filled row-major into the grid.
    pub fields: Vec<CardField>,
    /// Fields side by side on one card line. Clamped to at least 1.
    pub columns: usize,
    /// Per-grid-column width weights. Empty (or the wrong length) → equal
    /// shares. `[1, 1, 2]` gives the third column half the inner width.
    pub weights: Vec<usize>,
    pub labels: CardLabels,
    pub border: CardBorder,
    /// Blank columns between the border and the content, left and right.
    pub padding: usize,
    /// Blank lines appended after each card. They never take the selection
    /// highlight, so cards read as separate blocks.
    pub gap: usize,
    /// Filler between two grid columns.
    pub separator: String,
    /// Rule drawn *between* two cards, the glyph repeated across the card
    /// width (`"─"` → `────…`). Empty (the default) → the cards are separated
    /// by [`Self::gap`] alone.
    ///
    /// The rule takes the place of the **last** gap line, so `gap: 1` plus a
    /// divider is exactly one ruled line instead of one blank one, and
    /// `gap: 0` still gets its rule. It is never drawn after the last card —
    /// there the plain `gap` lines remain.
    pub divider: String,
}

impl Default for CardSpec {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            columns: 1,
            weights: Vec::new(),
            labels: CardLabels::default(),
            border: CardBorder::default(),
            padding: 1,
            gap: 0,
            separator: "  ".to_string(),
            divider: String::new(),
        }
    }
}

/// What a [`CardSpan`] carries, so the renderer can style it without parsing
/// text back apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardSpanKind {
    /// A border glyph run (corners, horizontal rule, vertical bar).
    Border,
    /// Structural whitespace: padding, inter-column separator, filler for an
    /// empty grid slot.
    Chrome,
    /// A field label (including its `: ` suffix under [`CardLabels::Inline`]).
    Label,
    /// A field value.
    Value,
}

/// One styled run within a card line.
#[derive(Debug, Clone)]
pub struct CardSpan {
    pub kind: CardSpanKind,
    pub text: String,
    /// Index into [`CardSpec::fields`] for `Label`/`Value` spans; `None` for
    /// chrome and borders.
    pub field: Option<usize>,
    /// Char-index highlight ranges within `text` (e.g. fuzzy matches),
    /// projected onto the fitted string. Only ever non-empty on `Value`.
    pub highlights: Vec<Range<usize>>,
}

impl CardSpan {
    fn chrome(text: impl Into<String>) -> Self {
        Self {
            kind: CardSpanKind::Chrome,
            text: text.into(),
            field: None,
            highlights: Vec::new(),
        }
    }

    fn border(text: impl Into<String>) -> Self {
        Self {
            kind: CardSpanKind::Border,
            text: text.into(),
            field: None,
            highlights: Vec::new(),
        }
    }
}

/// One physical line of a card.
#[derive(Debug, Clone)]
pub struct ComputedCardLine {
    pub spans: Vec<CardSpan>,
    /// Whether the line is painted with the selection style when its card is
    /// selected. `false` on the gap spacers.
    pub highlight_on_select: bool,
}

impl ComputedCardLine {
    /// The line's text, borders and padding included. Convenience for tests
    /// and width assertions.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// One laid-out card: the row's id plus its physical lines.
#[derive(Debug, Clone)]
pub struct ComputedCard<Id: Eq + Hash + Clone> {
    pub id: Id,
    pub lines: Vec<ComputedCardLine>,
    pub selectable: bool,
}

/// The result of [`compute_cards`].
pub struct ComputedCards<Id: Eq + Hash + Clone> {
    pub cards: Vec<ComputedCard<Id>>,
    /// Width of each grid column, shared by every card.
    pub grid_widths: Vec<usize>,
    /// Total card width (borders and padding included).
    pub width: usize,
}

/// Repeat `pattern` to exactly `width` display columns — the inter-card rule.
///
/// Unlike [`fit_to_width`] this clips **without** an ellipsis: a `…` in the
/// middle of a `────` rule would read as a bug. A pattern whose glyphs do not
/// divide the width evenly loses its last (partial) glyph and the remainder is
/// padded with spaces.
fn rule_line(pattern: &str, width: usize) -> String {
    let mut out = String::with_capacity(width);
    let mut used = 0usize;
    if pattern
        .chars()
        .any(|c| UnicodeWidthChar::width(c).unwrap_or(0) > 0)
    {
        for ch in pattern.chars().cycle() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if w == 0 {
                continue;
            }
            if used + w > width {
                break;
            }
            out.push(ch);
            used += w;
        }
    }
    out.push_str(&" ".repeat(width - used));
    out
}

/// Distribute `content` columns over `n` weighted grid columns. The last
/// column takes the rounding remainder so the shares sum to `content`
/// exactly — that is what keeps the right border aligned.
fn distribute(content: usize, weights: &[usize]) -> Vec<usize> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let total: usize = weights.iter().sum();
    if total == 0 {
        return vec![content / n; n];
    }
    let mut widths = Vec::with_capacity(n);
    let mut used = 0usize;
    for (i, w) in weights.iter().enumerate() {
        let width = if i == n - 1 {
            content.saturating_sub(used)
        } else {
            content * w / total
        };
        widths.push(width);
        used += width;
    }
    widths
}

/// Lay out `rows` as cards per `spec`, fitted to `max_width`.
///
/// Every card gets the same grid, so a field sits at the same offset in each
/// one. Fields beyond the last full grid row leave their slots blank rather
/// than shrinking the card, so all cards have identical height.
pub fn compute_cards<Id>(rows: &[Row<Id>], spec: &CardSpec, max_width: usize) -> ComputedCards<Id>
where
    Id: Eq + Hash + Clone,
{
    let glyphs = spec.border.glyphs();
    let border_cost = if glyphs.is_some() { 2 } else { 0 };
    let cols = spec.columns.max(1);
    let sep_width = spec.separator.width() * cols.saturating_sub(1);

    // Inner width = what the fields share, after borders, padding and the
    // inter-column separators are paid for. Floored at one column per grid
    // column so a pathologically narrow pane still produces a stable shape
    // (the widget clips the overflow).
    let chrome = border_cost + 2 * spec.padding + sep_width;
    let content = max_width.saturating_sub(chrome).max(cols);

    let weights = if spec.weights.len() == cols {
        spec.weights.clone()
    } else {
        vec![1usize; cols]
    };
    let grid_widths = distribute(content, &weights);
    let inner_width: usize = grid_widths.iter().sum::<usize>() + sep_width;
    let width = inner_width + 2 * spec.padding + border_cost;

    let grid_rows = spec.fields.len().div_ceil(cols).max(1);
    let padding = " ".repeat(spec.padding);

    let horizontal_rule = glyphs.as_ref().map(|g| {
        let bar: String =
            std::iter::repeat_n(g.horizontal, inner_width + 2 * spec.padding).collect();
        (
            format!("{}{}{}", g.top_left, bar, g.top_right),
            format!("{}{}{}", g.bottom_left, bar, g.bottom_right),
        )
    });

    // The inter-card rule, pre-fitted to the card width once (it is identical
    // for every card).
    let divider_line = (!spec.divider.is_empty()).then(|| rule_line(&spec.divider, width));

    let cards = rows
        .iter()
        .enumerate()
        .map(|(row_idx, row)| {
            let last_row = row_idx + 1 == rows.len();
            let mut lines: Vec<ComputedCardLine> = Vec::new();

            if let Some((top, _)) = &horizontal_rule {
                lines.push(ComputedCardLine {
                    spans: vec![CardSpan::border(top.clone())],
                    highlight_on_select: true,
                });
            }

            for grid_row in 0..grid_rows {
                let slots: Vec<Option<usize>> = (0..cols)
                    .map(|c| {
                        let idx = grid_row * cols + c;
                        (idx < spec.fields.len()).then_some(idx)
                    })
                    .collect();

                match spec.labels {
                    CardLabels::Above => {
                        lines.push(build_line(
                            spec,
                            &glyphs,
                            &padding,
                            &grid_widths,
                            &slots,
                            |field_idx, width| {
                                let label = &spec.fields[field_idx].label;
                                vec![CardSpan {
                                    kind: CardSpanKind::Label,
                                    text: fit_to_width(label, width),
                                    field: Some(field_idx),
                                    highlights: Vec::new(),
                                }]
                            },
                        ));
                        lines.push(build_line(
                            spec,
                            &glyphs,
                            &padding,
                            &grid_widths,
                            &slots,
                            |field_idx, width| vec![value_span(row, spec, field_idx, width)],
                        ));
                    }
                    CardLabels::Inline | CardLabels::None => {
                        lines.push(build_line(
                            spec,
                            &glyphs,
                            &padding,
                            &grid_widths,
                            &slots,
                            |field_idx, width| {
                                let label = &spec.fields[field_idx].label;
                                let inline = spec.labels == CardLabels::Inline && !label.is_empty();
                                if !inline {
                                    return vec![value_span(row, spec, field_idx, width)];
                                }
                                let label_text = format!("{label}: ");
                                let label_width = label_text.width();
                                if label_width >= width {
                                    // No room for a value next to the label:
                                    // keep the label (it identifies the slot)
                                    // and let it be truncated.
                                    return vec![CardSpan {
                                        kind: CardSpanKind::Label,
                                        text: fit_to_width(&label_text, width),
                                        field: Some(field_idx),
                                        highlights: Vec::new(),
                                    }];
                                }
                                vec![
                                    CardSpan {
                                        kind: CardSpanKind::Label,
                                        text: label_text,
                                        field: Some(field_idx),
                                        highlights: Vec::new(),
                                    },
                                    value_span(row, spec, field_idx, width - label_width),
                                ]
                            },
                        ));
                    }
                }
            }

            if let Some((_, bottom)) = &horizontal_rule {
                lines.push(ComputedCardLine {
                    spans: vec![CardSpan::border(bottom.clone())],
                    highlight_on_select: true,
                });
            }

            // Trailing space, then — unless this is the last card — the rule,
            // which takes the place of the last gap line. `gap: 1` + divider
            // is therefore one ruled line, not a blank one plus a rule.
            let draw_divider = divider_line.is_some() && !last_row;
            let blanks = if draw_divider {
                spec.gap.saturating_sub(1)
            } else {
                spec.gap
            };
            for _ in 0..blanks {
                lines.push(ComputedCardLine {
                    spans: Vec::new(),
                    highlight_on_select: false,
                });
            }
            if draw_divider {
                lines.push(ComputedCardLine {
                    spans: vec![CardSpan::border(divider_line.clone().unwrap())],
                    highlight_on_select: false,
                });
            }

            ComputedCard {
                id: row.id.clone(),
                lines,
                selectable: row.selectable,
            }
        })
        .collect();

    ComputedCards {
        cards,
        grid_widths,
        width,
    }
}

/// The fitted value span of one field, carrying its projected highlights.
fn value_span<Id: Eq + Hash>(
    row: &Row<Id>,
    spec: &CardSpec,
    field_idx: usize,
    width: usize,
) -> CardSpan {
    let content = row.cells.get(&spec.fields[field_idx].column);
    let (text, ranges) = match content {
        Some(c) => (
            c.text.as_str(),
            c.spans.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
        ),
        None => ("", Vec::new()),
    };
    let (fitted, highlights) = fit_to_width_with_highlights(text, width, &ranges);
    CardSpan {
        kind: CardSpanKind::Value,
        text: fitted,
        field: Some(field_idx),
        highlights,
    }
}

/// Assemble one content line: left border, padding, the grid columns joined by
/// the separator, padding, right border. `cell` renders one occupied slot into
/// spans of exactly the given width; empty slots become chrome filler.
fn build_line<F>(
    spec: &CardSpec,
    glyphs: &Option<BorderGlyphs>,
    padding: &str,
    grid_widths: &[usize],
    slots: &[Option<usize>],
    mut cell: F,
) -> ComputedCardLine
where
    F: FnMut(usize, usize) -> Vec<CardSpan>,
{
    let mut spans: Vec<CardSpan> = Vec::new();
    if let Some(g) = glyphs {
        spans.push(CardSpan::border(g.vertical.to_string()));
    }
    if !padding.is_empty() {
        spans.push(CardSpan::chrome(padding));
    }
    for (c, slot) in slots.iter().enumerate() {
        if c > 0 && !spec.separator.is_empty() {
            spans.push(CardSpan::chrome(spec.separator.clone()));
        }
        let width = grid_widths.get(c).copied().unwrap_or(0);
        match slot {
            Some(field_idx) => spans.extend(cell(*field_idx, width)),
            None => spans.push(CardSpan::chrome(" ".repeat(width))),
        }
    }
    if !padding.is_empty() {
        spans.push(CardSpan::chrome(padding));
    }
    if let Some(g) = glyphs {
        spans.push(CardSpan::border(g.vertical.to_string()));
    }
    ComputedCardLine {
        spans,
        highlight_on_select: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_2x3() -> CardSpec {
        CardSpec {
            fields: vec![
                CardField::new("key", "Key"),
                CardField::new("status", "Status"),
                CardField::new("assignee", "Assignee"),
                CardField::new("summary", "Summary"),
                CardField::new("creator", "Creator"),
                CardField::new("updated", "Updated"),
            ],
            columns: 3,
            ..CardSpec::default()
        }
    }

    fn row(id: u32) -> Row<u32> {
        Row::new(id)
            .cell("key", "ABC-1")
            .cell("status", "Open")
            .cell("assignee", "someone")
            .cell("summary", "a summary line")
            .cell("creator", "author")
            .cell("updated", "yesterday")
    }

    #[test]
    fn six_fields_at_three_columns_yield_two_content_lines() {
        // The line count is derived, never configured: 6 fields / 3 columns.
        let cards = compute_cards(&[row(1)], &spec_2x3(), 80);
        let card = &cards.cards[0];
        // top border + 2 content lines + bottom border
        assert_eq!(card.lines.len(), 4);
        assert_eq!(cards.grid_widths.len(), 3);
    }

    #[test]
    fn every_line_is_exactly_the_card_width() {
        // The right border can only align if each line fits the card width
        // exactly — including the remainder-absorbing last grid column.
        // 80 - 2 border - 2 padding - 4 separators = 72 / 3 = 24 each.
        let cards = compute_cards(&[row(1)], &spec_2x3(), 80);
        assert_eq!(cards.width, 80);
        assert_eq!(cards.grid_widths, vec![24, 24, 24]);
        for line in &cards.cards[0].lines {
            assert_eq!(line.text().width(), 80, "line: {:?}", line.text());
        }
    }

    #[test]
    fn width_not_divisible_by_columns_still_aligns() {
        // 79 - 2 - 2 - 4 = 71 → 23/23/25: the last column absorbs the
        // remainder so no line is a column short.
        let cards = compute_cards(&[row(1)], &spec_2x3(), 79);
        assert_eq!(cards.grid_widths.iter().sum::<usize>(), 71);
        for line in &cards.cards[0].lines {
            assert_eq!(line.text().width(), 79);
        }
    }

    #[test]
    fn inline_labels_precede_their_value() {
        let cards = compute_cards(&[row(1)], &spec_2x3(), 80);
        let first = &cards.cards[0].lines[1];
        let labels: Vec<&str> = first
            .spans
            .iter()
            .filter(|s| s.kind == CardSpanKind::Label)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(labels, vec!["Key: ", "Status: ", "Assignee: "]);
        let values: Vec<String> = first
            .spans
            .iter()
            .filter(|s| s.kind == CardSpanKind::Value)
            .map(|s| s.text.trim_end().to_string())
            .collect();
        assert_eq!(values, vec!["ABC-1", "Open", "someone"]);
        // Second content line carries the second grid row's fields.
        let second = &cards.cards[0].lines[2];
        let labels2: Vec<&str> = second
            .spans
            .iter()
            .filter(|s| s.kind == CardSpanKind::Label)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(labels2, vec!["Summary: ", "Creator: ", "Updated: "]);
    }

    #[test]
    fn labels_above_double_the_content_lines() {
        let spec = CardSpec {
            labels: CardLabels::Above,
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 80);
        // top + (label,value) × 2 grid rows + bottom
        assert_eq!(cards.cards[0].lines.len(), 6);
        let label_line = &cards.cards[0].lines[1];
        assert!(
            label_line
                .spans
                .iter()
                .all(|s| s.kind != CardSpanKind::Value)
        );
        let value_line = &cards.cards[0].lines[2];
        assert!(
            value_line
                .spans
                .iter()
                .all(|s| s.kind != CardSpanKind::Label)
        );
    }

    #[test]
    fn labels_none_shows_values_only() {
        let spec = CardSpec {
            labels: CardLabels::None,
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 80);
        assert!(
            cards.cards[0]
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .all(|s| s.kind != CardSpanKind::Label)
        );
    }

    #[test]
    fn trailing_slots_stay_blank_so_cards_keep_one_height() {
        // 4 fields at 3 columns = 2 grid rows with 2 empty slots. The card
        // must still be 2 content lines wide-open, not a ragged 1.5.
        let mut spec = spec_2x3();
        spec.fields.truncate(4);
        let cards = compute_cards(&[row(1), row(2)], &spec, 80);
        for card in &cards.cards {
            assert_eq!(card.lines.len(), 4);
            for line in &card.lines {
                assert_eq!(line.text().width(), 80);
            }
        }
        let second_row_line = &cards.cards[0].lines[2];
        // Only the first slot is occupied; the rest is chrome filler.
        assert_eq!(
            second_row_line
                .spans
                .iter()
                .filter(|s| s.kind == CardSpanKind::Value)
                .count(),
            1
        );
    }

    #[test]
    fn weights_give_a_column_a_bigger_share() {
        let spec = CardSpec {
            weights: vec![1, 1, 2],
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 80);
        // content 72 → 18 / 18 / 36 (last absorbs the remainder).
        assert_eq!(cards.grid_widths, vec![18, 18, 36]);
        for line in &cards.cards[0].lines {
            assert_eq!(line.text().width(), 80);
        }
    }

    #[test]
    fn value_is_truncated_to_its_slot() {
        let mut spec = spec_2x3();
        spec.fields.truncate(3);
        let row = Row::new(1u32)
            .cell("key", "ABC-1")
            .cell("status", "Open")
            .cell("assignee", "a-very-long-assignee-name-that-cannot-fit");
        let cards = compute_cards(&[row], &spec, 40);
        // 40 - 2 - 2 - 4 = 32 → 10/10/12; "Assignee: " is 10 wide, leaving 2.
        let line = &cards.cards[0].lines[1];
        let last = line
            .spans
            .iter()
            .filter(|s| s.kind == CardSpanKind::Value)
            .next_back()
            .unwrap();
        assert_eq!(last.text.width(), 2);
        assert!(last.text.ends_with('…'));
        assert_eq!(line.text().width(), 40);
    }

    #[test]
    fn label_wider_than_slot_keeps_the_label() {
        // A slot too narrow for `Label: value` shows the (truncated) label
        // rather than an anonymous value.
        let mut spec = spec_2x3();
        spec.fields = vec![CardField::new("assignee", "Assignee")];
        spec.columns = 1;
        let cards = compute_cards(&[row(1)], &spec, 8);
        let line = &cards.cards[0].lines[1];
        assert_eq!(
            line.spans
                .iter()
                .filter(|s| s.kind == CardSpanKind::Value)
                .count(),
            0
        );
        assert_eq!(line.text().width(), 8);
    }

    #[test]
    fn borderless_and_gapped_cards() {
        let spec = CardSpec {
            border: CardBorder::None,
            gap: 1,
            padding: 0,
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 80);
        // 2 content lines + 1 gap spacer, no borders.
        assert_eq!(cards.cards[0].lines.len(), 3);
        let spacer = cards.cards[0].lines.last().unwrap();
        assert!(spacer.spans.is_empty());
        assert!(!spacer.highlight_on_select);
        assert_eq!(cards.grid_widths.iter().sum::<usize>(), 76);
    }

    #[test]
    fn divider_replaces_the_last_gap_line_between_cards() {
        let spec = CardSpec {
            border: CardBorder::None,
            gap: 1,
            padding: 0,
            divider: "─".to_string(),
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1), row(2)], &spec, 80);
        // 2 content lines + the rule (which took the single gap line's place).
        assert_eq!(cards.cards[0].lines.len(), 3);
        let rule = cards.cards[0].lines.last().unwrap();
        assert_eq!(rule.text(), "─".repeat(80));
        assert_eq!(rule.spans[0].kind, CardSpanKind::Border);
        assert!(
            !rule.highlight_on_select,
            "the rule is not part of a selection"
        );
        // The last card keeps its plain gap line — no trailing rule.
        let tail = cards.cards[1].lines.last().unwrap();
        assert!(tail.spans.is_empty());
        assert_eq!(cards.cards[1].lines.len(), 3);
    }

    #[test]
    fn divider_is_drawn_with_gap_zero_and_spans_a_bordered_card() {
        let spec = CardSpec {
            gap: 0,
            divider: "─".to_string(),
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1), row(2)], &spec, 80);
        // top + 2 content + bottom + rule.
        assert_eq!(cards.cards[0].lines.len(), 5);
        let rule = cards.cards[0].lines.last().unwrap();
        assert_eq!(
            rule.text().width(),
            cards.width,
            "the rule spans the whole card, borders included"
        );
        assert_eq!(cards.cards[1].lines.len(), 4, "no rule after the last card");
    }

    #[test]
    fn multi_char_divider_is_repeated_and_clipped_to_the_card_width() {
        let spec = CardSpec {
            border: CardBorder::None,
            padding: 0,
            divider: "-·".to_string(),
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1), row(2)], &spec, 9);
        let rule = cards.cards[0].lines.last().unwrap();
        assert_eq!(rule.text(), "-·-·-·-·-", "odd width clips mid-pattern");
        assert_eq!(rule.text().width(), cards.width);
    }

    #[test]
    fn a_single_card_never_gets_a_divider() {
        let spec = CardSpec {
            border: CardBorder::None,
            gap: 1,
            padding: 0,
            divider: "─".to_string(),
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 40);
        let tail = cards.cards[0].lines.last().unwrap();
        assert!(tail.spans.is_empty(), "sole card keeps its blank gap line");
    }

    #[test]
    fn plain_border_uses_square_corners() {
        let spec = CardSpec {
            border: CardBorder::Plain,
            ..spec_2x3()
        };
        let cards = compute_cards(&[row(1)], &spec, 80);
        assert!(cards.cards[0].lines[0].text().starts_with('┌'));
        assert!(cards.cards[0].lines.last().unwrap().text().starts_with('└'));
    }

    #[test]
    fn highlights_survive_fitting() {
        use crate::cell::{CellContent, StyledSpan};

        let mut spec = spec_2x3();
        spec.fields = vec![CardField::new("summary", "Summary")];
        spec.columns = 1;
        let content = CellContent::text("hello world").with_spans(vec![StyledSpan {
            range: 0..5,
            style_id: 7,
        }]);
        let row = Row::new(1u32).cell("summary", content);
        let cards = compute_cards(&[row], &spec, 40);
        let value = cards.cards[0].lines[1]
            .spans
            .iter()
            .find(|s| s.kind == CardSpanKind::Value)
            .unwrap();
        assert_eq!(value.highlights, vec![0..5]);
    }

    #[test]
    fn missing_cell_renders_as_blank_value() {
        let mut spec = spec_2x3();
        spec.fields = vec![CardField::new("nope", "Nope")];
        spec.columns = 1;
        let cards = compute_cards(&[Row::new(1u32)], &spec, 40);
        let value = cards.cards[0].lines[1]
            .spans
            .iter()
            .find(|s| s.kind == CardSpanKind::Value)
            .unwrap();
        assert!(value.text.trim().is_empty());
        assert_eq!(cards.cards[0].lines[1].text().width(), 40);
    }

    #[test]
    fn non_selectable_row_stays_non_selectable() {
        let cards = compute_cards(&[row(1).not_selectable()], &spec_2x3(), 80);
        assert!(!cards.cards[0].selectable);
    }

    #[test]
    fn narrow_pane_does_not_panic() {
        // Chrome alone exceeds the pane: every grid column floors at 1 and the
        // widget clips the overflow.
        let cards = compute_cards(&[row(1)], &spec_2x3(), 4);
        assert_eq!(cards.grid_widths, vec![1, 1, 1]);
        assert!(!cards.cards[0].lines.is_empty());
    }
}
