# not-yet-done-ratatui — Widget Library

This document covers two audiences:

- **Users** — how to include and use the existing widgets in an application
- **Contributors** — how new widgets are structured, what conventions to follow,
  and how to extend the library

> **Note on design.** The conventions documented here are a starting point, not
> a fixed specification.  If you see a better way to structure something —
> a cleaner API, a smarter split of concerns, a more idiomatic Rust pattern —
> proposals and pull requests are welcome.  Good design evolves through
> discussion.

> **Migration status.**
> | Widget | Status |
> |---|---|
> | `TextInput` | ✅ tui-realm `MockComponent` |
> | `MultiChoice` | 🔄 pending migration |
> | `Form` | 🔄 pending migration |
> | `TwoColumnLayout` | 🔄 pending migration |

---

## Table of Contents

1. [User Guide](#1-user-guide)
   1. [TextInput](#11-textinput)
   2. [MultiChoice](#12-multichoice)
   3. [Form](#13-form)
   4. [TwoColumnLayout](#14-twocolumnlayout)
   5. [Utilities](#15-utilities)
2. [Developer Guide](#2-developer-guide)
   1. [Project layout](#21-project-layout)
   2. [Design principles](#22-design-principles)
   3. [Anatomy of a widget](#23-anatomy-of-a-widget)
   4. [Styling system](#24-styling-system)
   5. [Adding a new widget — checklist](#25-adding-a-new-widget--checklist)
   6. [Common pitfalls](#26-common-pitfalls)

---

## 1. User Guide

Public types re-exported from the crate root:
```rust
use not_yet_done_ratatui::{
    // tui-realm components
    TextInput, TextInputEvent, TextInputKeymap,
    TextInputStyle, TextInputStyleType, ATTR_ERROR,

    // pending migration
    MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceState,
        MultiChoiceStyle, MultiChoiceStyleType,
    Form, FormState, FormField, FormFieldState, FormStyle, FormWidgetStyle,
        FormKeymap, FormEvent, FieldEvent,
    TwoColumnLayout, TwoColumnStyle, ColumnWidth, BorderStyleType,
        HalfBorders, HalfPadding,

    hex_color,
};
```

---

### 1.1 TextInput

A single-line text field.
```
▍ Title
▍ value or placeholder text
  ⚠ error message (only when an error is set)
```

`TextInput` implements:
- `tuirealm::MockComponent` — low-level `view` / `perform` / `attr` / `query` / `state`
- `tuirealm::Component<TextInputEvent, NoUserEvent>` — maps keyboard events to messages

The component owns all its state.  No external state struct is needed.

#### Construction

Create once, mount into a tuirealm `Application`.  Do not rebuild per frame.
```rust
let input = TextInput::default()
    .with_title("Username")
    .with_placeholder("e.g. alice")
    .with_inactive_style(inactive_style)
    .with_active_style(active_style)
    .with_keymap(keymap);          // optional — defaults to TextInputKeymap::default()

app.mount(Id::Username, Box::new(input), vec![])?;
```

#### Focus and style selection

The `Application` manages focus.  When it sets focus on this component,
`attr(Attribute::Focus, AttrValue::Flag(true))` is called internally.
The component then renders with `active_style` and shows the terminal cursor on
the input row.  `inactive_style` is used otherwise.

#### Reading the value
```rust
if let Ok(State::One(StateValue::String(val))) = app.state(&Id::Username) {
    println!("current value: {}", val);
}
```

#### Setting and clearing errors
```rust
// Set an error:
app.attr(
    &Id::Username,
    Attribute::Custom(ATTR_ERROR),
    AttrValue::String("At least 3 characters required".into()),
)?;

// Clear the error:
app.attr(&Id::Username, Attribute::Custom(ATTR_ERROR), AttrValue::Flag(false))?;
```

#### Keymap
```rust
use tuirealm::event::{Key, KeyEvent, KeyModifiers};

// Default bindings
let keymap = TextInputKeymap::default();
// ←            move cursor left
// →            move cursor right
// Backspace    delete backwards   (→ Cmd::Delete)
// Delete       delete forwards    (→ Cmd::Custom("delete_fwd"))
// Ctrl+U       clear all         (→ Cmd::Custom("clear"))

// Emacs-style override via struct update syntax:
let keymap = TextInputKeymap {
    move_left:  KeyEvent { code: Key::Char('b'), modifiers: KeyModifiers::CONTROL },
    move_right: KeyEvent { code: Key::Char('f'), modifiers: KeyModifiers::CONTROL },
    ..TextInputKeymap::default()
};
```

#### Events
```rust
// In your Application::update():
match msg {
    Some(TextInputEvent::Changed(new_value)) => {
        // live validation, store value, etc.
    }
}
```

#### `perform` command reference

| `Cmd` | Effect |
|---|---|
| `Cmd::Move(Direction::Left)` | Move cursor left |
| `Cmd::Move(Direction::Right)` | Move cursor right |
| `Cmd::Delete` | Delete character before cursor (Backspace) |
| `Cmd::Custom("delete_fwd")` | Delete character after cursor (Delete key) |
| `Cmd::Custom("clear")` | Clear entire input |
| `Cmd::Type(char)` | Insert character at cursor |

Commands that change the value return
`CmdResult::Changed(State::One(StateValue::String(new_value)))`.
Cursor-only movement returns `CmdResult::None`.

#### Style

Two independent `TextInputStyle` instances control appearance depending on
focus state:
```rust
let inactive = TextInputStyle::new()
    .prefix_color(Color::Rgb(60, 60, 120))
    .set_style(TextInputStyleType::Title, Style::default().fg(BLUE))
    .set_style(TextInputStyleType::Input, Style::default().fg(WHITE))
    .set_style(TextInputStyleType::Error, Style::default().fg(RED))
    .placeholder_color(Color::Rgb(60, 60, 100));

let active = TextInputStyle::new()
    .prefix_color(Color::Rgb(100, 180, 255))
    .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT).bold())
    .set_style(TextInputStyleType::Input, Style::default().fg(WHITE).bg(FIELD_BG))
    .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
    .placeholder_color(Color::Rgb(80, 80, 110));

TextInput::default()
    .with_inactive_style(inactive)
    .with_active_style(active)
```

| `TextInputStyleType` | Affects |
|---|---|
| `Title` | Title row |
| `Input` | Input row (text and background) |
| `Error` | Error row |

Unset slots fall back to `Style::default()`.

---

### 1.2 MultiChoice

> **Pending tui-realm migration.**  Still uses the original stateless ratatui
> approach; API will change when migrated.

A dropdown-style multi-select widget.

**Collapsed:**
```
▍ Protocol
▍ HTTP, HTTPS
```

**Expanded** (`state.open == true`):
```
▍ Protocol
▍▶ HTTP
▍  HTTPS
▍  WS
              ← blank closing line
```

See the previous API documentation for full usage until migration is complete.

---

### 1.3 Form

> **Pending tui-realm migration.**

See the previous API documentation for full usage until migration is complete.

---

### 1.4 TwoColumnLayout

> **Pending tui-realm migration.**

See the previous API documentation for full usage until migration is complete.

---

### 1.5 Utilities

#### `hex_color`

Parses a CSS hex string into `ratatui::style::Color::Rgb`:
```rust
let blue = hex_color("#64B4FF");
```

#### `open_editor`

Suspends the TUI, opens `$EDITOR` for the given path, then resumes:
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
    │   ├── render.rs        — render_prefixed_line, truncate_to_width
    │   └── style.rs         — hex_color, impl_widget_style_base! macro
    ├── text_input/          — tui-realm MockComponent ✅
    │   ├── mod.rs           — TextInput struct + builder
    │   ├── keymap.rs        — TextInputKeymap (tuirealm KeyEvent fields)
    │   ├── state.rs         — TextInputEvent
    │   ├── style.rs         — TextInputStyle, TextInputStyleType
    │   ├── render.rs        — TextInputViewData + render() free function
    │   └── component.rs     — impl MockComponent + impl Component
    ├── multi_choice/        — stateless ratatui (pending migration)
    ├── two_column/          — stateless ratatui (pending migration)
    └── form/                — stateless ratatui (pending migration)
```

`common/` is the only shared dependency.  Widgets must not import each other
directly; the form layer is the only place that knows about multiple widget
types.

---

### 2.2 Design principles

**tui-realm component model.**
Each widget implements `MockComponent` (low-level) and optionally
`Component<Msg, UserEvent>` (high-level event-to-message mapping).  The
component owns all its mutable state; the `Application` manages focus via
`attr(Attribute::Focus, …)`.

**Focus-driven style selection.**
Each component holds two style instances — `inactive_style` and
`active_style`.  `view` selects between them based on `self.focused`, which
is kept in sync by the framework.

**Render logic as a free function.**
The actual drawing code lives in `render.rs` as a free function accepting a
plain data struct (`*ViewData`).  This separates the render logic from the
framework plumbing in `component.rs`, making it independently testable.

**`perform` is the single mutation point.**
All state changes go through `perform(Cmd) → CmdResult`.  `on` (and any
direct callers) map inputs to `Cmd` values and delegate — never mutating
`self` directly.  This keeps unit tests of `perform` reliable.

**Keymaps use tuirealm types directly.**
Keymap fields are `tuirealm::event::KeyEvent` — no custom wrapper type is
needed.  Comparison in `on` is a plain `==` check.

**Events, not callbacks.**
`Component::on` returns `Option<Msg>` rather than calling closures.

**Unicode-correct rendering.**
All text is measured and truncated using display width (`unicode-width`),
not byte or character count.

**English only.**
All code comments, doc comments, error messages, placeholder text, and
example strings must be in English.

**Design is not final.**
If you see a better approach — cleaner API, smarter module boundary, more
idiomatic pattern — open a discussion.  These conventions exist to provide
consistency, not to prevent improvement.

---

### 2.3 Anatomy of a tui-realm widget

A complete widget spans five files:
```
my_widget/
├── mod.rs           — struct definition + builder methods + pub re-exports
├── keymap.rs        — MyWidgetKeymap with tuirealm KeyEvent fields
├── state.rs         — MyWidgetEvent (PartialEq + Eq required)
├── style.rs         — MyWidgetStyle, MyWidgetStyleType
├── render.rs        — MyWidgetViewData + render() free function (pub(super))
└── component.rs     — impl MockComponent + impl Component (pub(super) mod)
```

#### `keymap.rs`
```rust
use tuirealm::event::{Key, KeyEvent, KeyModifiers};

#[derive(Debug, Clone)]
pub struct MyWidgetKeymap {
    pub confirm: KeyEvent,
    pub cancel:  KeyEvent,
}

impl Default for MyWidgetKeymap {
    fn default() -> Self {
        Self {
            confirm: KeyEvent { code: Key::Enter, modifiers: KeyModifiers::NONE },
            cancel:  KeyEvent { code: Key::Esc,   modifiers: KeyModifiers::NONE },
        }
    }
}
```

#### `state.rs`

Only the event enum — no external state struct.
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MyWidgetEvent {
    Confirmed(String),
    Cancelled,
}
```

#### `style.rs`

Unchanged from the original design — `repr(u8)` enum + style struct +
`impl_widget_style_base!`.

#### `render.rs`
```rust
pub(super) struct MyWidgetViewData<'a> {
    pub title:   &'a str,
    pub value:   &'a str,
    pub focused: bool,
    pub style:   &'a MyWidgetStyle,
}

pub(super) fn render(frame: &mut Frame, area: Rect, data: &MyWidgetViewData<'_>) {
    // draw rows using render_prefixed_line from common::render
    if data.focused {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}
```

#### `component.rs`
```rust
use super::{MyWidget, render::{MyWidgetViewData, render}, state::MyWidgetEvent};

impl MockComponent for MyWidget {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused { &self.active_style } else { &self.inactive_style };
        render(frame, area, &MyWidgetViewData { style, focused: self.focused, /* … */ });
    }
    fn state(&self) -> State { State::One(StateValue::String(self.value.clone())) }
    fn perform(&mut self, cmd: Cmd) -> CmdResult { /* mutations here */ }
    fn query(&self, attr: Attribute) -> Option<AttrValue> { /* … */ }
    fn attr(&mut self, attr: Attribute, value: AttrValue) { /* … */ }
}

impl tuirealm::Component<MyWidgetEvent, NoUserEvent> for MyWidget {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<MyWidgetEvent> {
        let Event::Keyboard(key_ev) = ev else { return None; };
        let cmd = /* map key_ev against keymap fields */;
        match self.perform(cmd) {
            CmdResult::Changed(State::One(StateValue::String(s))) => {
                Some(MyWidgetEvent::Confirmed(s))
            }
            _ => None,
        }
    }
}
```

#### `mod.rs`
```rust
mod component;
mod render;
pub mod keymap;
pub mod state;
pub mod style;

pub use component::ATTR_MY_WIDGET;  // if needed
pub use keymap::MyWidgetKeymap;
pub use state::MyWidgetEvent;
pub use style::{MyWidgetStyle, MyWidgetStyleType};

pub struct MyWidget {
    pub(crate) focused:         bool,
    pub(crate) value:           String,
    pub(crate) title:           String,
    pub(crate) inactive_style:  MyWidgetStyle,
    pub(crate) active_style:    MyWidgetStyle,
    pub(crate) keymap:          MyWidgetKeymap,
}

impl Default for MyWidget { /* … */ }
impl MyWidget {
    pub fn with_title(mut self, t: impl Into<String>) -> Self { /* … */ }
    // … other builder methods …
    // internal helpers used by component.rs:
    pub(crate) fn some_mutation(&mut self) { /* … */ }
}
```

---

### 2.4 Styling system

#### Option-based slots

Every style slot is `Option<Style>` (or `Option<Color>` for colour-only
fields).  `None` means "not configured".
```
resolved_style(SlotType)  → Style   — falls back to Style::default()
style(SlotType)           → Option<&Style>  — None if not set (for merge logic)
```

Render code always calls `resolved_style`.

#### `impl_widget_style_base!` macro

Any style struct with these two fields:
```rust
pub prefix_color: Option<Color>,
pub styles: [Option<Style>; N],
```
derives the standard accessors with:
```rust
impl_widget_style_base!(MyWidgetStyle, MyWidgetStyleType);
```
Generated: `prefix_color()`, `set_style()`, `style()`, `resolved_style()`.

#### Per-component precedence

Each component has two style instances.  Within each instance:
```
set_style(...)    ← highest (Some wins)
      ↓
Style::default()  ← fallback
```

The selection between the two instances is based on `self.focused`.

---

### 2.5 Adding a new widget — checklist

- [ ] Create `src/widgets/my_widget/` with all five files
- [ ] `StyleType` enum: `#[repr(u8)]`, variants start at 0
- [ ] Style struct: `pub prefix_color: Option<Color>`, `pub styles: [Option<Style>; N]`,
      `Default` initialises everything to `None`, call `impl_widget_style_base!`
- [ ] `MyWidgetEvent` derives `PartialEq` and `Eq`
- [ ] Widget struct fields: `pub(crate)`, two style instances, keymap field
- [ ] `render.rs`: free function, takes `*ViewData` with `&style` already selected
- [ ] `component.rs`: all mutations go through `perform`; `on` only maps events to `Cmd`
- [ ] `MockComponent::state` returns the primary value as `State::One(StateValue::…)`
- [ ] Cursor set inside `render()` via `frame.set_cursor_position()` when `data.focused`
- [ ] Re-export from `src/widgets/mod.rs` and `src/lib.rs`
- [ ] Add an example under `examples/`

---

### 2.6 Common pitfalls

**`px` advance must use display width.**
CJK characters are 2 columns wide.  Always use `ch.width().unwrap_or(1) as u16`,
never `+= 1`.

**`truncate_to_width` returns a `String`.**
Do not cache across frames; measure and truncate fresh each render.

**`resolved_style` in render code, `style` in merge logic.**
Render code calls `resolved_style()` (always returns a `Style`).
`style()` returns `Option<&Style>` and is only meaningful where `None` has
semantic value (e.g. future form-level merge logic).

**`perform` is the single mutation point.**
Never mutate component state directly in `on` — always delegate to `perform`.
This keeps unit tests of `perform` reliable.

**`Cmd::Custom` keys are `&'static str`.**
Use named constants for custom command strings to avoid silent typo mismatches:
```rust
const CMD_DELETE_FWD: &str = "delete_fwd";
// Cmd::Custom(CMD_DELETE_FWD)
```

**`block_on` inside a Tokio runtime panics.**
Delegate async work through channels; never call `block_on` from within a
tui-realm event-handling or render call stack running inside a Tokio runtime.
