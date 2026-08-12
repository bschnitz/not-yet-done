# vimrealm

A modal, vim-like multi-line text editor widget for [ratatui](https://ratatui.rs), ready to mount as a [tuirealm](https://github.com/veeso/tui-realm) component.

It is meant for the places a TUI would otherwise shell out to `$EDITOR`: chat messages, commit messages, ticket bodies, comments. The widget knows nothing about files — `:w` reports an event and the host decides what saving means.

## Quick start

```rust
use tuirealm::event::Key;
use vimrealm::{VimEditor, VimEvent};

let mut editor = VimEditor::default()
    .with_text("hello world")
    .with_title(" message ")
    .with_line_numbers(true);

// Feed keys; the editor reports only what matters to the host.
editor.on_key(Key::Char('d').into());
editor.on_key(Key::Char('w').into());
assert_eq!(editor.text(), "world");
```

Mounted in a tuirealm application it implements `Component` and `AppComponent<VimEvent, NoUserEvent>`, so `on()` returns a [`VimEvent`] per key press:

| Event          | Raised by         | Meaning                           |
| -------------- | ----------------- | --------------------------------- |
| `Changed`      | any edit          | the buffer text changed           |
| `Save`         | `:w`              | persist, keep editing             |
| `SaveAndClose` | `:wq`, `:x`       | persist and close                 |
| `Cancel`       | `:q` clean, `:q!` | close, discarding unsaved changes |

To see it in a terminal:

```sh
cargo run -p vimrealm --example vim_demo
```

## Keys

| Mode    | Keys                           | Effect                                                   |
| ------- | ------------------------------ | -------------------------------------------------------- |
| Normal  | `h j k l`, arrows              | move by character and line                               |
| Normal  | `w b e`                        | move by word                                             |
| Normal  | `0 ^ $`, `Home`/`End`          | line start, first non-blank, line end                    |
| Normal  | `gg G`                         | first/last line; with a count an absolute line (`5G`)    |
| Normal  | `{count}`                      | prefix for any of the above, and for operators (`2d3w`)  |
| Normal  | `d c y` + motion               | delete, change, yank; `dd cc yy` for whole lines         |
| Normal  | `x D C`                        | delete character, delete to line end, change to line end |
| Normal  | `p P`                          | paste after/before, char- or linewise as yanked          |
| Normal  | `"x` prefix                    | pick a register: `"ayy`, `"ap`; `"A` appends, `"_` drops |
| Normal  | `u`, `Ctrl+R`                  | undo, redo                                               |
| Normal  | `.`                            | repeat the last change                                   |
| Normal  | `i a I A o O`                  | enter insert mode                                        |
| Normal  | `v V`                          | start a charwise / linewise selection                    |
| Normal  | `:`                            | enter command mode                                       |
| Normal  | `/ ?`, `n N`                   | search forward/backward, repeat the search               |
| Visual  | `d x`, `c s`, `y`              | operate on the selection and return to normal mode       |
| Visual  | `o`                            | swap which end of the selection the cursor is on         |
| Visual  | motions, `v V`, `Esc`          | grow the selection, switch its kind, cancel it           |
| Both    | `i`/`a` + object               | text object: `w W` words, `" '` quotes, `( [ {` blocks   |
| Insert  | `Esc`                          | back to normal mode                                      |
| Insert  | arrows, `Home`/`End`           | move; may stand one column past the last character       |
| Insert  | `Enter`, `Backspace`, `Delete` | split the line, join it again, delete forward            |
| Command | `:w :wq :x :q :q!`             | save, save and close, close                              |

Text objects work after an operator and inside a selection alike: `ciw`
changes the word, `da"` deletes a quoted string with its quotes, `vi(`
selects what a bracket pair encloses (`i` = inner, `a` = around).

Every binding is a table entry, so a host can override or drop one:

```rust
use tuirealm::event::Key;
use vimrealm::{Keymap, VimEditor};

let mut keymap = Keymap::vim();
keymap.unbind(Key::Char(':').into()); // host owns the colon
let editor = VimEditor::default().with_keymap(keymap);
```

Counts, the register prefix `"`, the `g` of `gg` and the `i`/`a` of a text object are grammar rather than bindings and stay out of the table, so they cannot be shadowed by accident. Motions, on the other hand, come from the table even in visual mode — rebind `j` and the selection grows by whatever you bound it to. Only the keys that mean nothing without a selection (`d c y o v V`) are fixed.

Where a motion may stop is the caller's business, not the motion's: normal mode parks the cursor _on_ a character, so `l` and `$` stop at the last one, while insert mode sits _between_ characters and may stand behind the last — that is `motion::Bound`, and only the two motions aiming at the line end read it.

An insert session is a single undo step: `u` after typing a sentence removes the sentence, not its last character — the same grouping vim uses.

`.` records the keys of the last change and replays them, so `ciwfoo<Esc>`, `2dw` and a visual `d` all repeat through one mechanism. A count repeats the replay (`3.` applies the change three times) rather than replacing the recorded count as vim does.

Search is a literal substring match, not a regex: it keeps the crate dependency-free, and for the messages and ticket bodies this is built for a substring is what one types anyway. Searching wraps around the buffer and reports it the way vim does, including `E486` when there is no match.

## Styling

Nothing hardcodes a colour. `VimStyle` has one optional slot per visual part (text, cursor, selection, gutter, mode indicator, status line, command line); unset slots fall back to modifiers only (`REVERSED` for the cursor, `UNDERLINED` for a selection, `DIM` for the gutter), so the widget inherits the surrounding palette until a host says otherwise.

There are two cursors, and only one of them is styled. Normal and visual mode act on the character _under_ the cursor, so it is a block painted over that character — that is the `Cursor` slot, and it is drawn on top of a selection, which is why the selection's own fallback is an underline rather than a reverse: two reversed styles on one cell would hide it. Insert mode inserts _between_ characters and command mode does not point into the buffer at all; both place the terminal's own cursor instead (`Frame::set_cursor_position`), so they get the shape and blink the user's terminal is configured for.

## Layers

Each layer is testable without the one above it:

`buffer` → `motion` → `operator` → `keymap` → `editor` → `render` → `component`

- **`buffer`** — text storage, cursor arithmetic, snapshot undo. The only module that knows how the text is stored; positions are `(line, byte offset)` and everything above goes through its methods, so swapping in a rope stays local.
- **`motion`** — where the cursor goes, plus each motion's exclusive/inclusive/linewise kind.
- **`textobject`** — the spans `iw`, `a"`, `i(` resolve to; an object looks like a motion result, so operators need no separate path for it.
- **`operator`** — what happens to the span; because the kind travels with the motion, one code path serves `dw`, `de` and `dj` — and a visual selection, which is the same inclusive span typed by hand.
- **`register`** — the unnamed register plus the named ones, and the rule that every write also lands in the unnamed one.
- **`search`** — the substring search behind `/`, `?`, `n` and `N`.
- **`keymap`** — key → command tables for normal and insert mode.
- **`editor`** — modes, pending `["x]{count}{operator}{count}{motion|textobject}` input, the key state machine, one submodule per mode. Framework-free.
- **`render`** — soft wrap, viewport scrolling, gutter, status line. The only module that knows about display rows, which is why `j`/`k` stay logical.
- **`component`** — the tuirealm impls. Keys are passed through raw: a keymap that pre-translated `d` into a `Cmd` would lose the pending operator.

## Hosting it as a pane

The widget is a pane like any other, not an overlay: give it rows in your
layout and call `view`. In this workspace that is
`not-yet-done-tui/src/components/builtin_editor.rs` — worth reading as a
worked example. It shows the three things a host has to decide:

- **Who owns the keyboard.** While the pane is open it must see every key
  ahead of the application's own bindings, including a global quit binding —
  otherwise a `q` typed into the buffer closes the app. Leaving is `:q!`.
- **What `:w` means.** `Save` and `SaveAndClose` are the host's two persist
  hooks; the widget itself never touches a file.
- **Where the colours come from.** Fill the `VimStyle` slots from the host
  theme, so the pane matches the surrounding UI.

## Scope

Present: normal, insert, command and both visual modes; the motions, operators
and text objects listed above; counts; named registers; substring search;
dot-repeat; snapshot undo; soft wrap; ex commands.

Not yet: marks, macros, block-visual mode, `f`/`t` character motions, regex
search, `:s` substitution.

## License

MIT OR Apache-2.0
