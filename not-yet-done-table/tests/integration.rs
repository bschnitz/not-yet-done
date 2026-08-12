//! Integration tests using CharBuf rendering.

use not_yet_done_table::*;
use std::collections::HashMap;

fn config(max_width: usize, strategies: Vec<(&str, ColStrategy)>) -> TableConfig {
    let mut m = HashMap::new();
    for (name, strat) in strategies {
        m.insert(ColumnId::new(name), strat);
    }
    TableConfig {
        max_width,
        separator: "  ".to_string(),
        sizer: Box::new(MixedColSizer { strategies: m }),
    }
}

#[test]
fn render_simple_table() {
    let cols = vec![ColumnId::new("name"), ColumnId::new("age")];
    let cfg = config(
        25,
        vec![("name", ColStrategy::Flex(1)), ("age", ColStrategy::Max)],
    );

    let rows = vec![
        Row::new(1u32).cell("name", "Alice").cell("age", "30"),
        Row::new(2u32).cell("name", "Bob").cell("age", "7"),
    ];
    let header = Row::new(0u32).cell("name", "Name").cell("age", "Age");

    let table = compute_table(&rows, &cfg, &cols, Some(&header));
    let mut buf = CharBuf::new(25, 3);
    render_to_target(&mut buf, &table, "  ");

    // age=Max→3 (header "Age"), name=Flex→25-2-3=20
    assert_eq!(buf.row_str(0), "Name                  Age");
    assert_eq!(buf.row_str(1), "Alice                 30 ");
    assert_eq!(buf.row_str(2), "Bob                   7  ");
}

#[test]
fn render_right_aligned() {
    let cols = vec![ColumnId::new("desc"), ColumnId::new("num")];
    let cfg = config(
        20,
        vec![
            ("desc", ColStrategy::Fixed(10)),
            ("num", ColStrategy::Fixed(6)),
        ],
    );

    let rows = vec![
        Row::new(1u32)
            .cell("desc", "Fix bug")
            .cell("num", CellContent::aligned("42", CellAlignment::Right)),
        Row::new(2u32)
            .cell("desc", "Add tests")
            .cell("num", CellContent::aligned("7", CellAlignment::Right)),
    ];

    let table = compute_table(&rows, &cfg, &cols, None);
    let mut buf = CharBuf::new(20, 2);
    render_to_target(&mut buf, &table, "  ");

    // desc=Fixed(10), sep=2, num=Fixed(6) → 10+2+6=18, buf=20
    assert_eq!(buf.row_str(0), "Fix bug         42  ");
    assert_eq!(buf.row_str(1), "Add tests        7  ");
}

#[test]
fn non_selectable_rows_preserved() {
    let cols = vec![ColumnId::new("x")];
    let cfg = config(20, vec![("x", ColStrategy::Flex(1))]);

    let rows = vec![
        Row::new(1u32).cell("x", "── Group ──").not_selectable(),
        Row::new(2u32).cell("x", "Item A"),
        Row::new(3u32).cell("x", "Item B"),
    ];

    let table = compute_table(&rows, &cfg, &cols, None);
    assert!(!table.rows[0].selectable);
    assert!(table.rows[1].selectable);
    assert!(table.rows[2].selectable);
}

#[test]
fn truncation_preserves_alignment() {
    let cols = vec![ColumnId::new("a")];
    let cfg = config(8, vec![("a", ColStrategy::Fixed(8))]);

    let rows = vec![Row::new(1u32).cell("a", "A very long description")];

    let table = compute_table(&rows, &cfg, &cols, None);
    let mut buf = CharBuf::new(8, 1);
    render_to_target(&mut buf, &table, "");

    assert_eq!(buf.row_str(0), "A very …");
}

#[test]
fn center_aligned_column() {
    let cols = vec![ColumnId::new("title")];
    let cfg = config(20, vec![("title", ColStrategy::Fixed(20))]);

    let rows = vec![Row::new(1u32).cell(
        "title",
        CellContent::aligned("Hello", CellAlignment::Center),
    )];

    let table = compute_table(&rows, &cfg, &cols, None);
    let mut buf = CharBuf::new(20, 1);
    render_to_target(&mut buf, &table, "");

    // "Hello" = 5 chars, centered in 20 → 7 left, 8 right
    assert_eq!(buf.row_str(0), "       Hello        ");
}

#[test]
fn right_aligned_with_highlights_shifts_ranges() {
    let cols = vec![ColumnId::new("val")];
    let cfg = config(10, vec![("val", ColStrategy::Fixed(10))]);

    let content = CellContent::aligned("42", CellAlignment::Right).with_spans(vec![StyledSpan {
        range: 0..2,
        style_id: 0,
    }]);

    let rows = vec![Row::new(1u32).cell("val", content)];

    let table = compute_table(&rows, &cfg, &cols, None);
    // "42" right-aligned in 10 → "        42"
    assert_eq!(table.rows[0].cells[0], "        42");
    // Highlight range shifted by 8 (left padding).
    assert_eq!(table.rows[0].highlights[0], vec![8..10]);
}

#[test]
fn grouped_cell_spans_columns() {
    let cell = GroupedCell::new("Total: 42h", 3);
    let fitted = cell.fit(&[8, 8, 8], 2);
    // 8+2+8+2+8 = 28 chars
    assert_eq!(fitted.chars().count(), 28);
    assert!(fitted.starts_with("Total: 42h"));
}

#[test]
fn grouped_cell_right_aligned() {
    let cell = GroupedCell::aligned("99h", CellAlignment::Right, 2);
    let fitted = cell.fit(&[10, 10], 2);
    // 10+2+10 = 22 chars, "99h" right-aligned
    assert_eq!(fitted.chars().count(), 22);
    assert!(fitted.ends_with("99h"));
}
