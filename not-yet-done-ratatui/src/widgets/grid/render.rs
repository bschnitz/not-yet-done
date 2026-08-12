use ratatui::{Frame, buffer::Buffer, layout::Rect, style::Style};

use not_yet_done_grid_core::layout::GridLayout as CoreGridLayout;
use not_yet_done_grid_core::{
    BorderText as CoreBorderText, GapSlot, GridConfig, RenderTarget,
    SpannedBorder as CoreSpannedBorder, TextAnchor as CoreTextAnchor, draw_borders,
};

use super::{
    ColGapConfig, GapText, Grid, RowGapConfig,
    border::{BorderChars, TextAnchor},
    layout::{GridLayout, RowBand, compute_layout},
};

// ---------------------------------------------------------------------------
// RatatuiBuf — RenderTarget adapter for ratatui Buffer
// ---------------------------------------------------------------------------

/// Wraps a ratatui `Buffer` and implements [`RenderTarget`] so that the
/// backend-independent drawing code from `not-yet-done-grid-core` can write
/// directly into the terminal buffer.
struct RatatuiBuf<'a> {
    buf: &'a mut Buffer,
}

impl<'a> RenderTarget for RatatuiBuf<'a> {
    fn put_char(&mut self, x: u16, y: u16, ch: char) {
        if let Some(cell) = self.buf.cell_mut((x, y)) {
            cell.set_char(ch);
        }
    }

    fn get_char(&self, x: u16, y: u16) -> char {
        self.buf
            .cell((x, y))
            .map(|c| {
                let s = c.symbol();
                s.chars().next().unwrap_or(' ')
            })
            .unwrap_or(' ')
    }
}

// ---------------------------------------------------------------------------
// Conversion: Grid widget state → GridConfig (core)
// ---------------------------------------------------------------------------

/// Build a `GridConfig` from the `Grid` widget state so the core drawing
/// pipeline can be used without duplicating any logic.
fn grid_to_config(grid: &Grid) -> GridConfig {
    let mut cfg = GridConfig::new(grid.rows, grid.cols);
    cfg.col_constraints = grid.col_constraints.clone();
    cfg.row_constraints = grid.row_constraints.clone();

    // Outer border.
    if grid.outer.enabled {
        if let Some(chars) = grid.outer.chars {
            cfg.outer_border = Some(chars);
        }
    }
    if let Some(gt) = &grid.outer.text {
        cfg.outer_border_text = Some(gap_text_to_core(gt));
    }

    // Vertical gaps and their full / spanned borders.
    for (i, v_gap) in grid.v_gaps.iter().enumerate() {
        if !v_gap.has_gap {
            continue;
        }
        cfg.v_gaps[i] = Some(GapSlot {
            border: v_gap.full.as_ref().map(|b| b.chars),
            text: v_gap
                .full
                .as_ref()
                .and_then(|b| b.text.as_ref())
                .map(gap_text_to_core),
        });
        for span in &v_gap.spans {
            cfg.v_spanned.push(CoreSpannedBorder {
                gap_index: i,
                start: span.start,
                end: span.end,
                border: Some(span.chars),
                text: span.text.as_ref().map(gap_text_to_core),
            });
        }
    }

    // Horizontal gaps and their full / spanned borders.
    for (i, h_gap) in grid.h_gaps.iter().enumerate() {
        if !h_gap.has_gap {
            continue;
        }
        cfg.h_gaps[i] = Some(GapSlot {
            border: h_gap.full.as_ref().map(|b| b.chars),
            text: h_gap
                .full
                .as_ref()
                .and_then(|b| b.text.as_ref())
                .map(gap_text_to_core),
        });
        for span in &h_gap.spans {
            cfg.h_spanned.push(CoreSpannedBorder {
                gap_index: i,
                start: span.start,
                end: span.end,
                border: Some(span.chars),
                text: span.text.as_ref().map(gap_text_to_core),
            });
        }
    }

    // Cell groups.
    for group_def in &grid.groups {
        use not_yet_done_grid_core::CellGroup;
        // Convert GroupDef back to the canonical CellGroup::Span form.
        cfg.groups.push(CellGroup::Span {
            first_row: group_def.first_row,
            first_col: group_def.first_col,
            last_row: group_def.last_row,
            last_col: group_def.last_col,
        });
    }

    cfg
}

fn gap_text_to_core(gt: &GapText) -> CoreBorderText {
    CoreBorderText {
        anchor: match gt.anchor {
            TextAnchor::Start => CoreTextAnchor::Start,
            TextAnchor::End => CoreTextAnchor::End,
        },
        offset: gt.offset,
        text: gt.text.clone(),
    }
}

// ---------------------------------------------------------------------------
// Conversion: local GridLayout → core GridLayout
// ---------------------------------------------------------------------------

/// Repackage the locally computed `GridLayout` into the type expected by the
/// core drawing functions.  All pixel coordinates are identical; this is a
/// purely structural conversion.
fn local_to_core_layout(layout: &GridLayout, area: Rect) -> CoreGridLayout {
    CoreGridLayout {
        col_x: layout.col_rects.iter().map(|c| c.x).collect(),
        col_w: layout.col_rects.iter().map(|c| c.width).collect(),
        row_y: layout.row_rects.iter().map(|r| r.y).collect(),
        row_h: layout.row_rects.iter().map(|r| r.height).collect(),
        v_gap_x: layout.v_gap_x.clone(),
        h_gap_y: layout.h_gap_y.clone(),
        content_x: area.x + if layout.has_outer { 1 } else { 0 },
        content_y: area.y + if layout.has_outer { 1 } else { 0 },
        total_width: area.width,
        total_height: area.height,
    }
}

// ---------------------------------------------------------------------------
// Entry point — new rendering pipeline
// ---------------------------------------------------------------------------

/// Renders the complete grid into `frame` following the seven-step pipeline.
pub(super) fn render(frame: &mut Frame, area: Rect, grid: &mut Grid) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let layout = compute_layout(grid, area);

    {
        let buf = frame.buffer_mut();

        // Step 3: fill the entire grid area with the global style.
        apply_global_style(buf, area, grid);

        // Step 4: apply per-gap and outer-border styles.
        apply_gap_styles(buf, area, &layout, grid);

        // Step 5: draw border characters using the core pipeline.
        render_borders(buf, area, &layout, grid);

        // Step 6: write gap text overlays.
        render_gap_texts(buf, area, &layout, grid);
    }

    // Step 7: fill each cell's background, then render the child widget.
    render_cells(frame, &layout, grid);
}

// ---------------------------------------------------------------------------
// Step 3 — Global style
// ---------------------------------------------------------------------------

fn apply_global_style(buf: &mut Buffer, area: Rect, grid: &Grid) {
    let style = grid.global_style;
    for dy in 0..area.height {
        for dx in 0..area.width {
            if let Some(cell) = buf.cell_mut((area.x + dx, area.y + dy)) {
                cell.set_char(' ');
                cell.set_style(style);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Step 4 — Gap styles
// ---------------------------------------------------------------------------

fn apply_gap_styles(buf: &mut Buffer, area: Rect, layout: &GridLayout, grid: &Grid) {
    // Outer border frame.
    if layout.has_outer {
        if let Some(style) = grid.outer.style {
            let x0 = area.x;
            let y0 = area.y;
            let x1 = area.x + area.width.saturating_sub(1);
            let y1 = area.y + area.height.saturating_sub(1);
            for x in x0..=x1 {
                set_style(buf, x, y0, style);
                set_style(buf, x, y1, style);
            }
            for y in (y0 + 1)..y1 {
                set_style(buf, x0, y, style);
                set_style(buf, x1, y, style);
            }
        }
    }

    // Vertical gap columns.
    for (gi, gap) in grid.v_gaps.iter().enumerate() {
        let Some(gx) = layout.v_gap_x.get(gi).copied().flatten() else {
            continue;
        };
        if let Some(style) = gap.full.as_ref().and_then(|b| b.style) {
            for row in 0..grid.rows {
                if is_inside_h_group(grid, row, gi) {
                    continue;
                }
                let rb = layout.row_rects[row];
                for dy in 0..rb.height {
                    set_style(buf, gx, rb.y + dy, style);
                }
            }
            // Dead pixels between content rows and the outer border.
            if layout.has_outer {
                let y0 = area.y;
                let y1 = area.y + area.height.saturating_sub(1);
                if !is_inside_h_group(grid, 0, gi) {
                    for y in (y0 + 1)..layout.row_rects[0].y {
                        set_style(buf, gx, y, style);
                    }
                }
                let last = grid.rows - 1;
                if !is_inside_h_group(grid, last, gi) {
                    let row_end = layout.row_rects[last].y + layout.row_rects[last].height;
                    for y in row_end..y1 {
                        set_style(buf, gx, y, style);
                    }
                }
            }
        }
        for span in &gap.spans {
            if let Some(style) = span.style {
                for row in span.start..=span.end {
                    if is_inside_h_group(grid, row, gi) {
                        continue;
                    }
                    let rb = layout.row_rects[row];
                    for dy in 0..rb.height {
                        set_style(buf, gx, rb.y + dy, style);
                    }
                }
            }
        }
    }

    // Horizontal gap rows.
    for (gi, gap) in grid.h_gaps.iter().enumerate() {
        let Some(gy) = layout.h_gap_y.get(gi).copied().flatten() else {
            continue;
        };
        if let Some(style) = gap.full.as_ref().and_then(|b| b.style) {
            for col in 0..grid.cols {
                if is_inside_v_group(grid, gi, col) {
                    continue;
                }
                let cb = layout.col_rects[col];
                for dx in 0..cb.width {
                    set_style(buf, cb.x + dx, gy, style);
                }
            }
            // Dead pixels between content columns and the outer border.
            if layout.has_outer {
                let x0 = area.x;
                let x1 = area.x + area.width.saturating_sub(1);
                if !is_inside_v_group(grid, gi, 0) {
                    for x in (x0 + 1)..layout.col_rects[0].x {
                        set_style(buf, x, gy, style);
                    }
                }
                let last = grid.cols - 1;
                if !is_inside_v_group(grid, gi, last) {
                    let col_end = layout.col_rects[last].x + layout.col_rects[last].width;
                    for x in col_end..x1 {
                        set_style(buf, x, gy, style);
                    }
                }
            }
        }
        for span in &gap.spans {
            if let Some(style) = span.style {
                for col in span.start..=span.end {
                    if is_inside_v_group(grid, gi, col) {
                        continue;
                    }
                    let cb = layout.col_rects[col];
                    for dx in 0..cb.width {
                        set_style(buf, cb.x + dx, gy, style);
                    }
                }
            }
        }
    }

    // Crossing points (v_gap × h_gap intersections).
    // The v-gap pass only covers content rows and the h-gap pass only covers
    // content columns, so the intersection cells are missed by both.  Without
    // this pass, crossing characters (┼ ┬ ┴ ├ ┤) keep the global_style and
    // appear in the wrong colour — or become invisible when the global
    // foreground matches the background.
    for (vi, v_gap) in grid.v_gaps.iter().enumerate() {
        let Some(gx) = layout.v_gap_x.get(vi).copied().flatten() else {
            continue;
        };
        for (hi, h_gap) in grid.h_gaps.iter().enumerate() {
            let Some(gy) = layout.h_gap_y.get(hi).copied().flatten() else {
                continue;
            };
            let style = h_gap
                .full
                .as_ref()
                .and_then(|b| b.style)
                .or_else(|| v_gap.full.as_ref().and_then(|b| b.style))
                .unwrap_or(grid.global_style);
            set_style(buf, gx, gy, style);
        }
    }
}

// ---------------------------------------------------------------------------
// Step 5 — Border characters (via not-yet-done-grid-core)
// ---------------------------------------------------------------------------

fn render_borders(buf: &mut Buffer, area: Rect, layout: &GridLayout, grid: &Grid) {
    let cfg = grid_to_config(grid);
    let core_layout = local_to_core_layout(layout, area);
    let mut target = RatatuiBuf { buf };
    draw_borders(&cfg, &core_layout, &mut target);
}

// ---------------------------------------------------------------------------
// Step 6 — Gap text overlays
// ---------------------------------------------------------------------------

fn render_gap_texts(buf: &mut Buffer, area: Rect, layout: &GridLayout, grid: &Grid) {
    let rows = grid.rows;
    let cols = grid.cols;

    // Outer border title.
    if layout.has_outer {
        if let Some(gt) = &grid.outer.text {
            let x0 = area.x;
            let y0 = area.y;
            let x1 = area.x + area.width.saturating_sub(1);
            let line_start = x0 + 1;
            let line_len = x1.saturating_sub(x0 + 1) as usize;
            let style = grid.outer.style.unwrap_or(grid.global_style);
            write_text_h(buf, line_start, y0, line_len, gt, style);
        }
    }

    // Vertical gap texts.
    for (gi, gap) in grid.v_gaps.iter().enumerate() {
        let Some(gx) = layout.v_gap_x.get(gi).copied().flatten() else {
            continue;
        };
        if let Some(full) = &gap.full {
            if let Some(gt) = &full.text {
                let top_rb = layout.row_rects[0];
                let bot_rb = layout.row_rects[rows - 1];
                let line_start_y = top_rb.y;
                let line_len = (bot_rb.y + bot_rb.height - top_rb.y) as usize;
                let style = full.style.unwrap_or(grid.global_style);
                write_text_v(buf, gx, line_start_y, line_len, gt, style);
            }
        }
        for span in &gap.spans {
            if let Some(gt) = &span.text {
                let start_rb = layout.row_rects[span.start];
                let end_rb = layout.row_rects[span.end];
                let line_start_y = start_rb.y;
                let line_len = (end_rb.y + end_rb.height - start_rb.y) as usize;
                let style = span.style.unwrap_or(grid.global_style);
                write_text_v(buf, gx, line_start_y, line_len, gt, style);
            }
        }
    }

    // Horizontal gap texts.
    for (gi, gap) in grid.h_gaps.iter().enumerate() {
        let Some(gy) = layout.h_gap_y.get(gi).copied().flatten() else {
            continue;
        };
        if let Some(full) = &gap.full {
            if let Some(gt) = &full.text {
                let left_cb = layout.col_rects[0];
                let right_cb = layout.col_rects[cols - 1];
                let line_start_x = left_cb.x;
                let line_len = (right_cb.x + right_cb.width - left_cb.x) as usize;
                let style = full.style.unwrap_or(grid.global_style);
                write_text_h(buf, line_start_x, gy, line_len, gt, style);
            }
        }
        for span in &gap.spans {
            if let Some(gt) = &span.text {
                let start_cb = layout.col_rects[span.start];
                let end_cb = layout.col_rects[span.end];
                let line_start_x = start_cb.x;
                let line_len = (end_cb.x + end_cb.width - start_cb.x) as usize;
                let style = span.style.unwrap_or(grid.global_style);
                write_text_h(buf, line_start_x, gy, line_len, gt, style);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Step 7 — Cell content
// ---------------------------------------------------------------------------

fn render_cells(frame: &mut Frame, layout: &GridLayout, grid: &mut Grid) {
    let focused_cell = if grid.focused {
        Some(grid.focus_cell)
    } else {
        None
    };

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            if Some((row, col)) == focused_cell {
                continue;
            }
            if !grid.is_group_origin(row, col) {
                continue;
            }
            render_cell(frame, row, col, grid, layout);
        }
    }
    if let Some((row, col)) = focused_cell {
        if grid.is_group_origin(row, col) {
            render_cell(frame, row, col, grid, layout);
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy render pipeline (v1) — kept for reference
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(super) fn render_v1(frame: &mut Frame, area: Rect, grid: &mut Grid) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let layout = compute_layout(grid, area);
    let buf = frame.buffer_mut();

    if layout.has_outer {
        render_outer_border(buf, area, grid, &layout);
    }

    for (gap_idx, gap) in grid.v_gaps.iter().enumerate() {
        let Some(gx) = layout.v_gap_x.get(gap_idx).copied().flatten() else {
            continue;
        };
        render_v_gap(buf, gx, &layout, gap_idx, gap, grid);
    }

    for (gap_idx, gap) in grid.h_gaps.iter().enumerate() {
        let Some(gy) = layout.h_gap_y.get(gap_idx).copied().flatten() else {
            continue;
        };
        render_h_gap(buf, gy, &layout, gap_idx, gap, grid);
    }

    render_corners(buf, &layout, grid);

    let focused_cell = if grid.focused {
        Some(grid.focus_cell)
    } else {
        None
    };
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            if Some((row, col)) == focused_cell {
                continue;
            }
            if !grid.is_group_origin(row, col) {
                continue;
            }
            render_cell(frame, row, col, grid, &layout);
        }
    }
    if let Some((row, col)) = focused_cell {
        if grid.is_group_origin(row, col) {
            render_cell(frame, row, col, grid, &layout);
        }
    }
}

fn render_outer_border(buf: &mut Buffer, area: Rect, grid: &Grid, _layout: &GridLayout) {
    let Some(chars) = grid.outer.chars else {
        return;
    };
    let style = grid.outer.style.unwrap_or(grid.global_style);
    let x0 = area.x;
    let y0 = area.y;
    let x1 = area.x + area.width.saturating_sub(1);
    let y1 = area.y + area.height.saturating_sub(1);

    set_char(buf, x0, y0, chars.top_left, style);
    for x in (x0 + 1)..x1 {
        set_char(buf, x, y0, chars.horizontal, style);
    }
    if x1 > x0 {
        set_char(buf, x1, y0, chars.top_right, style);
    }
    set_char(buf, x0, y1, chars.bottom_left, style);
    for x in (x0 + 1)..x1 {
        set_char(buf, x, y1, chars.horizontal, style);
    }
    if x1 > x0 {
        set_char(buf, x1, y1, chars.bottom_right, style);
    }
    for y in (y0 + 1)..y1 {
        set_char(buf, x0, y, chars.vertical, style);
        set_char(buf, x1, y, chars.vertical, style);
    }

    for (gi, gx_opt) in _layout.v_gap_x.iter().enumerate() {
        let Some(gx) = gx_opt else { continue };
        if grid.v_gaps[gi].full.is_some() {
            if !is_inside_h_group(grid, 0, gi) {
                set_char(buf, *gx, y0, chars.t_top, style);
            }
            if !is_inside_h_group(grid, grid.rows.saturating_sub(1), gi) {
                set_char(buf, *gx, y1, chars.t_bottom, style);
            }
        }
    }
    for (gi, gy_opt) in _layout.h_gap_y.iter().enumerate() {
        let Some(gy) = gy_opt else { continue };
        if grid.h_gaps[gi].full.is_some() {
            set_char(buf, x0, *gy, chars.t_left, style);
            set_char(buf, x1, *gy, chars.t_right, style);
        }
    }

    if let Some(gt) = &grid.outer.text {
        let line_start = x0 + 1;
        let line_len = (x1.saturating_sub(x0 + 1)) as usize;
        write_text_h(buf, line_start, y0, line_len, gt, style);
    }
}

use std::ptr;

fn render_v_gap(
    buf: &mut Buffer,
    gx: u16,
    layout: &GridLayout,
    gap_idx: usize,
    gap: &ColGapConfig,
    grid: &Grid,
) {
    let rows = grid.rows;
    let style = gap
        .full
        .as_ref()
        .and_then(|b| b.style)
        .unwrap_or(grid.global_style);

    for row in 0..rows {
        let rb = layout.row_rects[row];
        let border_chars = effective_v_border(gap, row);

        for dy in 0..rb.height {
            let y = rb.y + dy;
            if is_inside_h_group(grid, row, gap_idx) {
                set_char(buf, gx, y, ' ', Style::default());
                continue;
            }
            let ch = if let Some(bc) = border_chars {
                bc.vertical
            } else {
                ' '
            };
            let cell_style = border_chars
                .and_then(|_| effective_v_style(gap, row))
                .unwrap_or(style);
            set_char(buf, gx, y, ch, cell_style);
        }
        render_v_gap_ends(buf, gx, rb, gap, row, rows, grid, gap_idx);
    }

    if let Some(full) = &gap.full {
        if let Some(gt) = &full.text {
            let top_rb = layout.row_rects[0];
            let bot_rb = layout.row_rects[rows - 1];
            let line_start_y = top_rb.y;
            let line_len = (bot_rb.y + bot_rb.height - top_rb.y) as usize;
            write_text_v(
                buf,
                gx,
                line_start_y,
                line_len,
                gt,
                full.style.unwrap_or(grid.global_style),
            );
        }
    }
    for span in &gap.spans {
        if let Some(gt) = &span.text {
            let start_rb = layout.row_rects[span.start];
            let end_rb = layout.row_rects[span.end];
            let line_len = (end_rb.y + end_rb.height - start_rb.y) as usize;
            write_text_v(
                buf,
                gx,
                start_rb.y,
                line_len,
                gt,
                span.style.unwrap_or(grid.global_style),
            );
        }
    }
}

fn render_v_gap_ends(
    buf: &mut Buffer,
    gx: u16,
    rb: RowBand,
    gap: &ColGapConfig,
    row: usize,
    rows: usize,
    grid: &Grid,
    gap_idx: usize,
) {
    for span in &gap.spans {
        if is_inside_h_group(grid, row, gap_idx) {
            continue;
        }
        let bc = span.chars;
        let sp_style = span.style.unwrap_or(grid.global_style);
        let full_border = gap.full.as_ref().map(|b| b.chars);

        if row == span.start && span.start > 0 && full_border.map_or(true, |fc| !ptr::eq(fc, bc)) {
            set_char(buf, gx, rb.y, bc.half_top, sp_style);
        }
        if row == span.end && span.end + 1 < rows && full_border.map_or(true, |fc| !ptr::eq(fc, bc))
        {
            set_char(
                buf,
                gx,
                rb.y + rb.height.saturating_sub(1),
                bc.half_bottom,
                sp_style,
            );
        }
    }
}

fn render_h_gap(
    buf: &mut Buffer,
    gy: u16,
    layout: &GridLayout,
    gap_idx: usize,
    gap: &RowGapConfig,
    grid: &Grid,
) {
    let cols = grid.cols;
    let style = gap
        .full
        .as_ref()
        .and_then(|b| b.style)
        .unwrap_or(grid.global_style);

    for col in 0..cols {
        let cb = layout.col_rects[col];

        if is_inside_v_group(grid, gap_idx, col) {
            for dx in 0..cb.width {
                set_char(buf, cb.x + dx, gy, ' ', Style::default());
            }
            continue;
        }

        let border_chars = effective_h_border(gap, col);
        let ch = border_chars.map_or(' ', |bc| bc.horizontal);
        let col_style = border_chars
            .and_then(|_| effective_h_style(gap, col))
            .unwrap_or(style);
        for dx in 0..cb.width {
            set_char(buf, cb.x + dx, gy, ch, col_style);
        }

        for span in &gap.spans {
            if is_inside_v_group(grid, gap_idx, col) {
                continue;
            }
            let bc = span.chars;
            let sp_style = span.style.unwrap_or(grid.global_style);
            let full_border = gap.full.as_ref().map(|b| b.chars);

            if col == span.start
                && span.start > 0
                && full_border.map_or(true, |fc| !ptr::eq(fc, bc))
            {
                set_char(buf, cb.x, gy, bc.half_left, sp_style);
            }
            if col == span.end
                && span.end + 1 < cols
                && full_border.map_or(true, |fc| !ptr::eq(fc, bc))
            {
                set_char(
                    buf,
                    cb.x + cb.width.saturating_sub(1),
                    gy,
                    bc.half_right,
                    sp_style,
                );
            }
        }
    }

    if let Some(full) = &gap.full {
        if let Some(gt) = &full.text {
            let left_cb = layout.col_rects[0];
            let right_cb = layout.col_rects[cols - 1];
            let line_len = (right_cb.x + right_cb.width - left_cb.x) as usize;
            write_text_h(
                buf,
                left_cb.x,
                gy,
                line_len,
                gt,
                full.style.unwrap_or(grid.global_style),
            );
        }
    }
    for span in &gap.spans {
        if let Some(gt) = &span.text {
            let start_cb = layout.col_rects[span.start];
            let end_cb = layout.col_rects[span.end];
            let line_len = (end_cb.x + end_cb.width - start_cb.x) as usize;
            write_text_h(
                buf,
                start_cb.x,
                gy,
                line_len,
                gt,
                span.style.unwrap_or(grid.global_style),
            );
        }
    }
}

fn render_corners(buf: &mut Buffer, layout: &GridLayout, grid: &Grid) {
    for (vi, vgx_opt) in layout.v_gap_x.iter().enumerate() {
        let Some(vgx) = vgx_opt else { continue };
        for (hi, hgy_opt) in layout.h_gap_y.iter().enumerate() {
            let Some(hgy) = hgy_opt else { continue };

            let h_bc = grid.h_gaps[hi].full.as_ref().map(|b| b.chars);
            let v_bc = grid.v_gaps[vi].full.as_ref().map(|b| b.chars);

            let v_above = v_bc.is_some() && !is_inside_h_group(grid, hi, vi);
            let v_below = v_bc.is_some() && !is_inside_h_group(grid, hi + 1, vi);

            let ch = compute_corner_char(h_bc, v_bc, v_above, v_below);
            let st = grid.h_gaps[hi]
                .full
                .as_ref()
                .and_then(|b| b.style)
                .or_else(|| grid.v_gaps[vi].full.as_ref().and_then(|b| b.style))
                .unwrap_or(grid.global_style);
            set_char(buf, *vgx, *hgy, ch, st);
        }
    }
}

fn compute_corner_char(
    h_bc: Option<&'static BorderChars>,
    v_bc: Option<&'static BorderChars>,
    v_above: bool,
    v_below: bool,
) -> char {
    match (h_bc, v_bc) {
        (Some(hb), Some(vb)) if ptr::eq(hb, vb) => match (v_above, v_below) {
            (true, true) => hb.cross,
            (true, false) => hb.t_bottom,
            (false, true) => hb.t_top,
            (false, false) => hb.horizontal,
        },
        (Some(hb), _) => hb.horizontal,
        (None, Some(vb)) => vb.vertical,
        (None, None) => ' ',
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn render_cell(frame: &mut Frame, row: usize, col: usize, grid: &mut Grid, layout: &GridLayout) {
    let rect = layout.effective_rect(row, col, grid);
    let cell_style = grid
        .cell_styles
        .get(row * grid.cols + col)
        .copied()
        .flatten()
        .unwrap_or(grid.global_style);

    let buf = frame.buffer_mut();
    fill_rect(buf, rect, cell_style);

    let child_idx = row * grid.cols + col;
    if let Some(child) = grid.children.get_mut(child_idx).and_then(|c| c.as_mut()) {
        child.view(frame, rect);
    }
}

pub(super) fn is_inside_h_group(grid: &Grid, row: usize, gap_idx: usize) -> bool {
    let col_right = gap_idx + 1;
    grid.groups.iter().any(|g| {
        g.first_row <= row && row <= g.last_row && g.first_col <= gap_idx && col_right <= g.last_col
    })
}

pub(super) fn is_inside_v_group(grid: &Grid, gap_idx: usize, col: usize) -> bool {
    let row_below = gap_idx + 1;
    grid.groups.iter().any(|g| {
        g.first_col <= col && col <= g.last_col && g.first_row <= gap_idx && row_below <= g.last_row
    })
}

fn effective_v_border<'a>(gap: &'a ColGapConfig, row: usize) -> Option<&'a BorderChars> {
    for span in &gap.spans {
        if span.start <= row && row <= span.end {
            return Some(span.chars);
        }
    }
    gap.full.as_ref().map(|b| b.chars)
}

fn effective_v_style(gap: &ColGapConfig, row: usize) -> Option<Style> {
    for span in &gap.spans {
        if span.start <= row && row <= span.end {
            return span.style;
        }
    }
    gap.full.as_ref().and_then(|b| b.style)
}

fn effective_h_border<'a>(gap: &'a RowGapConfig, col: usize) -> Option<&'a BorderChars> {
    for span in &gap.spans {
        if span.start <= col && col <= span.end {
            return Some(span.chars);
        }
    }
    gap.full.as_ref().map(|b| b.chars)
}

fn effective_h_style(gap: &RowGapConfig, col: usize) -> Option<Style> {
    for span in &gap.spans {
        if span.start <= col && col <= span.end {
            return span.style;
        }
    }
    gap.full.as_ref().and_then(|b| b.style)
}

fn write_text_h(
    buf: &mut Buffer,
    line_x: u16,
    y: u16,
    line_len: usize,
    gt: &GapText,
    style: Style,
) {
    let text_len = gt.text.chars().count();
    let start_col = match gt.anchor {
        TextAnchor::Start => gt.offset,
        TextAnchor::End => {
            if line_len >= text_len + gt.offset {
                line_len - text_len - gt.offset
            } else {
                0
            }
        }
    };
    for (i, ch) in gt.text.chars().enumerate() {
        let col = start_col + i;
        if col >= line_len {
            break;
        }
        set_char(buf, line_x + col as u16, y, ch, style);
    }
}

fn write_text_v(
    buf: &mut Buffer,
    x: u16,
    line_y: u16,
    line_len: usize,
    gt: &GapText,
    style: Style,
) {
    let text_len = gt.text.chars().count();
    let start_row = match gt.anchor {
        TextAnchor::Start => gt.offset,
        TextAnchor::End => {
            if line_len >= text_len + gt.offset {
                line_len - text_len - gt.offset
            } else {
                0
            }
        }
    };
    for (i, ch) in gt.text.chars().enumerate() {
        let row = start_row + i;
        if row >= line_len {
            break;
        }
        let actual_ch =
            if i + 1 == text_len.min(line_len - start_row) && text_len > line_len - start_row {
                '…'
            } else {
                ch
            };
        set_char(buf, x, line_y + row as u16, actual_ch, style);
    }
}

fn set_char(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch);
        cell.set_style(style);
    }
}

fn set_style(buf: &mut Buffer, x: u16, y: u16, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_style(style);
    }
}

fn fill_rect(buf: &mut Buffer, rect: Rect, style: Style) {
    for dy in 0..rect.height {
        for dx in 0..rect.width {
            if let Some(cell) = buf.cell_mut((rect.x + dx, rect.y + dy)) {
                cell.set_char(' ');
                cell.set_style(style);
            }
        }
    }
}
