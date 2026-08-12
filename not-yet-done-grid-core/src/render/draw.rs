use super::target::RenderTarget;
use crate::layout::GridLayout;
use crate::types::{BorderChars, BorderText, GridConfig, SpannedBorder, TextAnchor};

// ── Public entry points ───────────────────────────────────────────────────────

/// Draw only the gap/border skeleton (outer frame, lines, crossings, texts).
/// Does **not** fill cell backgrounds.
pub fn draw_borders<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    draw_outer_frame(cfg, layout, target);
    draw_horizontal_lines(cfg, layout, target);
    draw_vertical_lines(cfg, layout, target);
    draw_crossings(cfg, layout, target);
    draw_border_texts(cfg, layout, target);
}

// ── Outer frame ───────────────────────────────────────────────────────────────

fn draw_outer_frame<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    let Some(bc) = cfg.outer_border else { return };
    let w = layout.total_width as usize;
    let h = layout.total_height as usize;

    // x/y offsets of the frame (always 0,0 relative to the area origin, which
    // is baked into col_x/row_y by compute_layout).
    let x0 = layout.col_x[0].saturating_sub(1);
    let y0 = layout.row_y[0].saturating_sub(1);
    let x1 = x0 + w as u16 - 1;
    let y1 = y0 + h as u16 - 1;

    for x in x0..=x1 {
        target.put_char(x, y0, bc.horizontal);
        target.put_char(x, y1, bc.horizontal);
    }
    for y in y0..=y1 {
        target.put_char(x0, y, bc.vertical);
        target.put_char(x1, y, bc.vertical);
    }
    target.put_char(x0, y0, bc.top_left);
    target.put_char(x1, y0, bc.top_right);
    target.put_char(x0, y1, bc.bottom_left);
    target.put_char(x1, y1, bc.bottom_right);
}

// ── Horizontal lines ──────────────────────────────────────────────────────────

fn draw_horizontal_lines<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    // Full-width horizontal borders.
    for (gap_idx, slot) in cfg.h_gaps.iter().enumerate() {
        let Some(slot) = slot else { continue };
        let Some(bc) = slot.border else { continue };
        let Some(&y) = layout.h_gap_y.get(gap_idx).and_then(|v| v.as_ref()) else {
            continue;
        };

        let outer_same = cfg
            .outer_border
            .map(|ob| same_style(ob, bc))
            .unwrap_or(false);
        let active_xs = h_active_xs(cfg, layout, gap_idx, 0, cfg.cols - 1);

        for &x in &active_xs {
            target.put_char(x, y, bc.horizontal);
        }

        if outer_same {
            // Fill dead pixels between content columns and the outer border.
            let x0 = layout.col_x[0].saturating_sub(1);
            let x1 = x0 + layout.total_width - 1;
            // Left of the first column.
            if !cfg.is_inside_v_group(gap_idx, 0) {
                for x in (x0 + 1)..layout.col_x[0] {
                    target.put_char(x, y, bc.horizontal);
                }
            }
            // Right of the last column.
            let last = cfg.cols - 1;
            if !cfg.is_inside_v_group(gap_idx, last) {
                let col_end = layout.col_x[last] + layout.col_w[last];
                for x in col_end..(x1) {
                    target.put_char(x, y, bc.horizontal);
                }
            }
        } else if let (Some(&fx), Some(&lx)) = (active_xs.first(), active_xs.last()) {
            target.put_char(fx, y, bc.half_left);
            target.put_char(lx, y, bc.half_right);
        }
    }

    // Spanned horizontal borders.
    for span in &cfg.h_spanned {
        draw_h_spanned(cfg, span, layout, target);
    }
}

fn draw_h_spanned<T: RenderTarget>(
    cfg: &GridConfig,
    span: &SpannedBorder,
    layout: &GridLayout,
    target: &mut T,
) {
    let Some(&y) = layout.h_gap_y.get(span.gap_index).and_then(|v| v.as_ref()) else {
        return;
    };
    let active_xs = h_active_xs(cfg, layout, span.gap_index, span.start, span.end);

    if let Some(bc) = span.border {
        for &x in &active_xs {
            target.put_char(x, y, bc.horizontal);
        }
        if let (Some(&fx), Some(&lx)) = (active_xs.first(), active_xs.last()) {
            target.put_char(fx, y, bc.half_left);
            target.put_char(lx, y, bc.half_right);
        }
    }
}

/// Collect all x-positions that should receive a horizontal line in `h_gap_idx`
/// for the column range `col_start..=col_end`, skipping group-suppressed positions.
fn h_active_xs(
    cfg: &GridConfig,
    layout: &GridLayout,
    h_gap_idx: usize,
    col_start: usize,
    col_end: usize,
) -> Vec<u16> {
    let mut xs: Vec<u16> = Vec::new();

    for col in col_start..=col_end {
        if cfg.is_inside_v_group(h_gap_idx, col) {
            continue;
        }
        let x0 = layout.col_x[col];
        let x1 = x0 + layout.col_w[col];
        for x in x0..x1 {
            xs.push(x);
        }
    }
    // v-gap columns within the range.
    for gap_col in col_start..col_end {
        let Some(&gx) = layout.v_gap_x.get(gap_col).and_then(|v| v.as_ref()) else {
            continue;
        };
        if !cfg.is_inside_v_group(h_gap_idx, gap_col)
            || !cfg.is_inside_v_group(h_gap_idx, gap_col + 1)
        {
            xs.push(gx);
        }
    }
    xs.sort_unstable();
    xs.dedup();
    xs
}

// ── Vertical lines ────────────────────────────────────────────────────────────

fn draw_vertical_lines<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    // Full-height vertical borders.
    for (gap_idx, slot) in cfg.v_gaps.iter().enumerate() {
        let Some(slot) = slot else { continue };
        let Some(bc) = slot.border else { continue };
        let Some(&x) = layout.v_gap_x.get(gap_idx).and_then(|v| v.as_ref()) else {
            continue;
        };

        let outer_same = cfg
            .outer_border
            .map(|ob| same_style(ob, bc))
            .unwrap_or(false);
        let active_ys = v_active_ys(cfg, layout, gap_idx, 0, cfg.rows - 1);

        for &y in &active_ys {
            target.put_char(x, y, bc.vertical);
        }

        if outer_same {
            // Fill dead pixels between content rows and the outer border.
            // These appear when row constraints don't fill the inner height.
            let y0 = layout.row_y[0].saturating_sub(1);
            let y1 = y0 + layout.total_height - 1;
            // Above the first row.
            if !cfg.is_inside_h_group(0, gap_idx) {
                for y in (y0 + 1)..layout.row_y[0] {
                    target.put_char(x, y, bc.vertical);
                }
            }
            // Below the last row.
            let last = cfg.rows - 1;
            if !cfg.is_inside_h_group(last, gap_idx) {
                let row_end = layout.row_y[last] + layout.row_h[last];
                for y in row_end..(y1) {
                    target.put_char(x, y, bc.vertical);
                }
            }
        } else if let (Some(&fy), Some(&ly)) = (active_ys.first(), active_ys.last()) {
            target.put_char(x, fy, bc.half_top);
            target.put_char(x, ly, bc.half_bottom);
        }
    }

    // Spanned vertical borders.
    for span in &cfg.v_spanned {
        draw_v_spanned(cfg, span, layout, target);
    }
}

fn draw_v_spanned<T: RenderTarget>(
    cfg: &GridConfig,
    span: &SpannedBorder,
    layout: &GridLayout,
    target: &mut T,
) {
    let Some(&x) = layout.v_gap_x.get(span.gap_index).and_then(|v| v.as_ref()) else {
        return;
    };
    let active_ys = v_active_ys(cfg, layout, span.gap_index, span.start, span.end);

    if let Some(bc) = span.border {
        for &y in &active_ys {
            target.put_char(x, y, bc.vertical);
        }
        if let (Some(&fy), Some(&ly)) = (active_ys.first(), active_ys.last()) {
            target.put_char(x, fy, bc.half_top);
            target.put_char(x, ly, bc.half_bottom);
        }
    }
}

/// Collect all y-positions that should receive a vertical line in `v_gap_idx`
/// for the row range `row_start..=row_end`, skipping group-suppressed positions.
fn v_active_ys(
    cfg: &GridConfig,
    layout: &GridLayout,
    v_gap_idx: usize,
    row_start: usize,
    row_end: usize,
) -> Vec<u16> {
    let mut ys: Vec<u16> = Vec::new();

    for row in row_start..=row_end {
        if cfg.is_inside_h_group(row, v_gap_idx) {
            continue;
        }
        let y0 = layout.row_y[row];
        let y1 = y0 + layout.row_h[row];
        for y in y0..y1 {
            ys.push(y);
        }
    }
    // h-gap rows within the range.
    for gap_row in row_start..row_end {
        let Some(&gy) = layout.h_gap_y.get(gap_row).and_then(|v| v.as_ref()) else {
            continue;
        };
        if !cfg.is_inside_h_group(gap_row, v_gap_idx)
            || !cfg.is_inside_h_group(gap_row + 1, v_gap_idx)
        {
            ys.push(gy);
        }
    }
    ys.sort_unstable();
    ys.dedup();
    ys
}

// ── Crossings and corners ─────────────────────────────────────────────────────

fn draw_crossings<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    // Full × Full crossings.
    for (vi, vx) in layout.v_gap_x.iter().enumerate() {
        let Some(&x) = vx.as_ref() else { continue };
        for (hi, hy) in layout.h_gap_y.iter().enumerate() {
            let Some(&y) = hy.as_ref() else { continue };
            let h_bc = full_h_border(cfg, hi);
            let v_bc = full_v_border(cfg, vi);
            let h_left = h_bc.is_some() && !cfg.is_inside_v_group(hi, vi);
            let h_right = h_bc.is_some() && vi + 1 < cfg.cols && !cfg.is_inside_v_group(hi, vi + 1);
            let v_above = v_bc.is_some() && !cfg.is_inside_h_group(hi, vi);
            let v_below = v_bc.is_some() && hi + 1 < cfg.rows && !cfg.is_inside_h_group(hi + 1, vi);
            apply_crossing(x, y, h_bc, v_bc, h_left, h_right, v_above, v_below, target);
        }
    }

    // Full V × Spanned H crossings.
    for span in &cfg.h_spanned {
        let Some(&y) = layout.h_gap_y.get(span.gap_index).and_then(|v| v.as_ref()) else {
            continue;
        };
        let x_start = layout.col_x[span.start];
        let x_end = layout.col_x[span.end] + layout.col_w[span.end] - 1;
        for (vi, vx) in layout.v_gap_x.iter().enumerate() {
            let Some(&x) = vx.as_ref() else { continue };
            if x < x_start || x > x_end {
                continue;
            }
            let v_bc = full_v_border(cfg, vi);
            let h_left = !cfg.is_inside_v_group(span.gap_index, vi);
            let h_right = vi + 1 < cfg.cols && !cfg.is_inside_v_group(span.gap_index, vi + 1);
            let v_above = v_bc.is_some() && !cfg.is_inside_h_group(span.gap_index, vi);
            let v_below = v_bc.is_some()
                && span.gap_index + 1 < cfg.rows
                && !cfg.is_inside_h_group(span.gap_index + 1, vi);
            apply_crossing(
                x,
                y,
                span.border,
                v_bc,
                h_left,
                h_right,
                v_above,
                v_below,
                target,
            );
        }
    }

    // Spanned V × Full H crossings.
    for span in &cfg.v_spanned {
        let Some(&x) = layout.v_gap_x.get(span.gap_index).and_then(|v| v.as_ref()) else {
            continue;
        };
        let y_start = layout.row_y[span.start];
        let y_end = layout.row_y[span.end] + layout.row_h[span.end] - 1;
        for (hi, hy) in layout.h_gap_y.iter().enumerate() {
            let Some(&y) = hy.as_ref() else { continue };
            if y < y_start || y > y_end {
                continue;
            }
            let h_bc = full_h_border(cfg, hi);
            let h_left = h_bc.is_some() && !cfg.is_inside_v_group(hi, span.gap_index);
            let h_right = h_bc.is_some()
                && span.gap_index + 1 < cfg.cols
                && !cfg.is_inside_v_group(hi, span.gap_index + 1);
            let v_above = !cfg.is_inside_h_group(hi, span.gap_index);
            let v_below = hi + 1 < cfg.rows && !cfg.is_inside_h_group(hi + 1, span.gap_index);
            apply_crossing(
                x,
                y,
                h_bc,
                span.border,
                h_left,
                h_right,
                v_above,
                v_below,
                target,
            );
        }
    }

    // Spanned V × Spanned H crossings.
    for h_span in &cfg.h_spanned {
        let Some(&y) = layout
            .h_gap_y
            .get(h_span.gap_index)
            .and_then(|v| v.as_ref())
        else {
            continue;
        };
        let hx_start = layout.col_x[h_span.start];
        let hx_end = layout.col_x[h_span.end] + layout.col_w[h_span.end] - 1;
        for v_span in &cfg.v_spanned {
            let Some(&x) = layout
                .v_gap_x
                .get(v_span.gap_index)
                .and_then(|v| v.as_ref())
            else {
                continue;
            };
            if x < hx_start || x > hx_end {
                continue;
            }
            let vy_start = layout.row_y[v_span.start];
            let vy_end = layout.row_y[v_span.end] + layout.row_h[v_span.end] - 1;
            if y < vy_start || y > vy_end {
                continue;
            }
            let vi = v_span.gap_index;
            let hi = h_span.gap_index;
            let h_left = !cfg.is_inside_v_group(hi, vi);
            let h_right = vi + 1 < cfg.cols && !cfg.is_inside_v_group(hi, vi + 1);
            let v_above = !cfg.is_inside_h_group(hi, vi);
            let v_below = hi + 1 < cfg.rows && !cfg.is_inside_h_group(hi + 1, vi);
            apply_crossing(
                x,
                y,
                h_span.border,
                v_span.border,
                h_left,
                h_right,
                v_above,
                v_below,
                target,
            );
        }
    }

    // Outer frame: corners and T-pieces with inner borders of the same style.
    if let Some(outer_bc) = cfg.outer_border {
        let x0 = layout.col_x[0].saturating_sub(1);
        let y0 = layout.row_y[0].saturating_sub(1);
        let x1 = x0 + layout.total_width - 1;
        let y1 = y0 + layout.total_height - 1;

        for (vi, vx) in layout.v_gap_x.iter().enumerate() {
            let Some(&x) = vx.as_ref() else { continue };
            if let Some(v_bc) = full_v_border(cfg, vi) {
                if same_style(outer_bc, v_bc) {
                    if !cfg.is_inside_h_group(0, vi) {
                        target.put_char(x, y0, outer_bc.t_top);
                    }
                    if !cfg.is_inside_h_group(cfg.rows - 1, vi) {
                        target.put_char(x, y1, outer_bc.t_bottom);
                    }
                }
            }
        }
        for (hi, hy) in layout.h_gap_y.iter().enumerate() {
            let Some(&y) = hy.as_ref() else { continue };
            if let Some(h_bc) = full_h_border(cfg, hi) {
                if same_style(outer_bc, h_bc) {
                    if !cfg.is_inside_v_group(hi, 0) {
                        target.put_char(x0, y, outer_bc.t_left);
                    }
                    if !cfg.is_inside_v_group(hi, cfg.cols - 1) {
                        target.put_char(x1, y, outer_bc.t_right);
                    }
                }
            }
        }

        // Corners always last (they overwrite T-pieces at coincident positions).
        target.put_char(x0, y0, outer_bc.top_left);
        target.put_char(x1, y0, outer_bc.top_right);
        target.put_char(x0, y1, outer_bc.bottom_left);
        target.put_char(x1, y1, outer_bc.bottom_right);
    }
}

// ── Crossing character selection ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn apply_crossing<T: RenderTarget>(
    x: u16,
    y: u16,
    h_bc: Option<&'static BorderChars>,
    v_bc: Option<&'static BorderChars>,
    left: bool,
    right: bool,
    above: bool,
    below: bool,
    target: &mut T,
) {
    let (Some(h), Some(v)) = (h_bc, v_bc) else {
        return;
    };
    if !same_style(h, v) {
        return;
    }

    let ch = match (left, right, above, below) {
        (true, true, true, true) => h.cross,
        (true, true, true, false) => h.t_bottom,
        (true, true, false, true) => h.t_top,
        (false, true, true, true) => h.t_left,
        (true, false, true, true) => h.t_right,
        (true, false, true, false) => h.bottom_right,
        (true, false, false, true) => h.top_right,
        (false, true, true, false) => h.bottom_left,
        (false, true, false, true) => h.top_left,
        _ => return,
    };
    target.put_char(x, y, ch);
}

// ── Border text ───────────────────────────────────────────────────────────────

fn draw_border_texts<T: RenderTarget>(cfg: &GridConfig, layout: &GridLayout, target: &mut T) {
    // Outer border title on the top edge.
    if let Some(text) = &cfg.outer_border_text {
        if cfg.outer_border.is_some() {
            let x0 = layout.col_x[0].saturating_sub(1);
            let y = layout.row_y[0].saturating_sub(1);
            let x_min = x0 + 1;
            let x_max = x0 + layout.total_width - 2;
            write_h_text(target, y, x_min, x_max, text);
        }
    }

    // Full horizontal gap texts.
    for (gap_idx, slot) in cfg.h_gaps.iter().enumerate() {
        let Some(slot) = slot else { continue };
        let Some(text) = &slot.text else { continue };
        let Some(&y) = layout.h_gap_y.get(gap_idx).and_then(|v| v.as_ref()) else {
            continue;
        };
        let x_min = layout.content_x;
        let x_max = x_min + content_width(layout) as u16 - 1;
        write_h_text(target, y, x_min, x_max, text);
    }

    // Full vertical gap texts.
    for (gap_idx, slot) in cfg.v_gaps.iter().enumerate() {
        let Some(slot) = slot else { continue };
        let Some(text) = &slot.text else { continue };
        let Some(&x) = layout.v_gap_x.get(gap_idx).and_then(|v| v.as_ref()) else {
            continue;
        };
        let y_min = layout.content_y;
        let y_max = y_min + content_height(layout) as u16 - 1;
        write_v_text(target, x, y_min, y_max, text);
    }

    // Spanned horizontal gap texts.
    for span in &cfg.h_spanned {
        let Some(text) = &span.text else { continue };
        let Some(&y) = layout.h_gap_y.get(span.gap_index).and_then(|v| v.as_ref()) else {
            continue;
        };
        let x_min = layout.col_x[span.start];
        let x_max = layout.col_x[span.end] + layout.col_w[span.end] - 1;
        write_h_text(target, y, x_min, x_max, text);
    }

    // Spanned vertical gap texts.
    for span in &cfg.v_spanned {
        let Some(text) = &span.text else { continue };
        let Some(&x) = layout.v_gap_x.get(span.gap_index).and_then(|v| v.as_ref()) else {
            continue;
        };
        let y_min = layout.row_y[span.start];
        let y_max = layout.row_y[span.end] + layout.row_h[span.end] - 1;
        write_v_text(target, x, y_min, y_max, text);
    }
}

// ── Text helpers ──────────────────────────────────────────────────────────────

fn write_h_text<T: RenderTarget>(
    target: &mut T,
    y: u16,
    x_min: u16,
    x_max: u16,
    text: &BorderText,
) {
    if x_max < x_min {
        return;
    }
    let chars: Vec<char> = text.text.chars().collect();
    let capacity = (x_max - x_min + 1) as usize;

    let x_anchor = match text.anchor {
        TextAnchor::Start => x_min + text.offset as u16,
        TextAnchor::End => {
            let end = x_max.saturating_sub(text.offset as u16);
            end.saturating_sub(chars.len().saturating_sub(1) as u16)
        }
    };
    if x_anchor > x_max {
        return;
    }

    let space = (x_max - x_anchor + 1) as usize;
    let (to_write, truncate) = if chars.len() <= space {
        (&chars[..], false)
    } else {
        (&chars[..space.saturating_sub(1)], true)
    };

    for (i, &ch) in to_write.iter().enumerate() {
        let x = x_anchor + i as u16;
        if x > x_max {
            break;
        }
        target.put_char(x, y, ch);
    }
    if truncate {
        let x_ell = x_anchor + space as u16 - 1;
        if x_ell <= x_max {
            target.put_char(x_ell, y, '…');
        }
    }
    let _ = capacity; // suppress unused warning
}

fn write_v_text<T: RenderTarget>(
    target: &mut T,
    x: u16,
    y_min: u16,
    y_max: u16,
    text: &BorderText,
) {
    if y_max < y_min {
        return;
    }
    let chars: Vec<char> = text.text.chars().collect();

    let y_anchor = match text.anchor {
        TextAnchor::Start => y_min + text.offset as u16,
        TextAnchor::End => {
            let end = y_max.saturating_sub(text.offset as u16);
            end.saturating_sub(chars.len().saturating_sub(1) as u16)
        }
    };
    if y_anchor > y_max {
        return;
    }

    let space = (y_max - y_anchor + 1) as usize;
    let (to_write, truncate) = if chars.len() <= space {
        (&chars[..], false)
    } else {
        (&chars[..space.saturating_sub(1)], true)
    };

    for (i, &ch) in to_write.iter().enumerate() {
        let y = y_anchor + i as u16;
        if y > y_max {
            break;
        }
        target.put_char(x, y, ch);
    }
    if truncate {
        let y_ell = y_anchor + space as u16 - 1;
        if y_ell <= y_max {
            target.put_char(x, y_ell, '…');
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn full_h_border(cfg: &GridConfig, hi: usize) -> Option<&'static BorderChars> {
    cfg.h_gaps.get(hi)?.as_ref()?.border
}

fn full_v_border(cfg: &GridConfig, vi: usize) -> Option<&'static BorderChars> {
    cfg.v_gaps.get(vi)?.as_ref()?.border
}

pub(crate) fn same_style(a: &BorderChars, b: &BorderChars) -> bool {
    std::ptr::eq(a, b) || (a.horizontal == b.horizontal && a.vertical == b.vertical)
}

fn content_width(layout: &GridLayout) -> usize {
    if layout.col_x.is_empty() {
        return 0;
    }
    let last = layout.col_x.len() - 1;
    (layout.col_x[last] + layout.col_w[last] - layout.col_x[0]) as usize
}

fn content_height(layout: &GridLayout) -> usize {
    if layout.row_y.is_empty() {
        return 0;
    }
    let last = layout.row_y.len() - 1;
    (layout.row_y[last] + layout.row_h[last] - layout.row_y[0]) as usize
}
