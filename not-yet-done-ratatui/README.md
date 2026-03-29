# not-yet-done-ratatui — Widget Library

This document covers two audiences:

- **Users** — how to include and use the existing widgets and form in an application
- **Contributors** — how new widgets are structured, what conventions to follow,
  and how to integrate them with the form layer

---

## Table of Contents

1. [User Guide](#1-user-guide)
   1. [TextInput](#11-textinput)
   2. [MultiChoice](#12-multichoice)
   3. [Form](#13-form)
   4. [Utilities](#14-utilities)
2. [Developer Guide](#2-developer-guide)
   1. [Project layout](#21-project-layout)
   2. [Design principles](#22-design-principles)
   3. [Anatomy of a widget](#23-anatomy-of-a-widget)
   4. [Styling system](#24-styling-system)
   5. [Form integration](#25-form-integration)
   6. [Adding a new widget](#26-adding-a-new-widget)
   7. [Common pitfalls](#27-common-pitfalls)

---

## 1. User Guide

All public types are re-exported from the crate root:

```rust
use not_yet_done_ratatui::{
    TextInput, TextInputState, TextInputStyle, TextInputStyleType, TextInputKeymap, TextInputEvent,
    MultiChoice, MultiChoiceState, MultiChoiceStyle, MultiChoiceStyleType,
        MultiChoiceKeymap, MultiChoiceEvent,
    Form, FormState, FormField, FormFieldState, FormStyle, FormWidgetStyle,
        FormKeymap, FormEvent, FieldEvent,
    KeyBinding, hex_color,
};
```

---

### 1.1 TextInput

A single-line text field with a title, input line, and error line.

```
▍ Title
▍ value or placeholder text
  ⚠ error message (only when an error is set)
```

#### State

`TextInputState` holds the mutable value, cursor position and optional error.
Create one per field and keep it in your app struct.

```rust
let mut state = TextInputState::new();

// Read the current value
let v = state.value();

// Errors (set/clear by your validation code)
state.set_error("At least 3 characters required");
state.clear_error();
```

#### Widget

`TextInput` is stateless and rebuilt every frame.

```rust
TextInput::new("Username")
    .placeholder("e.g. alice")
    .width(40)                      // optional; defaults to area width
    .style(my_style)
    .keymap(my_keymap)
    .render_with_state(area, frame.buffer_mut(), &state);
```

#### Keymap

```rust
// Default bindings
let keymap = TextInputKeymap::default();
// ← / →       move cursor
// Backspace   delete backwards
// Delete      delete forwards
// Ctrl+U      clear

// Custom bindings (Emacs-style example)
let keymap = TextInputKeymap {
    move_left:   KeyBinding::ctrl(KeyCode::Char('b')),
    move_right:  KeyBinding::ctrl(KeyCode::Char('f')),
    delete_back: KeyBinding::ctrl(KeyCode::Char('h')),
    delete_fwd:  KeyBinding::ctrl(KeyCode::Char('d')),
    clear:       KeyBinding::ctrl(KeyCode::Char('k')),
};
```

`KeyBinding::new(code)` — no modifiers.
`KeyBinding::ctrl(code)` — Ctrl+key.
For other modifiers, build the struct directly:
```rust
KeyBinding { code: KeyCode::BackTab, modifiers: KeyModifiers::SHIFT }
```

#### Events

Call `state.handle_event(&event, &keymap)` in your event loop and match the
return value:

```rust
match state.handle_event(&event, &keymap) {
    TextInputEvent::Changed(new_value) => { /* live validation */ }
    TextInputEvent::Ignored => {}
}
```

#### Cursor position

After rendering, set the terminal cursor:

```rust
if !state.value().is_empty() {
    let pos = TextInput::new("").width(area.width)
        .cursor_position(area, &state);
    frame.set_cursor_position(pos);
}
```

#### Style

```rust
let style = TextInputStyle::new()
    .prefix_color(Color::Rgb(100, 180, 255))
    .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
    .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG).bg(INPUT_BG))
    .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
    .placeholder_color(Color::Rgb(80, 80, 110));
```

| `TextInputStyleType` | Affects |
|---|---|
| `Title` | Title row |
| `Input` | Input row (text and background) |
| `Error` | Error row |

Unset slots fall back to `Style::default()`.  When used inside a `Form`, unset
slots are filled in by the form's global style (see [Form](#13-form)).

---

### 1.2 MultiChoice

A dropdown-style multi-select widget.

**Collapsed** (field is not active):
```
▍ Protocol
▍ HTTP, HTTPS
```

**Expanded** (field is active, `state.open == true`):
```
▍ Protocol
▍▶ HTTP         ← cursor row
▍  HTTPS
▍  WS
▍  WSS
              ← blank closing line
```

When expanded the widget **writes below its layout slot**, overlapping whatever
is beneath it.  Render it last among potentially overlapped siblings (the `Form`
widget handles this automatically).

#### State

```rust
let mut state = MultiChoiceState::new(4); // number of options

// Open / close from outside (e.g. on focus change)
state.open();
state.close();

// Read selected indices
let selected: Vec<usize> = state.selected_indices();
```

#### Widget

```rust
let choices = ["HTTP", "HTTPS", "WS", "WSS"];

MultiChoice::new("Protocol", &choices)
    .placeholder("Choose protocol(s)")
    .style(my_style)
    .keymap(my_keymap)
    .render_with_state(area, frame.buffer_mut(), &state);
```

#### Keymap

```rust
// Default bindings
let keymap = MultiChoiceKeymap::default();
// Ctrl+J   move cursor down
// Ctrl+K   move cursor up
// Space    toggle selection

// Custom bindings (arrow keys)
let keymap = MultiChoiceKeymap {
    move_down: KeyBinding::new(KeyCode::Down),
    move_up:   KeyBinding::new(KeyCode::Up),
    toggle:    KeyBinding::new(KeyCode::Char(' ')),
};
```

#### Events

```rust
match state.handle_event(&event, &keymap) {
    MultiChoiceEvent::SelectionChanged(indices) => { /* … */ }
    MultiChoiceEvent::Opened  => {}
    MultiChoiceEvent::Closed  => {}
    MultiChoiceEvent::Ignored => {}
}
```

#### Style

```rust
let style = MultiChoiceStyle::new()
    .prefix_color(ACCENT)
    .set_style(MultiChoiceStyleType::Title,          Style::default().fg(ACCENT))
    .set_style(MultiChoiceStyleType::Normal,         Style::default().fg(BODY_FG))
    .set_style(MultiChoiceStyleType::Active,         Style::default().fg(YELLOW))
    .set_style(MultiChoiceStyleType::Selected,       Style::default().bg(SEL_BG))
    .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(YELLOW).bg(SEL_BG))
    .set_style(MultiChoiceStyleType::LastLine,       Style::default().bg(PANEL_BG));
```

| `MultiChoiceStyleType` | Affects |
|---|---|
| `Title` | Title row |
| `Normal` | Item row — not selected, cursor elsewhere |
| `Active` | Item row — not selected, cursor here (keyboard focus) |
| `Selected` | Item row — selected, cursor elsewhere |
| `SelectedActive` | Item row — selected and cursor here |
| `LastLine` | Blank closing line below the expanded list |

`LastLine` should match the panel background so the drop-down closes cleanly.

---

### 1.3 Form

`Form` composes multiple `TextInput` and `MultiChoice` widgets into a managed
form with automatic layout, focus handling, style inheritance, and correct
render order.

#### Overview of types

| Type | Role |
|---|---|
| `Form` | Stateless widget, rebuilt every frame |
| `FormState` | Mutable runtime state (active field, per-field states) |
| `FormField` | Descriptor for one field (widget + layout height) |
| `FormFieldState` | Enum wrapping `TextInputState` or `MultiChoiceState` |
| `FormStyle` | Global active/inactive style defaults |
| `FormWidgetStyle` | Style for one focus state (active or inactive) |
| `FormKeymap` | Form-level keybindings (focus next/prev, confirm) |
| `FormEvent` | Event returned by `Form::handle_event` |
| `FieldEvent` | Per-field event, tagged with field index |

#### Setting up state

```rust
let mut form_state = FormState::new(vec![
    FormFieldState::TextInput(TextInputState::new()),
    FormFieldState::MultiChoice(MultiChoiceState::new(CHOICES.len())),
    FormFieldState::TextInput(TextInputState::new()),
]);
// The first field receives focus; if it is a MultiChoice it opens automatically.
```

Accessing field state by index:

```rust
// Read
let text = form_state.field(0)
    .and_then(|f| f.as_text_input())
    .map(|ts| ts.value())
    .unwrap_or_default();

// Mutate (e.g. validation)
if let Some(ts) = form_state.field_mut(0).and_then(|f| f.as_text_input_mut()) {
    ts.set_error("Required");
}
```

#### Building the form

```rust
let form = Form::new()
    .style(global_style)    // FormStyle — see below
    .spacing(1)             // blank rows between fields (default: 1)
    .keymap(form_keymap)    // FormKeymap — see below
    .field(FormField::text_input(
        TextInput::new("Username").placeholder("…"),
    ))
    .field(FormField::multi_choice(
        MultiChoice::new("Role", &ROLES).placeholder("…"),
    ).with_height(3))       // override layout height when needed
    .field(FormField::text_input(
        TextInput::new("E-mail").placeholder("…"),
    ));
```

Default layout heights: `TextInput` = 3 rows, `MultiChoice` = 2 rows.
Use `.with_height(n)` when you need a different allocation (e.g. to align a
`MultiChoice` with `TextInput` fields).

#### Rendering

```rust
// in your render function:
form.render_with_state(form_area, frame.buffer_mut(), &form_state);

// terminal cursor for the active TextInput:
if let Some(pos) = form.cursor_position(form_area, &form_state) {
    frame.set_cursor_position(pos);
}
```

#### Event handling

```rust
match form.handle_event(&event, &mut form_state) {
    FormEvent::Submit => {
        // Enter was pressed on a TextInput field — validate and act.
    }
    FormEvent::FocusChanged { from, to } => {
        // Tab / Shift+Tab moved focus. form_state.active is already updated.
    }
    FormEvent::FieldEvent { index, event } => {
        // A widget-level event occurred in field `index`.
        if let FieldEvent::TextInput(TextInputEvent::Changed(_)) = event {
            validate_field(index, &mut form_state);
        }
    }
    FormEvent::Ignored => {}
}
```

Enter on a `MultiChoice` field toggles its drop-down open/closed (does not
emit `Submit`).

#### FormKeymap

```rust
// Default bindings
let keymap = FormKeymap::default();
// Tab          focus next
// Shift+Tab    focus previous
// Enter        submit (TextInput) / toggle MC (MultiChoice)

// Custom bindings
let keymap = FormKeymap {
    focus_next: KeyBinding::ctrl(KeyCode::Char('n')),
    focus_prev: KeyBinding::ctrl(KeyCode::Char('p')),
    confirm:    KeyBinding::ctrl(KeyCode::Char('s')),
};

let form = Form::new().keymap(keymap) /* … */;
```

#### FormStyle — global style defaults

`FormStyle` defines default styles for active and inactive fields.  These are
merged into each widget's own style before rendering; any slot already set on
the widget itself is left untouched.

```rust
let global_style = FormStyle::new()
    .background(PANEL_BG)   // fills form area before rendering fields
    .active(
        FormWidgetStyle::new()
            .prefix_color(GREEN)
            .title(Style::default().fg(GREEN).bg(FIELD_BG))
            .body(Style::default().fg(WHITE).bg(FIELD_BG))
            .placeholder(Color::Rgb(80, 100, 80))
            .error(Style::default().fg(RED))
            // MultiChoice-specific active slots:
            .mc_cursor(Style::default().fg(YELLOW).bg(FIELD_BG))
            .mc_selected(Style::default().bg(SEL_BG))
            .mc_selected_cursor(Style::default().fg(YELLOW).bg(SEL_BG))
            .mc_closing_line(Style::default().bg(PANEL_BG)),
    )
    .inactive(
        FormWidgetStyle::new()
            .prefix_color(BLUE)
            .title(Style::default().fg(BLUE))
            .body(Style::default().fg(WHITE))
            .placeholder(Color::Rgb(50, 50, 80))
            .error(Style::default().fg(RED)),
    );
```

`FormWidgetStyle` → widget style slot mapping:

| `FormWidgetStyle` field | `TextInputStyleType` | `MultiChoiceStyleType` |
|---|---|---|
| `prefix_color` | `prefix_color` | `prefix_color` |
| `title` | `Title` | `Title` |
| `body` | `Input` | `Normal` |
| `error` | `Error` | — |
| `placeholder` | `placeholder_color` | — |
| `mc_cursor` | — | `Active` |
| `mc_selected` | — | `Selected` |
| `mc_selected_cursor` | — | `SelectedActive` |
| `mc_closing_line` | — | `LastLine` |

#### Per-widget style overrides

Set style slots on the widget itself to override the form global style for
that specific field.  Only set slots win; unset slots still inherit.

```rust
// Only the title of this one field uses a special cyan colour.
// Body, error, placeholder all still come from the form global style.
FormField::text_input(
    TextInput::new("Server Name")
        .style(
            TextInputStyle::new()
                .set_style(TextInputStyleType::Title,
                    Style::default().fg(CYAN).bg(FIELD_BG)),
        ),
)
```

#### Render order and MultiChoice overlay

`Form` always renders inactive fields first, then the active field last.
This means an open `MultiChoice` drop-down can safely overlap any fields below
it — no manual z-ordering is needed.  Place a `MultiChoice` wherever it
belongs logically; the form handles the rest.

---

### 1.4 Utilities

#### `hex_color`

Parses a CSS hex string into a `ratatui::style::Color::Rgb`:

```rust
let blue = hex_color("#64B4FF");
```

#### `open_editor`

Suspends the TUI, opens `$EDITOR` for the given path, and resumes:

```rust
use not_yet_done_ratatui::{open_editor, EditorError};

ratatui::restore();
open_editor(path)?;
let terminal = ratatui::init();
```

---

## 2. Developer Guide

### 2.1 Project layout

```
src/
├── lib.rs                   — public re-exports
├── utils/
│   ├── mod.rs
│   └── open_editor.rs
└── widgets/
    ├── mod.rs
    ├── common/              — shared primitives (no widget logic)
    │   ├── mod.rs
    │   ├── keymap.rs        — KeyBinding
    │   ├── render.rs        — render_prefixed_line, truncate_to_width
    │   └── style.rs         — hex_color, impl_widget_style_base! macro
    ├── text_input/
    │   ├── mod.rs           — TextInput widget + render_with_state
    │   ├── keymap.rs        — TextInputKeymap
    │   ├── state.rs         — TextInputState, TextInputEvent
    │   └── style.rs         — TextInputStyle, TextInputStyleType
    ├── multi_choice/
    │   ├── mod.rs           — MultiChoice widget + render_with_state
    │   ├── keymap.rs        — MultiChoiceKeymap
    │   ├── state.rs         — MultiChoiceState, MultiChoiceEvent
    │   └── style.rs         — MultiChoiceStyle, MultiChoiceStyleType
    └── form/
        ├── mod.rs           — Form widget, render_with_state, handle_event
        ├── field.rs         — FormField, FormFieldWidget
        ├── keymap.rs        — FormKeymap
        ├── state.rs         — FormState, FormFieldState, FormEvent, FieldEvent
        └── style.rs         — FormStyle, FormWidgetStyle, merge_* helpers
```

Each widget is fully self-contained in its own subdirectory.  `common/` is the
only shared dependency; widgets must not import each other (the form layer is
the only place that knows about multiple widget types).

---

### 2.2 Design principles

**Separation of state and widget.**
The widget struct (`TextInput`, `MultiChoice`, `Form`) is stateless and
rebuilt on every frame.  All mutable data lives in a separate `*State` struct
owned by the application.  This follows Ratatui conventions and avoids borrow
conflicts between rendering and event handling.

**Builder API for configuration.**
Widget and style structs use consuming builder methods (`fn foo(mut self, …) -> Self`).
Every method call returns `Self`, enabling chaining.  Fields that are optional
are stored as `Option<T>`; the absence of a value is meaningful (see styling).

**Widgets own their keyboard logic.**
Each widget defines its own keymap struct and processes events via
`state.handle_event(&event, &keymap)`.  The form layer wraps this — it does
not re-implement widget key handling.

**Events, not callbacks.**
`handle_event` returns an event enum value rather than calling closures.  The
application decides what to do with each event (validation, side-effects, etc.).

**Unicode-correct rendering.**
All text is measured and truncated using display width (`unicode-width`), not
byte or character count.  Truncated strings get a `…` suffix.  All `px`
advances in render code use `ch.width().unwrap_or(1)`.

**English only.**
All code comments, doc comments, error messages, placeholder text, and example
strings are in English.

---

### 2.3 Anatomy of a widget

A complete widget consists of four files:

#### `keymap.rs`

Defines `MyWidgetKeymap` with one `KeyBinding` field per action and a
`Default` impl with sensible defaults.

```rust
#[derive(Debug, Clone)]
pub struct MyWidgetKeymap {
    pub confirm: KeyBinding,
    pub cancel:  KeyBinding,
}

impl Default for MyWidgetKeymap {
    fn default() -> Self {
        Self {
            confirm: KeyBinding::new(KeyCode::Enter),
            cancel:  KeyBinding::new(KeyCode::Esc),
        }
    }
}
```

#### `state.rs`

Defines `MyWidgetState` (pure data, `Default` + `Clone`) and `MyWidgetEvent`
(enum of observable outcomes).

```rust
#[derive(Debug, Clone)]
pub enum MyWidgetEvent {
    Confirmed(String),
    Cancelled,
    Ignored,
}

#[derive(Debug, Default, Clone)]
pub struct MyWidgetState {
    pub value: String,
    // internal fields are pub(crate) or private
}

impl MyWidgetState {
    pub fn new() -> Self { Self::default() }

    pub fn handle_event(
        &mut self,
        event: &Event,
        keymap: &MyWidgetKeymap,
    ) -> MyWidgetEvent {
        // match pressed key against keymap bindings
        // mutate self as needed
        // return the appropriate event variant
        MyWidgetEvent::Ignored
    }
}
```

#### `style.rs`

Defines `MyWidgetStyleType` (repr(u8) enum) and `MyWidgetStyle`.

```rust
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MyWidgetStyleType {
    Title = 0,
    Body  = 1,
}

#[derive(Debug, Clone)]
pub struct MyWidgetStyle {
    pub prefix_color: Option<Color>,
    pub styles: [Option<Style>; 2],   // length must match variant count
}

impl Default for MyWidgetStyle {
    fn default() -> Self {
        Self { prefix_color: None, styles: [None; 2] }
    }
}

impl MyWidgetStyle {
    pub fn new() -> Self { Self::default() }
    // Any widget-specific builder methods (e.g. placeholder_color) go here.
}

// Generates: prefix_color(), set_style(), style(), resolved_style()
impl_widget_style_base!(MyWidgetStyle, MyWidgetStyleType);
```

#### `mod.rs`

Defines the stateless `MyWidget` struct with a builder API and
`render_with_state`.

```rust
pub mod keymap;
pub mod state;
pub mod style;

pub use keymap::MyWidgetKeymap;
pub use state::{MyWidgetEvent, MyWidgetState};
pub use style::{MyWidgetStyle, MyWidgetStyleType};

#[derive(Debug, Clone)]
pub struct MyWidget<'a> {
    pub title:   &'a str,
    pub style:   MyWidgetStyle,
    pub keymap:  MyWidgetKeymap,
}

impl<'a> MyWidget<'a> {
    pub fn new(title: &'a str) -> Self {
        Self { title, style: MyWidgetStyle::default(), keymap: MyWidgetKeymap::default() }
    }
    pub fn style(mut self, s: MyWidgetStyle) -> Self { self.style = s; self }
    pub fn keymap(mut self, k: MyWidgetKeymap) -> Self { self.keymap = k; self }

    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &MyWidgetState) {
        let total_width = area.width;
        let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;

        let title_style = self.style.resolved_style(MyWidgetStyleType::Title);
        render_prefixed_line(buf, area.x, area.y, total_width, self.title,
            text_width, &self.style.prefix_color, &title_style, false);
        // … further rows
    }
}

impl Widget for MyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_with_state(area, buf, &MyWidgetState::default());
    }
}
```

Finally, export from `src/widgets/mod.rs` and `src/lib.rs`.

---

### 2.4 Styling system

#### Option-based slots

Every style slot is `Option<Style>` (or `Option<Color>` for colour-only
fields).  `None` means "not configured by this level".

```
widget.style(SlotType)           → Option<&Style>   (None = not set)
widget.resolved_style(SlotType)  → Style             (falls back to default)
```

Render code always calls `resolved_style` — it never unwraps `style()` directly.
The form's merge logic calls `style()` to check whether the widget has already
configured a slot before filling it in from the global style.

#### impl_widget_style_base! macro

Any widget style struct with the following two public fields:

```rust
pub prefix_color: Option<Color>,
pub styles: [Option<Style>; N],
```

can derive the standard accessor methods with one line:

```rust
impl_widget_style_base!(MyWidgetStyle, MyWidgetStyleType);
```

This generates `prefix_color()`, `set_style()`, `style()`, and
`resolved_style()`.  All other widget-specific builder methods (e.g.
`placeholder_color`) must be added manually in `impl MyWidgetStyle`.

#### Precedence chain

```
widget-level set_style(...)   ← highest priority (Some wins)
        ↓
form global style (active / inactive FormWidgetStyle)
        ↓
Style::default()              ← fallback when everything is None
```

The merge happens in `form/style.rs` (`merge_text_input` /
`merge_multi_choice`).  It only writes to slots that are still `None` on the
widget.

---

### 2.5 Form integration

To make a new widget usable inside `Form`, three things are needed:

#### 1. Add a variant to `FormFieldWidget` and `FormFieldState`

In `form/field.rs`:
```rust
pub enum FormFieldWidget<'a> {
    TextInput(TextInput<'a>),
    MultiChoice(MultiChoice<'a>),
    MyWidget(MyWidget<'a>),         // ← new
}
```

In `form/state.rs`:
```rust
pub enum FormFieldState {
    TextInput(TextInputState),
    MultiChoice(MultiChoiceState),
    MyWidget(MyWidgetState),        // ← new
}
// Add as_my_widget() / as_my_widget_mut() accessors.
```

#### 2. Add a merge function in `form/style.rs`

```rust
pub(crate) fn merge_my_widget(
    mut widget: MyWidgetStyle,
    form_slot: &FormWidgetStyle,
) -> MyWidgetStyle {
    if widget.prefix_color.is_none() {
        widget.prefix_color = form_slot.prefix_color;
    }
    if widget.style(MyWidgetStyleType::Title).is_none() {
        if let Some(s) = form_slot.title {
            widget = widget.set_style(MyWidgetStyleType::Title, s);
        }
    }
    // … map other FormWidgetStyle fields to your style types
    widget
}
```

Map `FormWidgetStyle` fields to your `StyleType` variants.  If none of the
existing `FormWidgetStyle` fields fit, add new ones there (and update the
mapping table in the doc comment).

#### 3. Handle the new variant in `Form::render_field` and `Form::handle_event`

In `form/mod.rs`, add match arms:

```rust
// render_field
(FormFieldWidget::MyWidget(w), FormFieldState::MyWidget(s)) => {
    let merged = MyWidget {
        style: merge_my_widget(w.style.clone(), form_slot),
        ..w.clone()
    };
    merged.render_with_state(rect, buf, s);
}

// handle_event
(FormFieldWidget::MyWidget(w), FormFieldState::MyWidget(s)) => {
    if pressed == self.keymap.confirm {
        return FormEvent::Submit;
    }
    let ev = s.handle_event(event, &w.keymap);
    FormEvent::FieldEvent { index: active, event: FieldEvent::MyWidget(ev) }
}
```

Also add the `MyWidget` variant to `FieldEvent` in `form/state.rs`.

#### Overlay-aware widgets

If the new widget can render outside its layout slot (like `MultiChoice`
when expanded), no special handling is needed — `Form` already renders the
active field last.  The widget just writes to whatever buffer cells it needs.

---

### 2.6 Adding a new widget — checklist

- [ ] Create `src/widgets/my_widget/` with `keymap.rs`, `state.rs`, `style.rs`, `mod.rs`
- [ ] `StyleType` enum: `#[repr(u8)]`, variants start at 0, `COUNT` associated constant
- [ ] Style struct: `pub prefix_color: Option<Color>`, `pub styles: [Option<Style>; N]`,
      `Default` initialises everything to `None`, call `impl_widget_style_base!`
- [ ] Widget struct: stateless, builder API, `render_with_state(area, buf, state)`
      takes `Rect` (not `x, y, width` separately)
- [ ] Render code: use `resolved_style()`, advance `px` by `ch.width().unwrap_or(1)`,
      truncate text with `truncate_to_width(text, width)` from `common::render`
- [ ] `handle_event` lives on the state struct, takes `(&Event, &Keymap) -> Event`
- [ ] Implement `ratatui::widgets::Widget` as a thin wrapper around
      `render_with_state` with a default state
- [ ] Re-export from `src/widgets/mod.rs` and `src/lib.rs`
- [ ] If form integration is needed, follow [§2.5](#25-form-integration)
- [ ] Add an example under `examples/` demonstrating the widget standalone

---

### 2.7 Common pitfalls

**`px` advance must use display width.**
CJK characters are 2 columns wide.  Always use `ch.width().unwrap_or(1) as u16`
when advancing the cursor, never `+= 1`.

**`truncate_to_width` returns a `String`.**
Unlike a naive `&str` slice, it measures display width and appends `…` when
truncated.  Do not cache the return value across frames.

**`resolved_style` vs `style`.**
Render code must always call `resolved_style()` (returns `Style`, never panics).
Use `style()` (returns `Option<&Style>`) only in form merge logic where `None`
is meaningful.

**`render_with_state` takes `Rect`.**
Both `TextInput` and `MultiChoice` take a `Rect` for their area.  A
`MultiChoice` may render below `area.bottom()` when expanded — that is
intentional and must not be guarded against.

**State and widget types must match.**
`Form` silently skips a field whose `FormFieldWidget` variant does not match
its `FormFieldState` variant.  Keep them aligned when constructing `FormState`.

**`SkimMatcherV2` or similar per-frame allocations.**
If a widget performs fuzzy matching or other expensive computation, create the
matcher once (e.g. in the app struct or a `FormState` companion) and reuse it.
Do not construct it inside render functions or `handle_event`.

**`block_on` inside a Tokio runtime panics.**
If the application uses async, delegate async work through channels.  Never
call `block_on` from within a render or event-handling call stack that is
already inside a Tokio runtime.
