//! Demonstrates the various configurations of [`TwoColumnLayout`].
//! Press `q` or `Esc` to quit, `j`/`k` or arrow keys to scroll.

use std::{io, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Frame, Terminal,
};

use not_yet_done_ratatui::widgets::two_column::{
    BorderStyleType, ColumnWidth, HalfBorders, HalfPadding, TwoColumnLayout, TwoColumnStyle,
};

// ── demo registry ────────────────────────────────────────────────────────────

struct Demo {
    label: &'static str,
    height: u16,
    render: fn(Rect, &mut Buffer),
}

const DEMOS: &[Demo] = &[
    Demo {
        label:  " 1  Plain borders, no collapse — panels adjacent ",
        height: 5,
        render: demo_plain,
    },
    Demo {
        label:  " 2  Collapsed center, cyan borders, bold yellow headers ",
        height: 5,
        render: demo_collapsed_styled,
    },
    Demo {
        label:  " 3  Top + bottom only, no side borders, collapsed (flat) ",
        height: 5,
        render: demo_flat,
    },
    Demo {
        label:  " 4  Thick borders, collapsed, dark content backgrounds ",
        height: 5,
        render: demo_thick,
    },
    Demo {
        label:  " 5  Right border of left panel disabled ",
        height: 5,
        render: demo_no_inner_left,
    },
    Demo {
        label:  " 6  Left border of right panel disabled ",
        height: 5,
        render: demo_no_inner_right,
    },
    Demo {
        label:  " 7  Top border disabled on both panels ",
        height: 5,
        render: demo_no_top,
    },
    Demo {
        label:  " 8  Padding inside full borders (symmetric) ",
        height: 7,
        render: demo_padding_with_border,
    },
    Demo {
        label:  " 9  Padding without border — left only ",
        height: 5,
        render: demo_padding_no_border,
    },
    Demo {
        label: " 10  Border with background colour on border style ",
        height: 5,
        render: demo_border_background,
    },
    Demo {
        label: " 11  No border, different background per side, padding ",
        height: 5,
        render: demo_no_border_bg,
    },
    Demo {
        label: " 12  Center divider only (no outer borders) ",
        height: 5,
        render: demo_center_only,
    },
    Demo {
        label: " 13  Asymmetric padding: left side per-edge, right side uniform ",
        height: 7,
        render: demo_asymmetric_padding,
    },
    Demo {
        label: " 14  Double borders, collapsed, padding 1 all sides ",
        height: 7,
        render: demo_double,
    },
    Demo {
        label: " 15  Rounded borders, no collapse, padding right 2 ",
        height: 5,
        render: demo_rounded,
    },
    Demo {
        label:  " 16  Colored padding independent from content ",
        height: 9,
        render: demo_colored_padding,
    },
    Demo {
        label:  " 17  Fixed left column width (20 columns) ",
        height: 5,
        render: demo_fixed_width,
    },
    Demo {
        label:  " 18  Percentage split: 30 % left / 70 % right ",
        height: 5,
        render: demo_percent_width,
    },
];

// ── app state & main ─────────────────────────────────────────────────────────

struct App {
    scroll: u16,
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App { scroll: 0 };

    loop {
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
                    KeyCode::Up   | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

// ── frame rendering ───────────────────────────────────────────────────────────

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.size();

    // title bar
    let [title_area, body_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  TwoColumnLayout demos",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   ↑/k  ↓/j  scroll   q  quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        title_area,
    );

    // scrollable demo area
    let total_height: u16 = DEMOS.iter().map(|d| d.height + 2).sum(); // +2 for label row + gap
    let scroll = app.scroll.min(total_height.saturating_sub(body_area.height));

    // Render into a virtual buffer that is `total_height` rows tall, then blit
    // the visible window into the real frame buffer.
    let virtual_area = Rect { x: 0, y: 0, width: body_area.width, height: total_height };
    let mut vbuf = Buffer::empty(virtual_area);

    let mut y: u16 = 0;
    for demo in DEMOS {
        // label
        let label_rect = Rect { x: 0, y, width: body_area.width, height: 1 };
        Paragraph::new(Line::from(
            Span::styled(demo.label, Style::default().fg(Color::DarkGray)),
        ))
        .render(label_rect, &mut vbuf);
        y += 1;

        // demo content
        let demo_rect = Rect { x: 2, y, width: body_area.width.saturating_sub(2), height: demo.height };
        (demo.render)(demo_rect, &mut vbuf);
        y += demo.height + 1; // +1 blank row between demos
    }

    // Blit the visible slice of the virtual buffer into the real frame buffer.
    let visible = Rect { x: 0, y: scroll, width: body_area.width, height: body_area.height };
    let dst_buf = frame.buffer_mut();
    for row in 0..body_area.height {
        let src_y = visible.y + row;
        let dst_y = body_area.y + row;
        if src_y >= total_height { break; }
        for col in 0..body_area.width {
            dst_buf[(col, dst_y)] = vbuf[(col, src_y)].clone();
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn fill(rect: Rect, buf: &mut Buffer, left: &str, right: &str) {
    let mid = rect.width / 2;
    let l = Rect { x: rect.x, y: rect.y, width: mid, height: rect.height };
    let r = Rect { x: rect.x + mid, y: rect.y, width: rect.width - mid, height: rect.height };
    Paragraph::new(left).render(l, buf);
    Paragraph::new(right).render(r, buf);
}

// ── demos ─────────────────────────────────────────────────────────────────────

/// 1 — Plain borders, no collapse.
fn demo_plain(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Left Panel")
        .header_right("Right Panel")
        .render_layout(area, buf);
    fill(area, buf, "", "");
    Paragraph::new("content A\nline 2").render(l, buf);
    Paragraph::new("content B\nline 2").render(r, buf);
}

/// 2 — Collapsed center, styled headers and borders.
fn demo_collapsed_styled(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Tasks")
        .header_right("Details")
        .collapse(true)
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Cyan))
                .headers(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        )
        .render_layout(area, buf);
    Paragraph::new("task list").render(l, buf);
    Paragraph::new("task details").render(r, buf);
}

/// 3 — Top + bottom borders only, no side borders, collapsed.
fn demo_flat(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Column A")
        .header_right("Column B")
        .borders(HalfBorders::none().horizontal(true))
        .padding(HalfPadding::new().left(1))
        .collapse(true)
        .render_layout(area, buf);
    Paragraph::new("flat left\nline 2").render(l, buf);
    Paragraph::new("flat right\nline 2").render(r, buf);
}

/// 4 — Thick borders, collapsed, dark content backgrounds.
fn demo_thick(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Bold Left")
        .header_right("Bold Right")
        .border_type(BorderStyleType::Thick)
        .collapse(true)
        .padding(HalfPadding::new().left(1).right(1))
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Magenta))
                .content(Style::default().bg(Color::DarkGray)),
        )
        .render_layout(area, buf);
    Paragraph::new("thick borders").render(l, buf);
    Paragraph::new("thick borders").render(r, buf);
}

/// 5 — Right border of the left panel disabled (open inner edge).
fn demo_no_inner_left(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Open Right →")
        .header_right("Normal")
        .left_borders(HalfBorders::all().right(false))
        .render_layout(area, buf);
    Paragraph::new("no right\nborder here").render(l, buf);
    Paragraph::new("normal").render(r, buf);
}

/// 6 — Left border of the right panel disabled (open inner edge).
fn demo_no_inner_right(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Normal")
        .header_right("← Open Left")
        .right_borders(HalfBorders::all().left(false))
        .render_layout(area, buf);
    Paragraph::new("normal").render(l, buf);
    Paragraph::new("no left\nborder here").render(r, buf);
}

/// 7 — Top border disabled on both panels.
fn demo_no_top(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("No top L")   // header is ignored when top border is off
        .header_right("No top R")
        .borders(HalfBorders::all().top(false))
        .render_layout(area, buf);
    Paragraph::new("no top\nborder").render(l, buf);
    Paragraph::new("no top\nborder").render(r, buf);
}

/// 8 — Symmetric padding (2) inside full borders.
fn demo_padding_with_border(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Padded Left")
        .header_right("Padded Right")
        .padding(HalfPadding::all(2))
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Green)),
        )
        .render_layout(area, buf);
    Paragraph::new("inner text").render(l, buf);
    Paragraph::new("inner text").render(r, buf);
}

/// 9 — Padding without any border, left panel only.
fn demo_padding_no_border(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .left_borders(HalfBorders::none())
        .right_borders(HalfBorders::none())
        .left_padding(HalfPadding::new().left(3).top(1))
        .render_layout(area, buf);
    Paragraph::new("indented left").render(l, buf);
    Paragraph::new("no padding\nright side").render(r, buf);
}

/// 10 — Border characters with a coloured background.
fn demo_border_background(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("BG Border L")
        .header_right("BG Border R")
        .collapse(true)
        .style(
            TwoColumnStyle::new()
                .border(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Blue),
                )
                .headers(
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .render_layout(area, buf);
    Paragraph::new("bg on borders").render(l, buf);
    Paragraph::new("bg on borders").render(r, buf);
}

/// 11 — No borders at all, different background colours per side, with padding.
fn demo_no_border_bg(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .borders(HalfBorders::none())
        .padding(HalfPadding::new().left(2).right(2).top(1))
        .style(
            TwoColumnStyle::new()
                .content_left(Style::default().bg(Color::Rgb(40, 20, 60)))
                .content_right(Style::default().bg(Color::Rgb(20, 50, 40))),
        )
        .render_layout(area, buf);
    Paragraph::new("purple bg").render(l, buf);
    Paragraph::new("teal bg").render(r, buf);
}

/// 12 — Only the center divider, no outer borders on either side.
fn demo_center_only(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Left")
        .header_right("Right")
        // disable outer edges; keep inner edges so the collapse junction renders
        .left_borders(HalfBorders::none().right(true))
        .right_borders(HalfBorders::none().left(true))
        .collapse(true)
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Yellow)),
        )
        .render_layout(area, buf);
    Paragraph::new("only divider\nbetween us").render(l, buf);
    Paragraph::new("only divider\nbetween us").render(r, buf);
}

/// 13 — Asymmetric padding: each side individually configured.
fn demo_asymmetric_padding(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Asymmetric L")
        .header_right("Asymmetric R")
        // Left: heavy left indent, no top padding, some bottom padding
        .left_padding(
            HalfPadding::new()
                .left(3)
                .right(1)
                .top(0)
                .bottom(2),
        )
        // Right: uniform horizontal padding, extra top
        .right_padding(
            HalfPadding::new()
                .left(1)
                .right(1)
                .top(2)
                .bottom(0),
        )
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Cyan))
                .content_left(Style::default().bg(Color::Rgb(30, 30, 50)))
                .content_right(Style::default().bg(Color::Rgb(30, 50, 30))),
        )
        .render_layout(area, buf);
    Paragraph::new("left=3\nbottom=2").render(l, buf);
    Paragraph::new("top=2\nlr=1").render(r, buf);
}

/// 14 — Double borders, collapsed, uniform padding 1.
fn demo_double(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Double L")
        .header_right("Double R")
        .border_type(BorderStyleType::Double)
        .collapse(true)
        .padding(HalfPadding::all(1))
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::LightBlue)),
        )
        .render_layout(area, buf);
    Paragraph::new("double\nborders").render(l, buf);
    Paragraph::new("double\nborders").render(r, buf);
}

/// 15 — Rounded borders, no collapse, right-padding 2 on each panel.
fn demo_rounded(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Rounded L")
        .header_right("Rounded R")
        .border_type(BorderStyleType::Rounded)
        .padding(HalfPadding::new().right(2))
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::LightMagenta))
                .headers(Style::default().fg(Color::White)),
        )
        .render_layout(area, buf);
    Paragraph::new("rounded\ncorners").render(l, buf);
    Paragraph::new("rounded\ncorners").render(r, buf);
}

/// 16 — Padding and content colored independently.
///
/// Left panel:  all-sides padding 2, padding = dark orange, content = near-black.
/// Right panel: bottom+right padding 2 only, padding = dark teal, content = near-black.
/// Neither background matches the padding color.
fn demo_colored_padding(area: Rect, buf: &mut Buffer) {
    let pad_left_color  = Color::Rgb(80, 35, 10);   // dark orange
    let pad_right_color = Color::Rgb(15, 55, 55);   // dark teal
    let content_color   = Color::Rgb(18, 18, 32);   // near-black (different from both)

    let (l, r) = TwoColumnLayout::new()
        .borders(HalfBorders::none())
        // Left: uniform padding on all four sides.
        .left_padding(HalfPadding::all(2))
        // Right: padding only on bottom and right.
        .right_padding(HalfPadding::new().bottom(2).right(3))
        .style(
            TwoColumnStyle::new()
                .padding_style_left(Style::default().bg(pad_left_color))
                .padding_style_right(Style::default().bg(pad_right_color))
                .content(Style::default().bg(content_color)),
        )
        .render_layout(area, buf);

    Paragraph::new("all sides\npadded\norange band").render(l, buf);
    Paragraph::new("bottom+right\npadded\nteal band").render(r, buf);
}

/// 17 — Left column fixed to 20 terminal columns.
fn demo_fixed_width(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("Narrow (20)")
        .header_right("Wide (rest)")
        .left_width(ColumnWidth::Fixed(20))
        .collapse(true)
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::Cyan))
                .headers(Style::default().fg(Color::Yellow)),
        )
        .render_layout(area, buf);
    Paragraph::new("fixed 20 cols").render(l, buf);
    Paragraph::new("gets everything else").render(r, buf);
}

/// 18 — Left column takes 30 % of the available width.
fn demo_percent_width(area: Rect, buf: &mut Buffer) {
    let (l, r) = TwoColumnLayout::new()
        .header_left("30 %")
        .header_right("70 %")
        .left_width(ColumnWidth::Percent(30))
        .collapse(true)
        .style(
            TwoColumnStyle::new()
                .border(Style::default().fg(Color::LightMagenta))
                .headers(Style::default().fg(Color::White)),
        )
        .render_layout(area, buf);
    Paragraph::new("30 %").render(l, buf);
    Paragraph::new("70 %").render(r, buf);
}
