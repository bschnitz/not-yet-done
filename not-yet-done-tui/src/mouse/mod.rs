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

/// The longest run of clicks that still means something: word, then line.
#[cfg(feature = "mouse")]
const TRIPLE_CLICK: u8 = 3;

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
    /// The breadcrumb line above a drilled-down pane.
    Breadcrumbs,
    /// One crumb of that line, carrying how many levels up it sits from the
    /// level on screen — 0 for the current one, which is where you already
    /// are. A *distance* rather than an index for the same reason
    /// [`Region::PopupRow`] carries one: the crumb knows how deep the pane
    /// stood when it was painted, and going up one level is what the back key
    /// already does.
    Crumb(u16),
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
    /// One entry row of a popup's list, carrying how far the list's cursor
    /// has to move to reach it. A *delta* rather than an index because no
    /// second implementation should know which of the fifteen popups is open:
    /// the row that was painted knows where the cursor stood when it was
    /// painted, and moving it is what the arrow keys already do.
    PopupRow(i16),
}

impl Region {
    /// Whether this region is a control rather than a surface.
    ///
    /// Controls are pushed *on top of* the surface they sit in, so a click
    /// finds them first — but a drag started on one should still select
    /// across the whole popup or tab bar behind it, not be trapped in a
    /// single row or label.
    #[cfg(feature = "mouse")]
    fn is_control(self) -> bool {
        matches!(
            self,
            Region::PopupRow(_) | Region::Tab(_) | Region::SubTab(_) | Region::Crumb(_)
        )
    }

    /// Whether a popup, if one is open, leaves this region reachable.
    #[cfg(feature = "mouse")]
    fn belongs_to_popup(self) -> bool {
        matches!(self, Region::Popup | Region::PopupRow(_))
    }
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
    find(x, y, |_| true)
}

/// Like [`hit`], but skipping the controls — the region a drag started here
/// must stay inside.
#[cfg(feature = "mouse")]
fn hit_surface(x: u16, y: u16) -> Option<(Rect, Region)> {
    find(x, y, |r| !r.is_control())
}

/// Whether a hit on this region may act, given what else is on screen.
///
/// A popup owns the input while it is up — the key path routes every keystroke
/// into it — so a click that lands *behind* one must not switch a tab or move
/// a cursor there. The frame's own map answers this: every popup draws through
/// [`PanelChrome`](crate::ui::panel_chrome::PanelChrome) or pushes its panel
/// itself, so "a popup is open" is exactly "this frame pushed a
/// [`Region::Popup`]" — no second list of the fifteen popups to keep in step
/// with `App`, and the transient overlays (the which-key hint, a modal
/// message) are covered by the same line.
///
/// Only *acting* is blocked. Selecting text behind a popup stays allowed: the
/// popup owns the input, not the screen, and copying a value you can see is
/// the reason the whole selection exists.
#[cfg(feature = "mouse")]
fn blocked_by_popup(region: Region) -> bool {
    !region.belongs_to_popup()
        && REGIONS.with_borrow(|regions| regions.iter().any(|(_, r)| *r == Region::Popup))
}

#[cfg(feature = "mouse")]
fn find(x: u16, y: u16, keep: impl Fn(Region) -> bool) -> Option<(Rect, Region)> {
    REGIONS.with_borrow(|regions| {
        regions
            .iter()
            .rev()
            .find(|(rect, region)| keep(*region) && rect.contains(Position::new(x, y)))
            .copied()
    })
}

/// Record the rows a popup list just painted, each carrying the distance from
/// the list's cursor to it.
///
/// Called right after the list's `view`, for the same reason [`push`] is
/// called where a surface is drawn: the geometry is only correct in the frame
/// that produced it. Lists without a cursor (the which-key display) register
/// nothing — there is nothing for a click to move.
#[cfg(feature = "mouse")]
pub fn push_list_rows(list: &not_yet_done_ratatui::LeaderList) {
    if !list.is_selectable() {
        return;
    }
    let cursor = list.selected() as isize;
    for row in list.painted_rows() {
        let delta = (row.index as isize - cursor).clamp(i16::MIN as isize, i16::MAX as isize);
        push(row.rect, Region::PopupRow(delta as i16));
    }
}

#[cfg(not(feature = "mouse"))]
#[inline]
pub fn push_list_rows(_list: &not_yet_done_ratatui::LeaderList) {}

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
    /// How many clicks that pairing has accumulated on that cell.
    #[cfg(feature = "mouse")]
    clicks: u8,
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

    /// Record a click and report how many it makes in a row: 1, 2 or 3.
    ///
    /// Three is where the ladder ends — a fourth click on the same cell starts
    /// over at one, the way a terminal's own word/line selection does, so
    /// holding the button down in place does not keep escalating. `now` is
    /// passed in rather than read here so the timeout is testable without
    /// sleeping through it.
    #[cfg(feature = "mouse")]
    fn register_click(&mut self, x: u16, y: u16, now: Instant) -> u8 {
        let repeat = self
            .last_click
            .filter(|&(px, py, at)| (px, py) == (x, y) && now.duration_since(at) < DOUBLE_CLICK)
            .map_or(0, |(_, _, _)| self.clicks);
        let clicks = if repeat >= TRIPLE_CLICK {
            1
        } else {
            repeat + 1
        };
        self.clicks = clicks;
        self.last_click = Some((x, y, now));
        clicks
    }

    /// Copy the region's cells out of the freshly drawn frame, then tint the
    /// selected ones. Runs at the very end of the render pass, so it sees
    /// every overlay.
    ///
    /// A press that has not moved yet is not tinted unless
    /// [`highlight_press`](crate::config::tui_config::MouseConfig::highlight_press)
    /// asks for it: almost every press turns out to be a click, and a single
    /// tinted cell that lives for one frame reads as a stray cursor rather
    /// than as a selection. The snapshot is taken either way — the second
    /// click of a double click reads the word out of it.
    #[cfg(feature = "mouse")]
    pub fn after_render(&mut self, buf: &mut Buffer, theme: &Theme, highlight_press: bool) {
        let Some(sel) = self.selection else { return };
        self.snapshot = Some(Snapshot::capture(buf, sel.bounds));
        if sel.dragging && sel.is_click() && !highlight_press {
            return;
        }
        sel.paint(buf, theme);
    }

    #[cfg(not(feature = "mouse"))]
    #[inline]
    pub fn after_render(
        &mut self,
        _buf: &mut ratatui::buffer::Buffer,
        _theme: &crate::ui::theme::Theme,
        _highlight_press: bool,
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
            match hit_surface(x, y) {
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
/// so a stray click on the tab bar behind it must not switch tabs. See
/// [`blocked_by_popup`].
///
/// Repeated clicks pick out text, the way they do in the terminal we took the
/// mouse away from: the second click takes the word, the third the line. The
/// second click yields where it already means something — a table row opens,
/// a popup entry is picked — but the third does not, so there is always a way
/// to grab a row's text without dragging across it.
#[cfg(feature = "mouse")]
fn click(app: &mut crate::app::App, x: u16, y: u16) -> EditorRequest {
    let Some((_, region)) = hit(x, y) else {
        app.mouse.clear_selection();
        return EditorRequest::None;
    };
    let clicks = app.mouse.register_click(x, y, Instant::now());
    let double = clicks == 2;
    // Behind a popup nothing acts, so every second click there is free for
    // the word under it.
    let blocked = blocked_by_popup(region);
    if clicks == TRIPLE_CLICK || (double && (blocked || !acts_on_double(app, region, x, y))) {
        return select_run(app, x, y, clicks == TRIPLE_CLICK);
    }
    // A single click is a press, not a selection: whatever the last one
    // highlighted is stale the moment this one lands.
    app.mouse.clear_selection();
    if blocked {
        return EditorRequest::None;
    }
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
        Region::PopupRow(delta) => return popup_row(app, delta, double),
        Region::Crumb(levels) => return app.click_breadcrumb(levels),
        _ => {}
    }
    EditorRequest::None
}

/// Whether the second click on this cell already means something.
///
/// Yes for what a second click still acts on: a popup entry, a data row, a
/// column header (which cycles its sort on every click). Everywhere else — the
/// bars, a popup's chrome, the empty space under the last row, the editor —
/// the second click is free, and text selection is what a user coming from the
/// terminal expects of it.
#[cfg(feature = "mouse")]
fn acts_on_double(app: &crate::app::App, region: Region, x: u16, y: u16) -> bool {
    match region {
        Region::PopupRow(_) => true,
        Region::ContentPane(id) => app.content_cell_acts(id, x, y),
        _ => false,
    }
}

/// Pick out the word or the line under the pointer and copy it.
///
/// Reshapes the selection the press already anchored, so it is clipped to the
/// same region a drag would be, and reads the text back from the snapshot of
/// the last frame — the same "copy what you see" the drag path uses.
#[cfg(feature = "mouse")]
fn select_run(app: &mut crate::app::App, x: u16, y: u16, line: bool) -> EditorRequest {
    // Taken out and put back so the selection can be reshaped against it;
    // both live on `app.mouse`.
    let Some(snap) = app.mouse.snapshot.take() else {
        return EditorRequest::None;
    };
    let picked = app.mouse.selection.as_mut().is_some_and(|sel| {
        if line {
            sel.select_line(&snap, y)
        } else {
            sel.select_word(&snap, x, y)
        }
    });
    app.mouse.snapshot = Some(snap);
    if picked && finish_copy(app) == Some(false) {
        app.notify_error("Could not reach the clipboard".to_string());
    }
    EditorRequest::None
}

/// Walk a popup list's cursor onto the clicked row, and open it on a double
/// click.
///
/// The walk goes through [`crate::app::App::handle_key`] like every other key
/// does, so whichever popup is up receives it through its own handler — no
/// resolver over the popups, and a list that treats the arrows specially keeps
/// doing so. The distance is bounded by the visible window, because both the
/// cursor and the clicked row are on screen.
#[cfg(feature = "mouse")]
fn popup_row(app: &mut crate::app::App, delta: i16, double: bool) -> EditorRequest {
    let key = if delta < 0 { "up" } else { "down" };
    for _ in 0..delta.unsigned_abs() {
        let req = app.handle_key(key);
        if !matches!(req, EditorRequest::None) {
            return req;
        }
    }
    if double {
        return app.handle_key("enter");
    }
    EditorRequest::None
}

/// Send the wheel where the pointer is, rather than where the focus is.
#[cfg(feature = "mouse")]
fn scroll(app: &mut crate::app::App, x: u16, y: u16, key: &str, forward: bool) -> EditorRequest {
    let region = hit(x, y).map(|(_, region)| region);
    // Behind a popup the wheel still reaches the keys — and they go to the
    // popup, which is where they belong — but it must not walk the tabs or
    // move the focus underneath it.
    if region.is_some_and(blocked_by_popup) {
        return wheel(app, key);
    }
    match region {
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

    /// The delta a row carries is what the arrow keys have to do to reach it,
    /// measured against the cursor as it stood when the frame was painted.
    #[test]
    fn list_rows_carry_the_distance_from_the_cursor() {
        use not_yet_done_ratatui::LeaderList;
        use ratatui::{Terminal, backend::TestBackend};
        use tuirealm::component::Component;
        use tuirealm::props::{AttrValue, Attribute};

        let mut list = LeaderList::default()
            .with_entries(vec![("a", "1"), ("b", "2"), ("c", "3")])
            .with_selectable(true);
        list.attr(Attribute::Value, AttrValue::Length(1));
        let mut terminal = Terminal::new(TestBackend::new(20, 3)).unwrap();
        terminal
            .draw(|frame| list.view(frame, frame.area()))
            .unwrap();

        begin_frame();
        push_list_rows(&list);
        assert_eq!(hit(0, 0).map(|(_, r)| r), Some(Region::PopupRow(-1)));
        assert_eq!(hit(0, 1).map(|(_, r)| r), Some(Region::PopupRow(0)));
        assert_eq!(hit(0, 2).map(|(_, r)| r), Some(Region::PopupRow(1)));
    }

    /// A list without a cursor has nothing for a click to move, so it stays
    /// out of the map and the click reaches the panel behind it.
    #[test]
    fn a_display_only_list_registers_no_rows() {
        use not_yet_done_ratatui::LeaderList;
        use ratatui::{Terminal, backend::TestBackend};
        use tuirealm::component::Component;

        let mut list = LeaderList::default().with_entries(vec![("a", "1"), ("b", "2")]);
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| list.view(frame, frame.area()))
            .unwrap();

        begin_frame();
        push(Rect::new(0, 0, 20, 2), Region::Popup);
        push_list_rows(&list);
        assert_eq!(hit(0, 0).map(|(_, r)| r), Some(Region::Popup));
    }

    #[test]
    fn a_popup_row_is_found_but_a_drag_still_spans_the_popup() {
        // Rows are pushed on top of the panel, so a click finds the row —
        // but a text selection started on one must not be trapped in a
        // single line.
        begin_frame();
        push(Rect::new(10, 5, 30, 10), Region::Popup);
        push(Rect::new(10, 7, 30, 1), Region::PopupRow(-2));
        push(Rect::new(10, 8, 30, 1), Region::PopupRow(-1));
        assert_eq!(hit(15, 7).map(|(_, r)| r), Some(Region::PopupRow(-2)));
        assert_eq!(
            hit_surface(15, 7),
            Some((Rect::new(10, 5, 30, 10), Region::Popup))
        );
    }

    #[test]
    fn a_drag_on_a_tab_label_spans_the_whole_bar() {
        begin_frame();
        push(Rect::new(0, 0, 80, 1), Region::TabBar);
        push(
            Rect::new(0, 0, 8, 1),
            Region::Tab(crate::tabs::Tab::Content(0)),
        );
        assert_eq!(
            hit_surface(3, 0),
            Some((Rect::new(0, 0, 80, 1), Region::TabBar))
        );
    }

    #[test]
    fn with_no_popup_on_screen_everything_acts() {
        begin_frame();
        push(Rect::new(0, 0, 80, 1), Region::TabBar);
        push(Rect::new(0, 1, 80, 23), Region::ContentPane(1));
        assert!(!blocked_by_popup(Region::ContentPane(1)));
        assert!(!blocked_by_popup(Region::Tab(crate::tabs::Tab::Content(0))));
    }

    #[test]
    fn an_open_popup_blocks_everything_behind_it() {
        // `scene` puts a popup over a pane. The popup and its rows stay
        // reachable; the surfaces it covers — and the bars it does not, which
        // is the point — do not.
        scene();
        assert!(blocked_by_popup(Region::ContentPane(1)));
        assert!(blocked_by_popup(Region::Tab(crate::tabs::Tab::Content(0))));
        assert!(blocked_by_popup(Region::Crumb(1)));
        assert!(!blocked_by_popup(Region::Popup));
        assert!(!blocked_by_popup(Region::PopupRow(-2)));
    }

    #[test]
    fn a_drag_across_the_breadcrumbs_is_not_trapped_in_one_crumb() {
        begin_frame();
        push(Rect::new(0, 1, 80, 1), Region::Breadcrumbs);
        push(Rect::new(0, 1, 6, 1), Region::Crumb(2));
        push(Rect::new(9, 1, 5, 1), Region::Crumb(1));
        assert_eq!(hit(10, 1).map(|(_, r)| r), Some(Region::Crumb(1)));
        assert_eq!(
            hit_surface(10, 1),
            Some((Rect::new(0, 1, 80, 1), Region::Breadcrumbs))
        );
    }

    #[test]
    fn quick_clicks_on_one_cell_count_up_to_three_and_start_over() {
        let mut state = MouseState::default();
        let mut t = Instant::now();
        let mut click = |state: &mut MouseState| {
            t += DOUBLE_CLICK / 2;
            state.register_click(4, 2, t)
        };
        assert_eq!(click(&mut state), 1);
        assert_eq!(click(&mut state), 2, "word");
        assert_eq!(click(&mut state), 3, "line");
        // A fourth click starts over rather than escalating: holding the
        // button down in place must not keep meaning something new.
        assert_eq!(click(&mut state), 1);
    }

    #[test]
    fn a_slow_second_click_or_one_on_another_cell_starts_over() {
        let mut state = MouseState::default();
        let t0 = Instant::now();
        assert_eq!(state.register_click(4, 2, t0), 1);
        assert_eq!(state.register_click(4, 2, t0 + DOUBLE_CLICK * 2), 1);

        let mut state = MouseState::default();
        assert_eq!(state.register_click(4, 2, t0), 1);
        assert_eq!(state.register_click(4, 3, t0), 1);
    }

    /// A press anchors a one-cell selection that the next event usually turns
    /// into a click. Tinting it in the meantime puts an orange block on screen
    /// for one frame, which is why it takes asking for.
    #[test]
    fn the_cell_under_a_press_is_left_alone_until_it_is_asked_for() {
        let theme = Theme::new(crate::config::ThemeConfig::default());
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let mut state = MouseState::default();
        state.selection = Some(Selection::new(area, 3, 0, false));

        state.after_render(&mut buf, &theme, false);
        assert_ne!(buf[(3, 0)].bg, theme.selection_bg());
        // The snapshot is taken either way — a double click reads its word
        // out of the frame the press was drawn in.
        assert!(state.snapshot.is_some());

        state.after_render(&mut buf, &theme, true);
        assert_eq!(buf[(3, 0)].bg, theme.selection_bg());
    }

    #[test]
    fn a_selection_that_moved_is_tinted_without_being_asked() {
        let theme = Theme::new(crate::config::ThemeConfig::default());
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let mut state = MouseState::default();
        let mut sel = Selection::new(area, 3, 0, false);
        sel.extend_to(5, 0);
        state.selection = Some(sel);

        state.after_render(&mut buf, &theme, false);
        assert_eq!(buf[(4, 0)].bg, theme.selection_bg());
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
