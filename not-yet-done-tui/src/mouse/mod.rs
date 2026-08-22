//! Mouse support: where things were drawn, and what a click on them means.
//!
//! # Why a hit map
//!
//! The UI is drawn imperatively — a widget receives a `Rect`, paints into it
//! and forgets it. A mouse event, though, arrives as a bare cell coordinate
//! and has to be turned back into "the sort menu" or "the left pane". The two
//! ways to bridge that are recomputing every layout when the event arrives,
//! or writing down what was painted while painting it. This module does the
//! second: the render pass records one `(Rect, Region)` entry per visible
//! surface, and [`hit`] walks them *backwards* so the topmost surface wins —
//! which is free, because the overlays are already drawn in Z-order.
//!
//! The map holds ~20 entries and is rebuilt every frame, so it can never go
//! stale against what is on screen.
//!
//! # Why the map is ambient
//!
//! [`push`] is a free function over frame-scoped state rather than a
//! parameter threaded through the render tree. A popup draws itself through
//! [`PanelChrome`](crate::ui::panel_chrome::PanelChrome), which is called from
//! inside `&mut app.some_popup` and so cannot also borrow the map off `App`;
//! handing every one of the seventeen panel sites an extra argument to work
//! around that would be plumbing with no meaning of its own. There is exactly
//! one terminal, one render pass and one map, which is the same reasoning
//! that already makes the hint-width cap and the graphics-protocol probe
//! process-wide.
//!
//! # Feature gate
//!
//! Everything here stays behind the `mouse` feature. With the feature off the
//! types remain (so the render pass and the event loop need no `cfg`
//! attributes at their call sites) but every entry point is an empty
//! `#[inline]` no-op, no escape sequence is emitted, and the terminal's own
//! selection is left exactly as it was.

#[cfg(feature = "mouse")]
mod selection;

use ratatui::layout::Rect;

use crate::views::content_view::PaneId;

#[cfg(feature = "mouse")]
use {
    crate::app::EditorRequest,
    crate::ui::theme::Theme,
    crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    ratatui::buffer::Buffer,
    ratatui::layout::Position,
    selection::{Selection, Snapshot},
    std::cell::RefCell,
    std::time::{Duration, Instant},
};

/// How many lines one wheel notch moves.
///
/// Without mouse reporting the terminal translates the wheel into arrow keys
/// on the alternate screen, so scrolling works today; turning reporting on
/// takes that away, and we have to serve the wheel ourselves or we would make
/// things worse than before the feature existed.
#[cfg(feature = "mouse")]
const WHEEL_LINES: usize = 3;

/// How close together two clicks on the same cell make a double click.
///
/// Terminals report presses and releases, never "double click", so the
/// pairing happens here. 400 ms is the interval most desktops default to.
#[cfg(feature = "mouse")]
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// A surface the render pass drew, in the terms a click cares about.
///
/// Window-local selection needs only the rectangle; the payloads are what
/// routing (switch to that tab, focus that pane) keys off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// One main-tab label, carrying the tab it activates. Pushed by the tab
    /// bar on top of [`Region::TabBar`], so a click between labels still
    /// lands on the bar and does nothing.
    Tab(crate::tabs::Tab),
    /// One sub-tab label of the active view, by index into its `view_defs`.
    SubTab(usize),
    /// The tab bar's background — everything the labels do not cover.
    TabBar,
    /// The active view's action bar.
    ActionBar,
    AlertBar,
    /// The inline query-error strip above the content.
    QueryError,
    /// One leaf of the active view's pane tree, inside its focus border.
    ContentPane(PaneId),
    /// The builtin editor pane (a row band, not an overlay).
    Editor,
    NotificationBar,
    StatusBar,
    /// A floating popup panel.
    Popup,
}

#[cfg(feature = "mouse")]
thread_local! {
    /// What the current frame has drawn so far, in paint order.
    static REGIONS: RefCell<Vec<(Rect, Region)>> = const { RefCell::new(Vec::new()) };
}

/// Start a new frame. The map describes one frame and one frame only.
#[cfg(feature = "mouse")]
pub fn begin_frame() {
    REGIONS.with_borrow_mut(Vec::clear);
}

#[cfg(not(feature = "mouse"))]
#[inline]
pub fn begin_frame() {}

/// Record a surface. Called from the render pass right where the surface is
/// drawn, so paint order and hit order cannot drift apart.
#[cfg(feature = "mouse")]
pub fn push(rect: Rect, region: Region) {
    if rect.width > 0 && rect.height > 0 {
        REGIONS.with_borrow_mut(|r| r.push((rect, region)));
    }
}

#[cfg(not(feature = "mouse"))]
#[inline]
pub fn push(_rect: Rect, _region: Region) {}

/// The topmost surface covering the cell, if any.
#[cfg(feature = "mouse")]
pub(crate) fn hit(x: u16, y: u16) -> Option<(Rect, Region)> {
    REGIONS.with_borrow(|regions| {
        regions
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(Position::new(x, y)))
            .copied()
    })
}

/// Mouse state carried across frames: the drag selection in progress, or the
/// last finished one, which stays highlighted until something invalidates it.
#[derive(Default)]
pub struct MouseState {
    #[cfg(feature = "mouse")]
    selection: Option<Selection>,
    /// The cells under the selection's region, copied out of the frame buffer
    /// each time it is drawn. Reading them back on button-release is what
    /// makes "copy" mean *what you see*, wide characters, tree glyphs and all.
    #[cfg(feature = "mouse")]
    snapshot: Option<Snapshot>,
    /// Cell and time of the last click, for pairing the next one with it.
    #[cfg(feature = "mouse")]
    last_click: Option<(u16, u16, Instant)>,
}

impl MouseState {
    /// Drop a selection that no longer describes what is on screen. Called
    /// when a keypress changes the content underneath it — a highlight left
    /// over a scrolled list is worse than no highlight.
    #[cfg(feature = "mouse")]
    pub fn clear_selection(&mut self) {
        self.snapshot = None;
        self.selection = None;
    }

    #[cfg(not(feature = "mouse"))]
    #[inline]
    pub fn clear_selection(&mut self) {}

    /// Record a click and report whether it completes a double click.
    ///
    /// A double click consumes the pair, so a third click in a row starts a
    /// new one instead of activating again on every further click. `now` is
    /// passed in rather than read here so the timeout is testable without
    /// sleeping through it.
    #[cfg(feature = "mouse")]
    fn register_click(&mut self, x: u16, y: u16, now: Instant) -> bool {
        let double = self.last_click.is_some_and(|(px, py, at)| {
            (px, py) == (x, y) && now.duration_since(at) < DOUBLE_CLICK
        });
        self.last_click = if double { None } else { Some((x, y, now)) };
        double
    }

    /// Copy the region's cells out of the freshly drawn frame, then tint the
    /// selected ones. Runs at the very end of the render pass, so it sees
    /// every overlay.
    #[cfg(feature = "mouse")]
    pub fn after_render(&mut self, buf: &mut Buffer, theme: &Theme) {
        let Some(sel) = self.selection else { return };
        self.snapshot = Some(Snapshot::capture(buf, sel.bounds));
        sel.paint(buf, theme);
    }

    #[cfg(not(feature = "mouse"))]
    #[inline]
    pub fn after_render(
        &mut self,
        _buf: &mut ratatui::buffer::Buffer,
        _theme: &crate::ui::theme::Theme,
    ) {
    }
}

/// Translate one mouse event into work on the app.
///
/// Lives next to the state rather than on `App` so the whole decision table
/// is readable in one place; the `App` method is a one-line delegate.
#[cfg(feature = "mouse")]
pub fn handle(app: &mut crate::app::App, ev: MouseEvent) -> EditorRequest {
    let (x, y) = (ev.column, ev.row);

    match ev.kind {
        // Press: anchor a selection inside whichever surface was hit. A press
        // outside every registered region just drops the old selection.
        MouseEventKind::Down(MouseButton::Left) => {
            match hit(x, y) {
                Some((bounds, _region)) => {
                    let block = ev.modifiers.contains(KeyModifiers::ALT);
                    app.mouse.selection = Some(Selection::new(bounds, x, y, block));
                    app.mouse.snapshot = None;
                }
                None => app.mouse.clear_selection(),
            }
            EditorRequest::None
        }

        // Drag: extend to the cursor, clamped into the region the drag
        // started in. This is the whole point — the selection cannot leave
        // its window even when the pointer does.
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = app.mouse.selection.as_mut() {
                sel.extend_to(x, y);
            }
            EditorRequest::None
        }

        // Release: a press and release on the same cell is a *click* and
        // belongs to whatever was drawn there; anything that moved is a drag
        // and belongs to the clipboard. Deciding it here is what lets one
        // button both select text and press things.
        MouseEventKind::Up(MouseButton::Left) => {
            // "Same cell" is not enough on its own: a drag that left the
            // region is clamped back onto it and can land on the anchor
            // again. Requiring the release to be inside the region too keeps
            // that from pressing whatever the pointer wandered onto.
            let clicked = app
                .mouse
                .selection
                .is_some_and(|sel| sel.is_click() && sel.bounds.contains(Position::new(x, y)));
            if clicked {
                app.mouse.clear_selection();
                return click(app, x, y);
            }
            if let Some(sel) = app.mouse.selection.as_mut() {
                sel.dragging = false;
            }
            // Success stays silent — the highlight is the receipt. Only a
            // clipboard that refused every path is worth a message.
            if finish_copy(app) == Some(false) {
                app.notify_error("Could not reach the clipboard".to_string());
            }
            EditorRequest::None
        }

        // Wheel: scroll whatever is under the pointer, not whatever happens
        // to be focused. On a content pane that means focusing it first —
        // the scroll would otherwise move a list the user is not looking at,
        // and focus-follows-wheel keeps a single scrolling path (the arrow
        // keys) instead of a second one per widget.
        MouseEventKind::ScrollDown => scroll(app, x, y, "down", true),
        MouseEventKind::ScrollUp => scroll(app, x, y, "up", false),
        MouseEventKind::ScrollRight => wheel(app, "right"),
        MouseEventKind::ScrollLeft => wheel(app, "left"),

        _ => EditorRequest::None,
    }
}

/// Route a click to the surface it landed on.
///
/// Everything reachable this way is reachable by key as well — the mouse is a
/// second way in, never a second implementation. What a click deliberately
/// does *not* do is reach past a popup: while one is open it owns the input,
/// so a stray click on the tab bar behind it must not switch tabs (the App
/// guards that, in the same place the key path does).
#[cfg(feature = "mouse")]
fn click(app: &mut crate::app::App, x: u16, y: u16) -> EditorRequest {
    let Some((_, region)) = hit(x, y) else {
        return EditorRequest::None;
    };
    let double = app.mouse.register_click(x, y, Instant::now());
    match region {
        Region::Tab(tab) => app.activate_tab(tab),
        Region::SubTab(idx) => return app.activate_subtab(idx),
        // A pane takes the focus first — the row cursor it is about to move
        // is the one the keys act on, so the two must not end up in
        // different panes.
        Region::ContentPane(id) => {
            app.focus_content_pane(id);
            // The header row is a control, not data: it sorts. Everything
            // below it moves the cursor.
            if !app.click_column_header(id, x, y) {
                return app.click_content_row(id, x, y, double);
            }
        }
        _ => {}
    }
    EditorRequest::None
}

/// Send the wheel where the pointer is, rather than where the focus is.
#[cfg(feature = "mouse")]
fn scroll(app: &mut crate::app::App, x: u16, y: u16, key: &str, forward: bool) -> EditorRequest {
    match hit(x, y).map(|(_, region)| region) {
        // The bar has nothing to scroll, and walking the tabs is what a wheel
        // does on a tab strip everywhere else.
        Some(Region::Tab(_) | Region::SubTab(_) | Region::TabBar) => {
            app.cycle_tab(forward);
            EditorRequest::None
        }
        // Focus follows the wheel: scrolling a pane the keys would not reach
        // leaves the two out of step, and the alternative is a second
        // scrolling path per widget.
        Some(Region::ContentPane(id)) => {
            app.focus_content_pane(id);
            wheel(app, key)
        }
        _ => wheel(app, key),
    }
}

/// Extract the highlighted text and put it on the clipboard.
///
/// `None` when there was nothing to copy — no selection, or one that covers
/// only blanks — otherwise whether the clipboard took it.
#[cfg(feature = "mouse")]
fn finish_copy(app: &mut crate::app::App) -> Option<bool> {
    let sel = app.mouse.selection?;
    let snapshot = app.mouse.snapshot.as_ref()?;
    let text = sel.text(snapshot);
    if text.is_empty() {
        return None;
    }
    Some(crate::clipboard::copy(&text))
}

/// Feed synthetic arrow keys through the normal key pipeline, so the wheel
/// does exactly what the key does — including every view-specific binding on
/// it — with no second scrolling path to keep in sync.
#[cfg(feature = "mouse")]
fn wheel(app: &mut crate::app::App, key: &str) -> EditorRequest {
    for _ in 0..WHEEL_LINES {
        let req = app.handle_key(key);
        // A scroll should never open an editor, but if a view binds one of
        // the arrows to something that does, honour it once rather than
        // firing it three times.
        if !matches!(req, EditorRequest::None) {
            return req;
        }
    }
    EditorRequest::None
}

#[cfg(test)]
#[cfg(feature = "mouse")]
mod tests {
    use super::*;

    fn scene() {
        begin_frame();
        push(Rect::new(0, 0, 80, 24), Region::ContentPane(1));
        push(Rect::new(10, 5, 30, 10), Region::Popup);
    }

    #[test]
    fn topmost_surface_wins() {
        // The popup is pushed after the pane, so a cell both cover belongs to
        // the popup — the same order the render pass paints in.
        scene();
        assert_eq!(hit(15, 7).map(|(_, r)| r), Some(Region::Popup));
        assert_eq!(hit(2, 2).map(|(_, r)| r), Some(Region::ContentPane(1)));
    }

    #[test]
    fn a_hit_reports_the_rect_to_clip_against() {
        scene();
        assert_eq!(hit(15, 7).map(|(r, _)| r), Some(Rect::new(10, 5, 30, 10)));
    }

    #[test]
    fn empty_rects_are_not_registered() {
        // Zero-height bars (a collapsed notification strip) must not swallow
        // clicks meant for whatever is drawn at the same row.
        begin_frame();
        push(Rect::new(0, 5, 80, 0), Region::NotificationBar);
        push(Rect::new(0, 5, 80, 1), Region::StatusBar);
        assert_eq!(hit(0, 5).map(|(_, r)| r), Some(Region::StatusBar));
    }

    #[test]
    fn a_label_pushed_after_the_bar_wins_over_it() {
        // The tab bar registers its background first and its labels while
        // painting them, so a click between two labels falls through to the
        // bar — which switches nothing — instead of the nearest tab.
        begin_frame();
        push(Rect::new(0, 0, 80, 1), Region::TabBar);
        push(
            Rect::new(0, 0, 8, 1),
            Region::Tab(crate::tabs::Tab::Content(0)),
        );
        push(
            Rect::new(8, 0, 9, 1),
            Region::Tab(crate::tabs::Tab::Content(1)),
        );
        assert_eq!(
            hit(3, 0).map(|(_, r)| r),
            Some(Region::Tab(crate::tabs::Tab::Content(0)))
        );
        assert_eq!(hit(40, 0).map(|(_, r)| r), Some(Region::TabBar));
    }

    #[test]
    fn two_quick_clicks_on_one_cell_are_a_double_click() {
        let mut state = MouseState::default();
        let t0 = Instant::now();
        assert!(!state.register_click(4, 2, t0));
        assert!(state.register_click(4, 2, t0 + DOUBLE_CLICK / 2));
        // The pair is consumed: holding the button down and clicking on must
        // not activate the row again on every further click.
        assert!(!state.register_click(4, 2, t0 + DOUBLE_CLICK / 2));
    }

    #[test]
    fn a_slow_second_click_or_one_on_another_cell_is_not_a_double_click() {
        let mut state = MouseState::default();
        let t0 = Instant::now();
        assert!(!state.register_click(4, 2, t0));
        assert!(!state.register_click(4, 2, t0 + DOUBLE_CLICK * 2));

        let mut state = MouseState::default();
        assert!(!state.register_click(4, 2, t0));
        assert!(!state.register_click(4, 3, t0));
    }

    #[test]
    fn a_miss_is_a_miss() {
        scene();
        assert_eq!(hit(100, 100), None);
    }

    #[test]
    fn a_new_frame_forgets_the_old_one() {
        scene();
        begin_frame();
        assert_eq!(hit(15, 7), None);
    }
}
