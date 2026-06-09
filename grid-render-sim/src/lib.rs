// Re-export everything from the core library so existing test code that
// imports `grid_render_sim::types::*` etc. continues to compile unchanged.
pub use not_yet_done_grid_core::layout;
pub use not_yet_done_grid_core::render;
pub use not_yet_done_grid_core::types;

pub use not_yet_done_grid_core::{
    BorderChars, BorderPos, BorderText, CellGroup, GapPos, GapSlot, GridConfig,
    SpannedBorder, TextAnchor,
    BORDER_DASHED, BORDER_DASHED_EXTENDED,
    BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
    BORDER_DOUBLE_EXTENDED,
    BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED,
    BORDER_SIMPLE, BORDER_SIMPLE_EXTENDED,
    BORDER_THICK_EXTENDED,
    GridLayout, compute_layout,
    CharBuf, RenderTarget, draw_borders,
};

// ── Sim-specific rendering helpers ────────────────────────────────────────────

const CELL_BG: [char; 3] = ['▓', '░', '█'];
const GROUP_BG: char     = '╳';

fn cell_bg(row: usize, col: usize) -> char {
    // Use 2*row + col so that adjacent cells always have different backgrounds
    // regardless of the column count.
    CELL_BG[(2 * row + col) % CELL_BG.len()]
}

/// Render the gap/border skeleton only (no cell content).
pub fn render_gaps_and_borders(cfg: &GridConfig, layout: &GridLayout) -> Vec<String> {
    let mut buf = CharBuf::new(layout.total_width as usize, layout.total_height as usize);
    draw_borders(cfg, layout, &mut buf);
    buf.into_lines()
}

/// Render gap/border skeleton **plus** cell background characters
/// (`▓` `░` `█` cycling per the simulation spec, `╳` for grouped cells).
pub fn render_with_cells(cfg: &GridConfig, layout: &GridLayout) -> Vec<String> {
    let mut buf = CharBuf::new(layout.total_width as usize, layout.total_height as usize);
    fill_cell_backgrounds(cfg, layout, &mut buf);
    draw_borders(cfg, layout, &mut buf);
    buf.into_lines()
}

fn fill_cell_backgrounds(cfg: &GridConfig, layout: &GridLayout, buf: &mut CharBuf) {
    let mut visited: Vec<*const CellGroup> = Vec::new();

    for row in 0..cfg.rows {
        for col in 0..cfg.cols {
            match cfg.group_of(row, col) {
                Some(group) => {
                    let ptr = group as *const _;
                    if visited.contains(&ptr) { continue; }
                    visited.push(ptr);
                    let (fr, fc, lr, lc) = GridConfig::group_bounds(cfg.rows, cfg.cols, group);
                    let rect = layout.group_rect(fr, fc, lr, lc);
                    fill_rect(buf, rect.x, rect.y, rect.width, rect.height, GROUP_BG);
                }
                None => {
                    fill_rect(
                        buf,
                        layout.col_x[col], layout.row_y[row],
                        layout.col_w[col], layout.row_h[row],
                        cell_bg(row, col),
                    );
                }
            }
        }
    }
}

fn fill_rect(buf: &mut CharBuf, x: u16, y: u16, w: u16, h: u16, ch: char) {
    for dy in 0..h {
        for dx in 0..w {
            buf.put_char(x + dx, y + dy, ch);
        }
    }
}

// ── Tests (moved here from the old lib.rs) ────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::layout::{Constraint, Rect};
    use crate::*;

    fn make_3x3(col_len: u16, row_len: u16) -> GridConfig {
        let mut cfg = GridConfig::new(3, 3);
        cfg.col_constraints = vec![Constraint::Length(col_len); 3];
        cfg.row_constraints = vec![Constraint::Length(row_len); 3];
        cfg
    }

    fn make_3x5(col_len: u16, row_len: u16) -> GridConfig {
        let mut cfg = GridConfig::new(3, 5);
        cfg.col_constraints = vec![Constraint::Length(col_len); 5];
        cfg.row_constraints = vec![Constraint::Length(row_len); 3];
        cfg
    }

    fn render(cfg: &GridConfig) -> Vec<String> {
        let area = Rect::new(0, 0, cfg.total_width_hint(), cfg.total_height_hint());
        let layout = compute_layout(cfg, area);
        render_with_cells(cfg, &layout)
    }

    fn print_grid(label: &str, lines: &[String]) {
        println!("── {} ──", label);
        for line in lines { println!("{}", line); }
        println!();
    }

    fn assert_grid(label: &str, lines: &[String], expected: &[&str]) {
        print_grid(label, lines);
        assert_eq!(
            lines.len(), expected.len(),
            "{label}: height mismatch (got {}, want {})",
            lines.len(), expected.len()
        );
        for (i, (got, want)) in lines.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                got.as_str(), *want,
                "{label}: row {i}\n  got : {got:?}\n  want: {want:?}"
            );
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 1 — Outer border only
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_outer_border_simple() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        assert_grid("outer_border_simple", &render(&cfg), &[
            "┌─────────────────────┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_outer_border_rounded() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_ROUNDED);
        assert_grid("outer_border_rounded", &render(&cfg), &[
            "╭─────────────────────╮",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "╰─────────────────────╯",
        ]);
    }

    #[test]
    fn test_outer_border_double_extended() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_DOUBLE_EXTENDED);
        assert_grid("outer_border_double_extended", &render(&cfg), &[
            "╔═════════════════════╗",
            "║▓▓▓▓▓▓▓░░░░░░░███████║",
            "║▓▓▓▓▓▓▓░░░░░░░███████║",
            "║▓▓▓▓▓▓▓░░░░░░░███████║",
            "║███████▓▓▓▓▓▓▓░░░░░░░║",
            "║███████▓▓▓▓▓▓▓░░░░░░░║",
            "║███████▓▓▓▓▓▓▓░░░░░░░║",
            "║░░░░░░░███████▓▓▓▓▓▓▓║",
            "║░░░░░░░███████▓▓▓▓▓▓▓║",
            "║░░░░░░░███████▓▓▓▓▓▓▓║",
            "╚═════════════════════╝",
        ]);
    }

    #[test]
    fn test_outer_border_thick_extended() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_THICK_EXTENDED);
        assert_grid("outer_border_thick_extended", &render(&cfg), &[
            "┏━━━━━━━━━━━━━━━━━━━━━┓",
            "┃▓▓▓▓▓▓▓░░░░░░░███████┃",
            "┃▓▓▓▓▓▓▓░░░░░░░███████┃",
            "┃▓▓▓▓▓▓▓░░░░░░░███████┃",
            "┃███████▓▓▓▓▓▓▓░░░░░░░┃",
            "┃███████▓▓▓▓▓▓▓░░░░░░░┃",
            "┃███████▓▓▓▓▓▓▓░░░░░░░┃",
            "┃░░░░░░░███████▓▓▓▓▓▓▓┃",
            "┃░░░░░░░███████▓▓▓▓▓▓▓┃",
            "┃░░░░░░░███████▓▓▓▓▓▓▓┃",
            "┗━━━━━━━━━━━━━━━━━━━━━┛",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 2 — Inner full borders
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_inner_v_border() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        assert_grid("inner_v_border", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░╵███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_inner_v_border_extended() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE_EXTENDED);
        assert_grid("inner_v_border_extended", &render(&cfg), &[
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_inner_h_border() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        assert_grid("inner_h_border", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╶───────────────────╴",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_inner_h_border_extended() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE_EXTENDED);
        assert_grid("inner_h_border_extended", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "─────────────────────",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_inner_v_and_h_crossing() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        assert_grid("inner_v_and_h_crossing", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "╶──────┼─────────────╴",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░╵███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_inner_v_and_h_crossing_extended() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE_EXTENDED);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE_EXTENDED);
        assert_grid("inner_v_and_h_crossing_extended", &render(&cfg), &[
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "───────┼──────────────",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_outer_with_inner_borders() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        assert_grid("outer_with_inner_borders", &render(&cfg), &[
            "┌───────┬──────────────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░███████│",
            "│▓▓▓▓▓▓▓│░░░░░░░███████│",
            "│▓▓▓▓▓▓▓│░░░░░░░███████│",
            "├───────┼──────────────┤",
            "│███████│▓▓▓▓▓▓▓░░░░░░░│",
            "│███████│▓▓▓▓▓▓▓░░░░░░░│",
            "│███████│▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░│███████▓▓▓▓▓▓▓│",
            "│░░░░░░░│███████▓▓▓▓▓▓▓│",
            "│░░░░░░░│███████▓▓▓▓▓▓▓│",
            "└───────┴──────────────┘",
        ]);
    }

    #[test]
    fn test_outer_double_inner_simple_no_join() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_DOUBLE_EXTENDED);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        assert_grid("outer_double_inner_simple_no_join", &render(&cfg), &[
            "╔══════════════════════╗",
            "║▓▓▓▓▓▓▓╷░░░░░░░███████║",
            "║▓▓▓▓▓▓▓│░░░░░░░███████║",
            "║▓▓▓▓▓▓▓│░░░░░░░███████║",
            "║╶──────┼─────────────╴║",
            "║███████│▓▓▓▓▓▓▓░░░░░░░║",
            "║███████│▓▓▓▓▓▓▓░░░░░░░║",
            "║███████│▓▓▓▓▓▓▓░░░░░░░║",
            "║░░░░░░░│███████▓▓▓▓▓▓▓║",
            "║░░░░░░░│███████▓▓▓▓▓▓▓║",
            "║░░░░░░░╵███████▓▓▓▓▓▓▓║",
            "╚══════════════════════╝",
        ]);
    }

    #[test]
    fn test_outer_rounded_inner_thick_no_join() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_ROUNDED);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_THICK_EXTENDED);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_THICK_EXTENDED);
        assert_grid("outer_rounded_inner_thick_no_join", &render(&cfg), &[
            "╭──────────────────────╮",
            "│▓▓▓▓▓▓▓┃░░░░░░░███████│",
            "│▓▓▓▓▓▓▓┃░░░░░░░███████│",
            "│▓▓▓▓▓▓▓┃░░░░░░░███████│",
            "│━━━━━━━╋━━━━━━━━━━━━━━│",
            "│███████┃▓▓▓▓▓▓▓░░░░░░░│",
            "│███████┃▓▓▓▓▓▓▓░░░░░░░│",
            "│███████┃▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░┃███████▓▓▓▓▓▓▓│",
            "│░░░░░░░┃███████▓▓▓▓▓▓▓│",
            "│░░░░░░░┃███████▓▓▓▓▓▓▓│",
            "╰──────────────────────╯",
        ]);
    }

    #[test]
    fn test_all_inner_borders_full_grid() {
        let mut cfg = make_3x5(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        for i in 0..4 { cfg.apply_border_pos(&BorderPos::AfterCol(i), &BORDER_SIMPLE); }
        for i in 0..2 { cfg.apply_border_pos(&BorderPos::AfterRow(i), &BORDER_SIMPLE); }
        assert_grid("all_inner_borders_full_grid", &render(&cfg), &[
            "┌───────┬───────┬───────┬───────┬───────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│",
            "├───────┼───────┼───────┼───────┼───────┤",
            "│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│",
            "│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│",
            "│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│",
            "├───────┼───────┼───────┼───────┼───────┤",
            "│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│",
            "│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│",
            "│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│",
            "└───────┴───────┴───────┴───────┴───────┘",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 3 — Gaps only
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_v_gap_only() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_gap_pos(&GapPos::AfterCol(0));
        let lines = render(&cfg);
        print_grid("v_gap_only", &lines);
        assert_eq!(lines.len(), 9);
        for line in &lines {
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(chars.len(), 22);
            assert_eq!(chars[7], ' ', "gap col should be space: {line:?}");
        }
    }

    #[test]
    fn test_h_gap_only() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_gap_pos(&GapPos::AfterRow(0));
        let lines = render(&cfg);
        print_grid("h_gap_only", &lines);
        assert_eq!(lines.len(), 10);
        let chars: Vec<char> = lines[3].chars().collect();
        assert_eq!(chars.len(), 21);
        assert!(chars.iter().all(|&c| c == ' '), "gap row should be spaces");
    }

    #[test]
    fn test_gap_grid() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_gap_pos(&GapPos::Grid);
        let lines = render(&cfg);
        print_grid("gap_grid", &lines);
        assert_eq!(lines.len(), 11);
        assert!(lines[3].chars().all(|c| c == ' '));
        assert!(lines[7].chars().all(|c| c == ' '));
        for (y, line) in lines.iter().enumerate() {
            if y == 3 || y == 7 { continue; }
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(chars[7],  ' ', "y={y} gap col 0");
            assert_eq!(chars[15], ' ', "y={y} gap col 1");
        }
    }

    #[test]
    fn test_v_gap_with_outer_border() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        cfg.apply_gap_pos(&GapPos::AfterCol(0));
        let lines = render(&cfg);
        print_grid("v_gap_with_outer_border", &lines);
        assert!(lines[0].starts_with('┌') && lines[0].ends_with('┐'));
        assert!(lines.last().unwrap().starts_with('└'));
        for line in &lines[1..lines.len()-1] {
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(chars[8], ' ', "gap col should be space: {line:?}");
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 4 — Spanned borders
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_h_spanned_border_partial() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 1 },
            &BORDER_SIMPLE,
        );
        assert_grid("h_spanned_border_partial", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╶────────────╴       ",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_v_spanned_border_partial() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterColSpanned { col: 0, row_start: 0, row_end: 1 },
            &BORDER_SIMPLE,
        );
        assert_grid("v_spanned_border_partial", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████╵▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_spanned_no_crossing() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 0 },
            &BORDER_SIMPLE,
        );
        cfg.apply_border_pos(
            &BorderPos::AfterColSpanned { col: 1, row_start: 1, row_end: 2 },
            &BORDER_SIMPLE,
        );
        assert_grid("spanned_no_crossing", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "╶─────╴               ",
            "███████▓▓▓▓▓▓▓╷░░░░░░░",
            "███████▓▓▓▓▓▓▓│░░░░░░░",
            "███████▓▓▓▓▓▓▓│░░░░░░░",
            "░░░░░░░███████│▓▓▓▓▓▓▓",
            "░░░░░░░███████│▓▓▓▓▓▓▓",
            "░░░░░░░███████╵▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_spanned_crossing() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 1 },
            &BORDER_SIMPLE,
        );
        cfg.apply_border_pos(
            &BorderPos::AfterColSpanned { col: 0, row_start: 0, row_end: 1 },
            &BORDER_SIMPLE,
        );
        assert_grid("spanned_crossing", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "╶──────┼──────╴       ",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████╵▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 5 — Different styles, no join
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_simple_v_double_h_no_join() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_DOUBLE_EXTENDED);
        assert_grid("simple_v_double_h_no_join", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "═══════│══════════════",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "███████│▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░│███████▓▓▓▓▓▓▓",
            "░░░░░░░╵███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 6 — Cell backgrounds
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_cell_backgrounds_no_border() {
        let cfg = make_3x3(7, 3);
        assert_grid("cell_backgrounds_no_border", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_cell_backgrounds_with_gaps_and_border() {
        let mut cfg = GridConfig::new(2, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        assert_grid("cell_backgrounds_with_gaps_and_border", &render(&cfg), &[
            "┌───────┬───────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "├───────┼───────┤",
            "│███████│▓▓▓▓▓▓▓│",
            "│███████│▓▓▓▓▓▓▓│",
            "│███████│▓▓▓▓▓▓▓│",
            "└───────┴───────┘",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 7 — Border text
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_border_text_h_start() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::AfterRow(0), TextAnchor::Start, 0, " Hello ");
        assert_grid("border_text_h_start", &render(&cfg), &[
            "┌─────────────────────┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "├ Hello ──────────────┤",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_border_text_h_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::AfterRow(0), TextAnchor::End, 0, " World ");
        assert_grid("border_text_h_end", &render(&cfg), &[
            "┌─────────────────────┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "├────────────── World ┤",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_border_text_h_start_with_offset() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::AfterRow(0), TextAnchor::Start, 2, "Hi");
        assert_grid("border_text_h_start_offset", &render(&cfg), &[
            "┌─────────────────────┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "├──Hi─────────────────┤",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_border_text_outer_top_start() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::Grid, TextAnchor::Start, 1, " Title ");
        assert_grid("border_text_outer_top_start", &render(&cfg), &[
            "┌─ Title ─────────────┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_border_text_outer_top_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::Grid, TextAnchor::End, 1, " v1.0 ");
        assert_grid("border_text_outer_top_end", &render(&cfg), &[
            "┌────────────── v1.0 ─┐",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│▓▓▓▓▓▓▓░░░░░░░███████│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│███████▓▓▓▓▓▓▓░░░░░░░│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "│░░░░░░░███████▓▓▓▓▓▓▓│",
            "└─────────────────────┘",
        ]);
    }

    #[test]
    fn test_border_text_truncation() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::Grid,        &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.set_border_text(
            &BorderPos::AfterRow(0), TextAnchor::Start, 0,
            "This text is way too long to fit in the border",
        );
        let lines = render(&cfg);
        print_grid("border_text_truncation", &lines);
        let row4: Vec<char> = lines[4].chars().collect();
        assert_eq!(row4[0],  '├');
        assert_eq!(row4[22], '┤');
        assert_eq!(row4[21], '…', "last text char should be ellipsis");
    }

    #[test]
    fn test_border_text_v_start() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::AfterCol(0), TextAnchor::Start, 0, "ABC");
        let lines = render(&cfg);
        print_grid("border_text_v_start", &lines);
        let chars_at_gap: Vec<char> = lines.iter()
            .map(|l| l.chars().nth(7).unwrap_or(' '))
            .collect();
        assert_eq!(chars_at_gap[0], 'A');
        assert_eq!(chars_at_gap[1], 'B');
        assert_eq!(chars_at_gap[2], 'C');
    }

    #[test]
    fn test_border_text_v_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.set_border_text(&BorderPos::AfterCol(0), TextAnchor::End, 0, "XY");
        let lines = render(&cfg);
        print_grid("border_text_v_end", &lines);
        let chars_at_gap: Vec<char> = lines.iter()
            .map(|l| l.chars().nth(7).unwrap_or(' '))
            .collect();
        let h = lines.len();
        assert_eq!(chars_at_gap[h - 2], 'X');
        assert_eq!(chars_at_gap[h - 1], 'Y');
    }

    #[test]
    fn test_border_text_gap_no_border_chars() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_gap_pos(&GapPos::AfterRow(0));
        cfg.set_border_text(&BorderPos::AfterRow(0), TextAnchor::Start, 0, "gap");
        let lines = render(&cfg);
        print_grid("border_text_gap_no_border_chars", &lines);
        let chars: Vec<char> = lines[3].chars().collect();
        assert_eq!(chars[0], 'g');
        assert_eq!(chars[1], 'a');
        assert_eq!(chars[2], 'p');
        assert_eq!(chars[3], ' ');
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 8 — CellGroup backgrounds only
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_group_col_span_no_gaps() {
        let mut cfg = GridConfig::new(2, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("group_col_span_no_gaps", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
        ]);
    }

    #[test]
    fn test_group_row_no_gaps() {
        let mut cfg = GridConfig::new(2, 3);
        cfg.col_constraints = vec![Constraint::Length(7); 3];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.group_cells(CellGroup::Row(0));
        assert_grid("group_row_no_gaps", &render(&cfg), &[
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
        ]);
    }

    #[test]
    fn test_group_col_no_gaps() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::Col(1));
        assert_grid("group_col_no_gaps", &render(&cfg), &[
            "▓▓▓▓▓▓▓╳╳╳╳╳╳╳███████",
            "▓▓▓▓▓▓▓╳╳╳╳╳╳╳███████",
            "▓▓▓▓▓▓▓╳╳╳╳╳╳╳███████",
            "███████╳╳╳╳╳╳╳░░░░░░░",
            "███████╳╳╳╳╳╳╳░░░░░░░",
            "███████╳╳╳╳╳╳╳░░░░░░░",
            "░░░░░░░╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "░░░░░░░╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "░░░░░░░╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_group_row_span_no_gaps() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::RowSpan { col: 0, first_row: 1, last_row: 2 });
        assert_grid("group_row_span_no_gaps", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_group_span_no_gaps() {
        let mut cfg = make_3x5(7, 3);
        cfg.group_cells(CellGroup::Span {
            first_row: 1, first_col: 1, last_row: 2, last_col: 3,
        });
        assert_grid("group_span_no_gaps", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░",
            "███████╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "███████╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "███████╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "░░░░░░░╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳███████",
            "░░░░░░░╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳███████",
            "░░░░░░░╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳███████",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 9 — CellGroup with gaps (no borders)
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_group_col_span_with_v_gap() {
        let mut cfg = GridConfig::new(2, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.apply_gap_pos(&GapPos::AfterCol(0));
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("group_col_span_with_v_gap", &render(&cfg), &[
            "▓▓▓▓▓▓▓ ░░░░░░░",
            "▓▓▓▓▓▓▓ ░░░░░░░",
            "▓▓▓▓▓▓▓ ░░░░░░░",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
        ]);
    }

    #[test]
    fn test_group_row_span_with_h_gap() {
        let mut cfg = GridConfig::new(3, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 3];
        cfg.apply_gap_pos(&GapPos::AfterRow(1));
        cfg.group_cells(CellGroup::RowSpan { col: 0, first_row: 1, last_row: 2 });
        assert_grid("group_row_span_with_h_gap", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░",
            "▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳       ",
            "╳╳╳╳╳╳╳███████",
            "╳╳╳╳╳╳╳███████",
            "╳╳╳╳╳╳╳███████",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 10 — CellGroup with borders (suppression inside group)
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_group_col_span_v_border_suppressed() {
        let mut cfg = GridConfig::new(2, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("group_col_span_v_border_suppressed", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░",
            "▓▓▓▓▓▓▓│░░░░░░░",
            "▓▓▓▓▓▓▓╵░░░░░░░",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
        ]);
    }

    #[test]
    fn test_group_col1_all_borders() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterCol(1), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(1), &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::Col(1));
        assert_grid("group_col1_all_borders", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷╳╳╳╳╳╳╳╷███████",
            "▓▓▓▓▓▓▓│╳╳╳╳╳╳╳│███████",
            "▓▓▓▓▓▓▓│╳╳╳╳╳╳╳│███████",
            "╶──────┤╳╳╳╳╳╳╳├──────╴",
            "███████│╳╳╳╳╳╳╳│░░░░░░░",
            "███████│╳╳╳╳╳╳╳│░░░░░░░",
            "███████│╳╳╳╳╳╳╳│░░░░░░░",
            "╶──────┤╳╳╳╳╳╳╳├──────╴",
            "░░░░░░░│╳╳╳╳╳╳╳│▓▓▓▓▓▓▓",
            "░░░░░░░│╳╳╳╳╳╳╳│▓▓▓▓▓▓▓",
            "░░░░░░░╵╳╳╳╳╳╳╳╵▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_group_row_span_h_border_suppressed() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(1), &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::RowSpan { col: 0, first_row: 1, last_row: 2 });
        assert_grid("group_row_span_h_border_suppressed", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╶───────────────────╴",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳▓▓▓▓▓▓▓░░░░░░░",
            "╳╳╳╳╳╳╳╶────────────╴",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
            "╳╳╳╳╳╳╳███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_group_span_all_borders() {
        let mut cfg = make_3x5(7, 3);
        for i in 0..4 { cfg.apply_border_pos(&BorderPos::AfterCol(i), &BORDER_SIMPLE); }
        for i in 0..2 { cfg.apply_border_pos(&BorderPos::AfterRow(i), &BORDER_SIMPLE); }
        cfg.group_cells(CellGroup::Span {
            first_row: 1, first_col: 1, last_row: 2, last_col: 3,
        });
        assert_grid("group_span_all_borders", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░╷███████╷▓▓▓▓▓▓▓╷░░░░░░░",
            "▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░",
            "▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░",
            "╶──────┼───────┴───────┴───────┼──────╴",
            "███████│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│▓▓▓▓▓▓▓",
            "███████│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│▓▓▓▓▓▓▓",
            "███████│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│▓▓▓▓▓▓▓",
            "╶──────┤╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳├──────╴",
            "░░░░░░░│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│███████",
            "░░░░░░░│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│███████",
            "░░░░░░░╵╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╵███████",
        ]);
    }

    #[test]
    fn test_group_col_span_with_outer_and_h_border() {
        let mut cfg = GridConfig::new(2, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(3); 2];
        cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("group_col_span_outer_h_border", &render(&cfg), &[
            "┌───────┬───────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "├───────┴───────┤",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "└───────────────┘",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 11 — group_cells overlap rules
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_group_larger_replaces_smaller() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        cfg.group_cells(CellGroup::Span { first_row: 1, first_col: 0, last_row: 1, last_col: 2 });
        assert_eq!(cfg.groups.len(), 1);
        let lines = render(&cfg);
        assert_grid("group_larger_replaces_smaller", &lines, &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_group_smaller_ignored_when_contained() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::Row(1));
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_eq!(cfg.groups.len(), 1);
        let lines = render(&cfg);
        for y in 3..6 {
            assert!(lines[y].chars().all(|c| c == '╳'),
                "row {y} should be all ╳: {:?}", lines[y]);
        }
    }

    #[test]
    #[should_panic]
    fn test_group_partial_overlap_panics() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        cfg.group_cells(CellGroup::Span { first_row: 1, first_col: 1, last_row: 2, last_col: 2 });
    }

    #[test]
    fn test_ungroup_cells() {
        let mut cfg = make_3x3(7, 3);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_eq!(cfg.groups.len(), 1);
        cfg.ungroup_cells(1, 0);
        assert_eq!(cfg.groups.len(), 0);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 12 — Border text on spanned positions
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_border_text_on_spanned_h_border() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 1, col_end: 2 },
            &BORDER_SIMPLE,
        );
        cfg.set_border_text(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 1, col_end: 2 },
            TextAnchor::Start, 0, "AB",
        );
        assert_grid("border_text_spanned_h_start", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "       AB───────────╴",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 13 — Multiple non-adjacent groups
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_two_non_adjacent_groups() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterCol(1), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(0), &BORDER_SIMPLE);
        cfg.apply_border_pos(&BorderPos::AfterRow(1), &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::Col(0));
        cfg.group_cells(CellGroup::Col(2));
        assert_grid("two_non_adjacent_groups", &render(&cfg), &[
            "╳╳╳╳╳╳╳╷░░░░░░░╷╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│░░░░░░░│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│░░░░░░░│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳├───────┤╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│▓▓▓▓▓▓▓│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│▓▓▓▓▓▓▓│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│▓▓▓▓▓▓▓│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳├───────┤╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│███████│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳│███████│╳╳╳╳╳╳╳",
            "╳╳╳╳╳╳╳╵███████╵╳╳╳╳╳╳╳",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 14 — CellGroup + spanned border overlap
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_group_with_v_spanned_border_overlap() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterColSpanned { col: 0, row_start: 0, row_end: 1 },
            &BORDER_SIMPLE,
        );
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("group_with_v_spanned_border_overlap", &render(&cfg), &[
            "▓▓▓▓▓▓▓╷░░░░░░░███████",
            "▓▓▓▓▓▓▓│░░░░░░░███████",
            "▓▓▓▓▓▓▓╵░░░░░░░███████",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳░░░░░░░",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳░░░░░░░",
            "╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳░░░░░░░",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 15 — Border text: gap-only and implicit gap creation
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_border_text_v_gap_only_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_gap_pos(&GapPos::AfterCol(1));
        cfg.set_border_text(&BorderPos::AfterCol(1), TextAnchor::End, 0, "XY");
        assert_grid("border_text_v_gap_only_end", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "▓▓▓▓▓▓▓░░░░░░░ ███████",
            "███████▓▓▓▓▓▓▓ ░░░░░░░",
            "███████▓▓▓▓▓▓▓ ░░░░░░░",
            "███████▓▓▓▓▓▓▓ ░░░░░░░",
            "░░░░░░░███████ ▓▓▓▓▓▓▓",
            "░░░░░░░███████X▓▓▓▓▓▓▓",
            "░░░░░░░███████Y▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_border_text_h_creates_gap_implicitly() {
        let mut cfg = make_3x3(7, 3);
        cfg.set_border_text(&BorderPos::AfterRow(1), TextAnchor::Start, 0, "hi");
        assert_grid("border_text_creates_gap_implicitly", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "hi                   ",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_border_text_v_creates_gap_implicitly() {
        let mut cfg = make_3x3(7, 3);
        cfg.set_border_text(&BorderPos::AfterCol(0), TextAnchor::End, 0, "Z");
        assert_grid("border_text_v_creates_gap_implicitly", &render(&cfg), &[
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░Z███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 17 — Spanned border text: gap-only (no prior border)
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_border_text_h_spanned_gap_only() {
        let mut cfg = make_3x3(7, 3);
        cfg.set_border_text(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 1, col_end: 2 },
            TextAnchor::Start, 0, "AB",
        );
        assert!(cfg.h_gaps[0].is_some(), "h-gap after row 0 must have been created");
        assert_grid("border_text_h_spanned_gap_only", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "       AB            ",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_border_text_v_spanned_gap_only() {
        let mut cfg = make_3x3(7, 3);
        cfg.set_border_text(
            &BorderPos::AfterColSpanned { col: 0, row_start: 1, row_end: 2 },
            TextAnchor::End, 0, "XY",
        );
        assert!(cfg.v_gaps[0].is_some(), "v-gap after col 0 must have been created");
        assert_grid("border_text_v_spanned_gap_only", &render(&cfg), &[
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "▓▓▓▓▓▓▓ ░░░░░░░███████",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "███████ ▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░ ███████▓▓▓▓▓▓▓",
            "░░░░░░░X███████▓▓▓▓▓▓▓",
            "░░░░░░░Y███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_border_text_h_spanned_with_border_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.apply_border_pos(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 1 },
            &BORDER_SIMPLE,
        );
        cfg.set_border_text(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 1 },
            TextAnchor::End, 0, "Hi",
        );
        assert_grid("border_text_h_spanned_with_border_end", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "╶───────────Hi       ",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    #[test]
    fn test_border_text_h_spanned_without_border_end() {
        let mut cfg = make_3x3(7, 3);
        cfg.set_border_text(
            &BorderPos::AfterRowSpanned { row: 0, col_start: 0, col_end: 1 },
            TextAnchor::End, 0, "Hi",
        );
        assert_grid("border_text_h_spanned_without_border_end", &render(&cfg), &[
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "▓▓▓▓▓▓▓░░░░░░░███████",
            "            Hi       ",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "███████▓▓▓▓▓▓▓░░░░░░░",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
            "░░░░░░░███████▓▓▓▓▓▓▓",
        ]);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Group 18 — ColSpan with outer border and all inner borders (form layout)
    // ═════════════════════════════════════════════════════════════════════════

    /// Reproduces the "New Team Member" form layout: 3×2 grid with a ColSpan
    /// on the middle row, outer border, and full inner borders.  The crossing
    /// characters at h_gap × v_gap must be correct (┴ above the group, ┬ below).
    #[test]
    fn test_col_span_middle_row_all_borders_and_outer() {
        let mut cfg = GridConfig::new(3, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(2); 3];
        cfg.set_outer_border(&BORDER_SIMPLE);
        cfg.set_v_border(0, &BORDER_SIMPLE);
        cfg.set_h_border(0, &BORDER_SIMPLE);
        cfg.set_h_border(1, &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });
        assert_grid("col_span_middle_row_all_borders_and_outer", &render(&cfg), &[
            "┌───────┬───────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "├───────┴───────┤",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "├───────┬───────┤",
            "│░░░░░░░│███████│",
            "│░░░░░░░│███████│",
            "└───────┴───────┘",
        ]);
    }

    /// When row constraints don't fill the available height, dead pixels appear
    /// between the last content row and the outer border.  The inner vertical
    /// border must extend through these dead pixels.
    #[test]
    fn test_v_border_extends_to_outer_with_dead_pixels() {
        let mut cfg = GridConfig::new(3, 2);
        cfg.col_constraints = vec![Constraint::Length(7); 2];
        cfg.row_constraints = vec![Constraint::Length(2); 3];
        cfg.set_outer_border(&BORDER_SIMPLE);
        cfg.set_v_border(0, &BORDER_SIMPLE);
        cfg.set_h_border(0, &BORDER_SIMPLE);
        cfg.set_h_border(1, &BORDER_SIMPLE);
        cfg.group_cells(CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 });

        // Request 1 extra row of height beyond what the constraints need.
        let w = cfg.total_width_hint();
        let h = cfg.total_height_hint() + 1; // 10 + 1 = 11
        let layout = compute_layout(&cfg, Rect::new(0, 0, w, h));
        let lines = render_with_cells(&cfg, &layout);
        assert_grid("v_border_dead_pixels", &lines, &[
            "┌───────┬───────┐",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "│▓▓▓▓▓▓▓│░░░░░░░│",
            "├───────┴───────┤",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "│╳╳╳╳╳╳╳╳╳╳╳╳╳╳╳│",
            "├───────┬───────┤",
            "│░░░░░░░│███████│",
            "│░░░░░░░│███████│",
            "│       │       │",
            "└───────┴───────┘",
        ]);
    }
}
