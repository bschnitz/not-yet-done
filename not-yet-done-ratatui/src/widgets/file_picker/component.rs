use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, PropPayload, PropValue, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::widgets::common::render::{PREFIX_LEN, render_prefixed_line};
use crate::widgets::common::types::{FilterMode, SelectionMarker, SelectionMode};
use crate::widgets::select_list::{ATTR_SELECTED, SelectList, SelectListEvent, SelectListKeymap};
use crate::widgets::text_input::{TextInput, TextInputEvent};

use super::{
    enumerator::{self, EnumerationOptions},
    keymap::FilePickerKeymap,
    state::FilePickerEvent,
    style::FilePickerStyle,
};

/// Which slot inside the [`FilePicker`] currently owns keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePickerFocus {
    Directory,
    Glob,
    Files,
    Selected,
}

impl FilePickerFocus {
    fn next(self) -> Self {
        match self {
            Self::Directory => Self::Glob,
            Self::Glob => Self::Files,
            Self::Files => Self::Selected,
            Self::Selected => Self::Directory,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Directory => Self::Selected,
            Self::Glob => Self::Directory,
            Self::Files => Self::Glob,
            Self::Selected => Self::Files,
        }
    }
}

impl Default for FilePickerFocus {
    fn default() -> Self {
        Self::Files
    }
}

/// Composite picker. Two independent dropdown-style pickers sit under
/// their associated text inputs:
///
/// - `dir_picker` (Browse Directory) is its own focusable component; its
///   body expands only while it has focus. Pressing Enter on a row
///   updates the Directory text input.
/// - `files` (under Glob) shows files matching the glob and expands
///   while Glob or Files has focus.
///
/// The `selected` SelectList is always visible at the bottom and
/// survives directory changes — this is the source of truth for what
/// [`Self::confirmed_paths`] returns on submit.
pub struct FilePicker {
    pub(crate) focused: bool,
    pub(crate) focus_slot: FilePickerFocus,

    pub(crate) directory: TextInput,
    pub(crate) glob: TextInput,
    pub(crate) dir_picker: SelectList,
    pub(crate) files: SelectList,
    pub(crate) selected: SelectList,

    /// Absolute paths backing `dir_picker` items, index-aligned.
    /// First entry is the parent directory (`..`); remainder are subdirs.
    pub(crate) dir_picker_paths: Vec<PathBuf>,
    /// Absolute paths backing the Files SelectList items, index-aligned.
    pub(crate) files_paths: Vec<PathBuf>,
    /// Absolute paths chosen by the user. Survives directory changes.
    pub(crate) selected_paths: Vec<PathBuf>,

    pub(crate) enum_opts: EnumerationOptions,

    /// `true` when the expanded Directory value points at an existing
    /// directory (or when the Directory field is empty — empty is treated
    /// as not-yet-typed, not as an error).
    pub(crate) directory_valid: bool,

    /// Optional callback that supplies clipboard contents on demand.
    /// Kept here as a callback so `arboard` (or any other clipboard impl)
    /// stays out of this crate. Invoked when the user presses
    /// [`FilePickerKeymap::paste`] — the keypress is treated as a global
    /// "paste a path" command, independent of the currently focused slot.
    pub(crate) paste_provider: Option<Box<dyn Fn() -> Option<String>>>,

    /// Transient error banner. Set by [`paste_clipboard_path`] when the
    /// clipboard content can't be resolved as a directory or file path.
    /// Cleared automatically on the next key event so it disappears as
    /// soon as the user interacts with the picker again.
    pub(crate) paste_error: Option<String>,

    /// Title rendered above the Files SelectList.
    pub(crate) files_title: String,
    /// Title rendered above the Selected SelectList.
    pub(crate) selected_title: String,

    /// Optional panel title rendered at the top of the picker area. When
    /// set, [`Self::view`] also draws a wrapping help bar at the bottom
    /// and pads with blank gutter rows above + below.
    pub(crate) title: Option<String>,

    pub(crate) keymap: FilePickerKeymap,
    pub(crate) style: FilePickerStyle,
}

impl Default for FilePicker {
    fn default() -> Self {
        let picker_keymap = FilePickerKeymap::default();
        let list_keymap = SelectListKeymap {
            toggle: picker_keymap.toggle.clone(),
            ..SelectListKeymap::default()
        };

        let directory = TextInput::default().with_title("Directory");
        let glob = TextInput::default().with_title("Glob");

        // Directory dropdown: bare items, no embedded filter or footer
        // — the Directory input itself is the filter source (we filter
        // by basename prefix in `populate_dir_picker`).
        let dir_picker = SelectList::default()
            .with_marker(SelectionMarker::None)
            .with_mode(SelectionMode::Single)
            .with_show_filter(false)
            .with_show_footer(false)
            .with_keymap(list_keymap.clone());

        let files = SelectList::default()
            .with_marker(SelectionMarker::Checkbox)
            .with_mode(SelectionMode::Multi)
            .with_show_filter(true)
            .with_filter_mode(FilterMode::Fuzzy)
            .with_show_footer(true)
            .with_keymap(list_keymap.clone());

        let selected = SelectList::default()
            .with_marker(SelectionMarker::None)
            .with_mode(SelectionMode::Single)
            .with_show_filter(true)
            .with_filter_mode(FilterMode::Fuzzy)
            .with_show_footer(true)
            .with_keymap(list_keymap);

        let mut picker = Self {
            focused: false,
            focus_slot: FilePickerFocus::default(),
            directory,
            glob,
            dir_picker,
            files,
            selected,
            dir_picker_paths: Vec::new(),
            files_paths: Vec::new(),
            selected_paths: Vec::new(),
            enum_opts: EnumerationOptions::default(),
            directory_valid: true,
            paste_provider: None,
            paste_error: None,
            files_title: "Files in Directory".to_string(),
            selected_title: "Selected Files".to_string(),
            title: None,
            keymap: picker_keymap,
            style: FilePickerStyle::default(),
        };
        picker.sync_subfocus();
        picker
    }
}

impl FilePicker {
    pub fn with_initial_directory(mut self, dir: impl Into<String>) -> Self {
        // Normalize: trailing slash means "browse children of this dir".
        // Without it the dropdown would treat the basename as a prefix
        // filter against the parent, which is never what an initial-dir
        // setup wants.
        let mut s: String = dir.into();
        if !s.is_empty() && !s.ends_with('/') {
            s.push('/');
        }
        self.directory.attr(Attribute::Value, AttrValue::String(s));
        self.reload();
        self
    }

    pub fn with_initial_glob(mut self, glob: impl Into<String>) -> Self {
        self.glob
            .attr(Attribute::Value, AttrValue::String(glob.into()));
        self.reload();
        self
    }

    pub fn with_keymap(mut self, keymap: FilePickerKeymap) -> Self {
        // Propagate the `toggle` binding into the inner SelectLists so
        // Enter / Ctrl+Enter (or whatever the user reconfigures) fires
        // their internal toggle command. Other SelectList keys stay at
        // their respective list defaults — see SelectListKeymap if you
        // need to override those individually.
        self.dir_picker.keymap.toggle = keymap.toggle.clone();
        self.files.keymap.toggle = keymap.toggle.clone();
        self.selected.keymap.toggle = keymap.toggle.clone();
        self.keymap = keymap;
        self
    }

    /// Apply a [`FilePickerStyle`]. Each populated field is forwarded to
    /// the matching sub-component's `inactive_style` / `active_style`
    /// slot. Fields left as `None` keep the sub-component's own default.
    pub fn with_style(mut self, style: FilePickerStyle) -> Self {
        if let Some(s) = style.text_input_inactive.clone() {
            self.directory.inactive_style = s.clone();
            self.glob.inactive_style = s;
        }
        if let Some(s) = style.text_input_active.clone() {
            self.directory.active_style = s.clone();
            self.glob.active_style = s;
        }
        if let Some(s) = style.select_list_inactive.clone() {
            self.dir_picker.inactive_style = s.clone();
            self.files.inactive_style = s.clone();
            self.selected.inactive_style = s;
        }
        if let Some(s) = style.select_list_active.clone() {
            self.dir_picker.active_style = s.clone();
            self.files.active_style = s.clone();
            self.selected.active_style = s;
        }
        self.style = style;
        self
    }

    pub fn with_enumeration_options(mut self, opts: EnumerationOptions) -> Self {
        self.enum_opts = opts;
        self.reload();
        self
    }

    /// Install a clipboard-text provider. Called on every paste
    /// keypress; return `Some(text)` to paste, `None` to no-op.
    pub fn with_paste_provider<F>(mut self, f: F) -> Self
    where
        F: Fn() -> Option<String> + 'static,
    {
        self.paste_provider = Some(Box::new(f));
        self
    }

    /// Override the title rendered above the Files pane (default:
    /// `"Files in Directory"`).
    pub fn with_files_title(mut self, title: impl Into<String>) -> Self {
        self.files_title = title.into();
        self
    }

    /// Override the title rendered above the Selected pane (default:
    /// `"Selected Files"`).
    pub fn with_selected_title(mut self, title: impl Into<String>) -> Self {
        self.selected_title = title.into();
        self
    }

    /// Enable the panel chrome with the given title. When set, the
    /// picker fills its full area with [`FilePickerStyle::panel_bg`],
    /// renders the title on the top row, and wraps a help bar across the
    /// bottom rows. Both rows are flanked by blank gutter lines. Without
    /// this, [`Self::view`] renders only the picker proper.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Treat the clipboard content as a path and apply it to the picker:
    ///
    /// - **Directory** → replace the Directory input with `path` (calls
    ///   [`set_directory`] which appends a trailing `/` and reloads).
    /// - **File** → set the Directory input to the file's parent and add
    ///   the file to `selected_paths`, mirroring how a Files-pane toggle
    ///   would behave.
    /// - **Unresolved** → store an error message in `paste_error` so the
    ///   next render shows a transient banner.
    ///
    /// Multi-line clipboard content uses the first non-empty trimmed
    /// line. Tilde (`~`, `~/foo`) is expanded against `$HOME`. The slot
    /// that currently has focus is irrelevant — paste is a global action
    /// inside the dialog.
    fn paste_clipboard_path(&mut self, raw: &str) {
        let Some(line) = raw.lines().map(str::trim).find(|l| !l.is_empty()) else {
            self.paste_error = Some("Clipboard is empty".to_string());
            return;
        };
        let expanded = expand_tilde(line);
        let path = PathBuf::from(&expanded);

        if path.is_dir() {
            self.set_directory(&path);
            return;
        }
        if path.is_file() {
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            };
            self.set_directory(&parent);
            if !self.selected_paths.contains(&path) {
                self.selected_paths.push(path.clone());
            }
            self.sync_files_marks_from_selected();
            self.refresh_selected_pane();
            return;
        }
        self.paste_error = Some(format!("Not a valid path: {expanded}"));
    }

    pub fn focus_slot(&self) -> FilePickerFocus {
        self.focus_slot
    }

    /// Read-only access to the picker's keymap so embedders can build
    /// a help bar (or any other UI) that always mirrors the currently
    /// active bindings rather than hardcoding them.
    pub fn keymap(&self) -> &FilePickerKeymap {
        &self.keymap
    }

    pub fn set_focus_slot(&mut self, slot: FilePickerFocus) {
        self.focus_slot = slot;
        self.sync_subfocus();
    }

    /// Paths the user has selected. Returned on [`FilePickerEvent::Confirmed`].
    pub fn confirmed_paths(&self) -> Vec<PathBuf> {
        self.selected_paths.clone()
    }

    /// Refill both pickers from the current Directory + Glob values.
    /// - `dir_picker` reflects subdirs of the input's _parent_ directory
    ///   filtered by the basename prefix typed after the last `/`. So
    ///   `/foo/bar/hall` shows subdirs of `/foo/bar/` whose name starts
    ///   with `hall`. An input ending in `/` shows all subdirs of that
    ///   directory unfiltered.
    /// - `files` reflects glob enumeration rooted at the input itself
    ///   when it's a valid directory, falling back to the parent
    ///   otherwise.
    pub fn reload(&mut self) {
        let raw = self.directory_value();
        let expanded = expand_tilde(&raw);
        let dir_path: PathBuf = expanded.clone().into();
        self.directory_valid = raw.is_empty() || dir_path.is_dir();
        self.refresh_directory_title();

        let (parent, prefix) = parse_dir_input(&expanded);
        self.populate_dir_picker(&parent, &prefix);

        let files_base: PathBuf = if dir_path.is_dir() { dir_path } else { parent };
        self.populate_files(&files_base);
    }

    /// `true` when the typed Directory value (after tilde expansion)
    /// points at an existing directory, or the field is empty.
    pub fn is_directory_valid(&self) -> bool {
        self.directory_valid
    }

    fn refresh_directory_title(&mut self) {
        let title = if self.directory_valid {
            "Directory"
        } else {
            "Directory (not found)"
        };
        self.directory.title = title.to_string();
    }

    fn populate_files(&mut self, dir_path: &Path) {
        let glob = self.glob_value();
        self.files_paths = enumerator::enumerate(dir_path, &glob, &self.enum_opts);

        let labels: Vec<String> = self
            .files_paths
            .iter()
            .map(|p| label_relative_to(p, dir_path))
            .collect();
        self.files.set_items(labels);
        self.sync_files_marks_from_selected();
    }

    /// Populate `dir_picker` with the subdirectories of `parent`. When
    /// `prefix` is empty the list is alphabetical; otherwise entries
    /// are fuzzy-matched against `prefix` and sorted by descending
    /// score (Skim semantics — same matcher SelectList uses internally),
    /// with the name as a tiebreaker. Files are excluded.
    fn populate_dir_picker(&mut self, parent: &Path, prefix: &str) {
        use fuzzy_matcher::FuzzyMatcher;
        use fuzzy_matcher::skim::SkimMatcherV2;

        let mut entries: Vec<PathBuf> = Vec::new();
        let mut labels: Vec<String> = Vec::new();

        if parent.is_dir() {
            if let Ok(read) = std::fs::read_dir(parent) {
                let raw: Vec<(String, PathBuf)> = read
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        (name, e.path())
                    })
                    .collect();

                let ordered: Vec<(String, PathBuf)> = if prefix.is_empty() {
                    let mut v = raw;
                    v.sort_by(|a, b| a.0.cmp(&b.0));
                    v
                } else {
                    let matcher = SkimMatcherV2::default();
                    let mut scored: Vec<(i64, String, PathBuf)> = raw
                        .into_iter()
                        .filter_map(|(name, path)| {
                            matcher.fuzzy_match(&name, prefix).map(|s| (s, name, path))
                        })
                        .collect();
                    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                    scored.into_iter().map(|(_, n, p)| (n, p)).collect()
                };

                for (label, path) in ordered {
                    entries.push(path);
                    labels.push(label);
                }
            }
        }
        self.dir_picker_paths = entries;
        self.dir_picker.set_items(labels);
    }

    /// Tab-completion: extend the Directory input to the longest common
    /// prefix of the dropdown entries whose name literally starts with
    /// the currently typed basename. Returns whether the input changed.
    ///
    /// Entries that match only fuzzily (e.g. `core` matching
    /// `not-yet-done-core`) are intentionally excluded from completion
    /// — there's no prefix to extend in that case.
    fn tab_complete_directory(&mut self) -> bool {
        let raw = self.directory_value();
        let (_, typed_prefix) = parse_dir_input(&raw);

        let names: Vec<String> = self
            .dir_picker_paths
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|n| n.starts_with(&typed_prefix))
            .collect();
        if names.is_empty() {
            return false;
        }

        let lcp = longest_common_prefix(&names);
        if lcp.len() <= typed_prefix.len() {
            return false;
        }

        // Replace the trailing prefix portion of the raw input with the
        // longer LCP. `parse_dir_input` ensures `typed_prefix` was the
        // suffix of `raw`, so a simple tail-swap is correct.
        let head_len = raw.len() - typed_prefix.len();
        let mut new_value = String::with_capacity(head_len + lcp.len());
        new_value.push_str(&raw[..head_len]);
        new_value.push_str(&lcp);
        self.directory
            .attr(Attribute::Value, AttrValue::String(new_value));
        self.reload();
        true
    }

    /// Resolve the path under the dir-picker cursor. Returns the
    /// absolute path of the highlighted entry, or `None` if empty.
    fn browse_target(&self) -> Option<PathBuf> {
        let cursor = self.dir_picker.cursor;
        let idx = *self.dir_picker.filtered_indices.get(cursor)?;
        self.dir_picker_paths.get(idx).cloned()
    }

    /// Remove the path under the focused list's cursor from
    /// `selected_paths`, then refresh both panes. Files-focus only acts
    /// when the cursor points at a currently-selected entry — pressing
    /// the remove key on an unselected file is a no-op so the user
    /// can't accidentally "unselect what wasn't selected".
    fn remove_at_cursor(&mut self) -> bool {
        match self.focus_slot {
            FilePickerFocus::Files => {
                let cursor = self.files.cursor;
                let Some(&real_idx) = self.files.filtered_indices.get(cursor) else {
                    return false;
                };
                let Some(path) = self.files_paths.get(real_idx).cloned() else {
                    return false;
                };
                let before = self.selected_paths.len();
                self.selected_paths.retain(|p| p != &path);
                if self.selected_paths.len() == before {
                    return false;
                }
                self.sync_files_marks_from_selected();
                self.refresh_selected_pane();
                true
            }
            FilePickerFocus::Selected => {
                let cursor = self.selected.cursor;
                let Some(&real_idx) = self.selected.filtered_indices.get(cursor) else {
                    return false;
                };
                if real_idx >= self.selected_paths.len() {
                    return false;
                }
                self.selected_paths.remove(real_idx);
                self.sync_files_marks_from_selected();
                self.refresh_selected_pane();
                true
            }
            _ => false,
        }
    }

    /// If the Directory input ends with `..` as the basename component
    /// (e.g. `/foo/bar/..`), return the parent of the current dir
    /// context (`/foo`). Used by the Enter handler so `..<Enter>` jumps
    /// up one level instead of trying to commit a (likely empty)
    /// dropdown selection.
    fn dotdot_target(&self) -> Option<PathBuf> {
        let raw = self.directory_value();
        let expanded = expand_tilde(&raw);
        let (parent, prefix) = parse_dir_input(&expanded);
        if prefix != ".." {
            return None;
        }
        parent.parent().map(|p| p.to_path_buf())
    }

    /// Replace the Directory input with `path` (appending a trailing
    /// `/` so the next autocomplete enumerates the new directory's
    /// children unfiltered) and refresh the dropdown + files.
    fn set_directory(&mut self, path: &Path) {
        let mut s = path.display().to_string();
        if !s.ends_with('/') {
            s.push('/');
        }
        self.directory.attr(Attribute::Value, AttrValue::String(s));
        self.reload();
    }

    fn directory_value(&self) -> String {
        match self
            .directory
            .query(Attribute::Value)
            .map(|q| q.into_attr())
        {
            Some(AttrValue::String(s)) => s,
            _ => String::new(),
        }
    }

    fn glob_value(&self) -> String {
        match self.glob.query(Attribute::Value).map(|q| q.into_attr()) {
            Some(AttrValue::String(s)) => s,
            _ => String::new(),
        }
    }

    /// Apply the boolean "is this row selected?" marks on the Files
    /// SelectList based on which `files_paths` entries are present in
    /// `selected_paths`. Called after every reload.
    fn sync_files_marks_from_selected(&mut self) {
        let indices: Vec<PropValue> = self
            .files_paths
            .iter()
            .enumerate()
            .filter(|(_, p)| self.selected_paths.contains(p))
            .map(|(i, _)| PropValue::Usize(i))
            .collect();
        self.files.attr(
            Attribute::Custom(ATTR_SELECTED),
            AttrValue::Payload(PropPayload::Vec(indices)),
        );
    }

    /// Reconcile `selected_paths` with the Files SelectList after a toggle.
    /// Paths outside the current Files enumeration are preserved; paths
    /// inside it follow the toggle state of the SelectList.
    fn sync_selected_from_files(&mut self, files_indices: Vec<usize>) {
        let now_selected: Vec<PathBuf> = files_indices
            .iter()
            .filter_map(|&i| self.files_paths.get(i).cloned())
            .collect();
        let in_files: Vec<&PathBuf> = self.files_paths.iter().collect();

        self.selected_paths.retain(|p| {
            // Keep paths that aren't part of the current Files enumeration,
            // or that are part of it AND currently toggled on.
            !in_files.contains(&p) || now_selected.contains(p)
        });
        for p in now_selected {
            if !self.selected_paths.contains(&p) {
                self.selected_paths.push(p);
            }
        }
        self.refresh_selected_pane();
    }

    fn refresh_selected_pane(&mut self) {
        let labels: Vec<String> = self
            .selected_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        self.selected.set_items(labels);
    }

    fn sync_subfocus(&mut self) {
        let dir_active = self.focused && self.focus_slot == FilePickerFocus::Directory;
        let glob_active = self.focused && self.focus_slot == FilePickerFocus::Glob;
        let files_active = self.focused && self.focus_slot == FilePickerFocus::Files;
        let selected_active = self.focused && self.focus_slot == FilePickerFocus::Selected;

        self.directory
            .attr(Attribute::Focus, AttrValue::Flag(dir_active));
        self.glob
            .attr(Attribute::Focus, AttrValue::Flag(glob_active));
        // dir_picker is the autocomplete dropdown for the Directory
        // input. It shares focus with the Directory input.
        self.dir_picker
            .attr(Attribute::Focus, AttrValue::Flag(dir_active));
        self.files
            .attr(Attribute::Focus, AttrValue::Flag(files_active));
        self.selected
            .attr(Attribute::Focus, AttrValue::Flag(selected_active));
    }

    /// `true` when the dir-picker dropdown should be visible — only
    /// while the Directory input has focus.
    fn dir_picker_open(&self) -> bool {
        self.focus_slot == FilePickerFocus::Directory
    }

    /// `true` when the file picker body should be expanded under its
    /// title (Glob or Files focused). The title is always visible.
    fn files_picker_open(&self) -> bool {
        matches!(
            self.focus_slot,
            FilePickerFocus::Glob | FilePickerFocus::Files
        )
    }
}

/// Compute the body height of an open dropdown so it fits its content
/// exactly: filter row + items + footer row. Items are capped so the
/// dropdown can't grow without bound; scrolling kicks in beyond that.
fn dropdown_body_height(item_count: usize) -> u16 {
    const MAX_VISIBLE: u16 = 8;
    let items = (item_count as u16).min(MAX_VISIBLE);
    1 + items + 1
}

/// Height for the Directory autocomplete dropdown: bare items only, no
/// filter row (the filter is implicit in the Directory input) and no
/// footer row. Capped at the same visible limit.
fn dir_dropdown_height(item_count: usize) -> u16 {
    const MAX_VISIBLE: u16 = 8;
    (item_count as u16).min(MAX_VISIBLE)
}

/// Longest common prefix of a slice of strings, computed character by
/// character so multi-byte UTF-8 is handled cleanly (the cut never lands
/// inside a codepoint).
fn longest_common_prefix<S: AsRef<str>>(strs: &[S]) -> String {
    let Some(first) = strs.first().map(|s| s.as_ref()) else {
        return String::new();
    };
    let mut common_bytes = first.len();
    for s in &strs[1..] {
        let s = s.as_ref();
        let prefix_bytes: usize = first
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
        common_bytes = common_bytes.min(prefix_bytes);
    }
    first[..common_bytes].to_string()
}

/// Parse the Directory input string into `(parent_dir, prefix)` for
/// dropdown autocompletion.
///
/// - Empty input returns `(""/empty path, "")` — nothing to enumerate.
/// - Input ending in `/` (e.g. `"/foo/bar/"`) returns
///   `("/foo/bar/", "")` so the dropdown lists every subdir of that
///   directory unfiltered.
/// - Input without a trailing `/` (e.g. `"/foo/bar/hall"`) splits on
///   the last `/` into `("/foo/bar/", "hall")`.
///
/// Splitting is done on the raw string (not via [`Path::file_name`])
/// because the latter returns `None` for `.` and `..`, which would
/// silently swallow those as prefix filters — typing a leading `.` to
/// hunt for dotfiles must keep `.` as the prefix.
fn parse_dir_input(raw: &str) -> (PathBuf, String) {
    if raw.is_empty() {
        return (PathBuf::new(), String::new());
    }
    if raw.ends_with('/') {
        return (PathBuf::from(raw), String::new());
    }
    match raw.rfind('/') {
        Some(slash) => {
            let parent = &raw[..=slash];
            let prefix = &raw[slash + 1..];
            (PathBuf::from(parent), prefix.to_string())
        }
        None => (PathBuf::from("."), raw.to_string()),
    }
}

/// Format an absolute file path for the Files pane: relative to `base`
/// when possible, falling back to the absolute path for entries outside
/// `base` (defensive — shouldn't occur in practice since enumeration is
/// rooted at `base`).
fn label_relative_to(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// Render a single-row pane title with the `▍ ` left bar, matching the
/// look of [`TextInput`]'s title row.
fn render_pane_title(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    active: bool,
    style: &FilePickerStyle,
) {
    render_picker_row(frame, area, title, active, style, /* bold */ true);
}

/// Render the "current selection" row shown directly below a picker's
/// title when the picker is collapsed (body hidden). Mirrors the
/// title row's look but without BOLD so the label/value pair reads
/// as one logical unit.
fn render_collapsed_value(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    active: bool,
    style: &FilePickerStyle,
) {
    render_picker_row(frame, area, text, active, style, /* bold */ false);
}

fn render_picker_row(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    active: bool,
    style: &FilePickerStyle,
    bold: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let (prefix_color, mut row_style) = title_styles(active, style);
    if !bold {
        row_style = row_style.remove_modifier(Modifier::BOLD);
    }
    let buf = frame.buffer_mut();
    let text_width = area.width.saturating_sub(PREFIX_LEN) as usize;
    render_prefixed_line(
        buf,
        area.x,
        area.y,
        area.width,
        text,
        text_width,
        &prefix_color,
        &row_style,
        false,
    );
}

/// Label at the SelectList's current cursor, if any. Used to show the
/// "current value" when a dropdown is collapsed.
fn cursor_label(list: &SelectList) -> Option<&str> {
    let idx = *list.filtered_indices.get(list.cursor)?;
    list.items.get(idx).map(|i| i.label.as_str())
}

/// Render a [`SelectList`] inside `area`, with a `▍ ` bar painted across
/// every row on the leftmost two columns. The list itself gets the inner
/// area (shifted right by [`PREFIX_LEN`]).
fn render_select_list_with_prefix(
    frame: &mut Frame,
    area: Rect,
    list: &mut SelectList,
    active: bool,
    style: &FilePickerStyle,
) {
    if area.height == 0 || area.width <= PREFIX_LEN {
        // Degenerate area: bail rather than try to paint into nothing.
        return;
    }
    let prefix_color = title_styles(active, style).0;
    let bar_bg = select_list_bar_bg(active, style);
    {
        let buf = frame.buffer_mut();
        for dy in 0..area.height {
            let y = area.y + dy;
            paint_prefix_cells(buf, area.x, y, prefix_color, bar_bg);
        }
    }
    let inner = Rect {
        x: area.x + PREFIX_LEN,
        width: area.width - PREFIX_LEN,
        ..area
    };
    list.view(frame, inner);
}

fn paint_prefix_cells(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    fg: Option<Color>,
    bg: Option<Color>,
) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char('▍');
        let mut s = Style::default();
        if let Some(fg) = fg {
            s = s.fg(fg);
        }
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        cell.set_style(s);
    }
    if let Some(cell) = buf.cell_mut((x + 1, y)) {
        cell.set_char(' ');
        let mut s = Style::default();
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        cell.set_style(s);
    }
}

/// Resolve the `(prefix_fg, title_style)` pair for a pane title row, based
/// on whether the pane is currently focused. The title's bg is pulled
/// from the active SelectListStyle's Item slot so the title row blends
/// with the body of the picker below it.
fn title_styles(active: bool, style: &FilePickerStyle) -> (Option<Color>, Style) {
    let list_style = if active {
        style.select_list_active.as_ref()
    } else {
        style.select_list_inactive.as_ref()
    };
    let prefix_color = list_style.and_then(|s| s.prefix_color);
    let body_bg = list_style.and_then(|s| {
        s.style(crate::widgets::select_list::SelectListStyleType::Item)
            .and_then(|st| st.bg)
    });
    let mut title_style = Style::default();
    if let Some(fg) = prefix_color {
        title_style = title_style.fg(fg);
    }
    if let Some(bg) = body_bg {
        title_style = title_style.bg(bg);
    }
    if active {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    (prefix_color, title_style)
}

/// Background colour used for the `▍ ` bar's cells, derived from the
/// active/inactive SelectListStyle's Item slot so the bar visually blends
/// with the first row of the list.
fn select_list_bar_bg(active: bool, style: &FilePickerStyle) -> Option<Color> {
    let list_style = if active {
        style.select_list_active.as_ref()
    } else {
        style.select_list_inactive.as_ref()
    };
    list_style.and_then(|s| {
        s.style(crate::widgets::select_list::SelectListStyleType::Item)
            .and_then(|st| st.bg)
    })
}

/// Expand a leading `~` or `~/` against `$HOME`. Embedded tildes
/// (`/foo/~/bar`) are left alone — shell-style `~user` is not supported.
fn expand_tilde(s: &str) -> String {
    let home = std::env::var("HOME").ok();
    expand_tilde_with(s, home.as_deref())
}

fn expand_tilde_with(s: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return s.to_string();
    };
    if s == "~" {
        return home.to_string();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        let mut out = String::with_capacity(home.len() + 1 + rest.len());
        out.push_str(home);
        out.push('/');
        out.push_str(rest);
        return out;
    }
    s.to_string()
}

impl Component for FilePicker {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // With a title configured the picker owns its full panel: paint
        // the panel BG, draw the title on top, the help bar on bottom,
        // and reserve the middle band for the picker proper.
        let inner_area = if self.title.is_some() {
            self.render_chrome(frame, area)
        } else {
            area
        };
        self.render_picker_body(frame, inner_area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        if let Attribute::Focus = attr {
            if let AttrValue::Flag(f) = value {
                self.focused = f;
                self.sync_subfocus();
            }
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::Usize(self.focus_slot as usize))
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

impl FilePicker {
    fn render_picker_body(&mut self, frame: &mut Frame, area: Rect) {
        // Layout:
        //   Directory      (text input, 2 rows)
        //   <dropdown>     (autocomplete list, only when Directory focused)
        //   <blank>
        //   Glob           (text input, 2 rows)
        //   <blank>
        //   Files          (title + collapsed value OR full dropdown body)
        //   <blank>
        //   Selected       (title + always-expanded body)
        let dir_open = self.dir_picker_open();
        let files_open = self.files_picker_open();

        let dir_body_h = if dir_open {
            dir_dropdown_height(self.dir_picker_paths.len())
        } else {
            0
        };
        let files_body_h = if files_open {
            dropdown_body_height(self.files_paths.len())
        } else {
            1
        };
        // Selected pane is always expanded but sized to its content so
        // an empty/short selection doesn't leave trailing bar-rows below
        // the last item.
        let selected_body_h = dropdown_body_height(self.selected_paths.len());

        let chunks = Layout::vertical([
            Constraint::Length(2),               // 0: Directory input (title + value)
            Constraint::Length(dir_body_h),      // 1: Directory autocomplete dropdown
            Constraint::Length(1),               // 2: blank
            Constraint::Length(2),               // 3: Glob input
            Constraint::Length(1),               // 4: blank
            Constraint::Length(1),               // 5: Files title
            Constraint::Length(files_body_h),    // 6: Files body OR collapsed value
            Constraint::Length(1),               // 7: blank
            Constraint::Length(1),               // 8: Selected title
            Constraint::Length(selected_body_h), // 9: Selected body
        ])
        .split(area);

        self.directory.view(frame, chunks[0]);

        let dir_focused = self.focus_slot == FilePickerFocus::Directory;
        if dir_open && chunks[1].height > 0 {
            render_select_list_with_prefix(
                frame,
                chunks[1],
                &mut self.dir_picker,
                dir_focused,
                &self.style,
            );
        }

        self.glob.view(frame, chunks[3]);

        let files_focused = self.focus_slot == FilePickerFocus::Files;
        render_pane_title(
            frame,
            chunks[5],
            &self.files_title,
            files_focused,
            &self.style,
        );
        if files_open {
            render_select_list_with_prefix(
                frame,
                chunks[6],
                &mut self.files,
                files_focused,
                &self.style,
            );
        } else {
            let value = cursor_label(&self.files).unwrap_or("");
            render_collapsed_value(frame, chunks[6], value, files_focused, &self.style);
        }

        let selected_focused = self.focus_slot == FilePickerFocus::Selected;
        render_pane_title(
            frame,
            chunks[8],
            &self.selected_title,
            selected_focused,
            &self.style,
        );
        render_select_list_with_prefix(
            frame,
            chunks[9],
            &mut self.selected,
            selected_focused,
            &self.style,
        );
    }

    fn render_chrome(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        let Some(title_text) = self.title.clone() else {
            return area;
        };
        if area.height == 0 || area.width == 0 {
            return area;
        }

        let panel_bg = self.style.panel_bg;
        if let Some(bg) = panel_bg {
            let bg_block = ratatui::widgets::Block::default().style(Style::default().bg(bg));
            frame.render_widget(bg_block, area);
        }

        let title_style = self
            .style
            .title_style
            .unwrap_or_else(|| Style::default().add_modifier(Modifier::BOLD));
        let help_lines = self.help_lines(area.width.saturating_sub(4));
        let help_h = help_lines.len() as u16;

        // Vertical layout: blank, title, blank, …picker…, blank, …help…, blank
        // → picker height = area.height − 5 − help_h
        // → picker top    = area.y + 3
        let title_row = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), 1);
        let title_para = Paragraph::new(Line::from(vec![Span::styled(title_text, title_style)]));
        frame.render_widget(title_para, title_row);

        // Transient paste-error banner. Lives in the blank gutter row
        // between title and picker body so the picker layout below stays
        // unchanged. Theme via `FilePickerStyle::paste_error_style`,
        // defaulting to red on the panel background.
        if let Some(err) = self.paste_error.clone() {
            let err_style = self.style.paste_error_style.unwrap_or_else(|| {
                let mut s = Style::default().fg(Color::Red);
                if let Some(bg) = panel_bg {
                    s = s.bg(bg);
                }
                s
            });
            let err_row = Rect::new(area.x + 2, area.y + 2, area.width.saturating_sub(4), 1);
            let err_para = Paragraph::new(Line::from(vec![Span::styled(err, err_style)]));
            frame.render_widget(err_para, err_row);
        }

        let help_top = area
            .y
            .saturating_add(area.height.saturating_sub(help_h + 1));
        let help_w = area.width.saturating_sub(4);
        for (i, line) in help_lines.into_iter().enumerate() {
            let row = Rect::new(area.x + 2, help_top + i as u16, help_w, 1);
            frame.render_widget(Paragraph::new(line), row);
        }

        let picker_h = area.height.saturating_sub(5 + help_h);
        Rect::new(
            area.x + 2,
            area.y + 3,
            area.width.saturating_sub(4),
            picker_h,
        )
    }

    /// Build the help-bar lines from the live keymap, greedily wrapping
    /// each `<keys> <label>` entry across rows.
    fn help_lines(&self, max_width: u16) -> Vec<Line<'static>> {
        let keys_style = self
            .style
            .help_keys_style
            .unwrap_or_else(|| Style::default().add_modifier(Modifier::BOLD));
        let labels_style = self
            .style
            .help_labels_style
            .unwrap_or_else(|| Style::default().fg(Color::DarkGray));

        let entries: Vec<(String, &'static str)> = vec![
            (
                format!(
                    "{}/{}",
                    self.keymap.focus_next.display(),
                    self.keymap.focus_prev.display(),
                ),
                "focus",
            ),
            (
                format!(
                    "{}/{}",
                    self.keymap.browse_down.display(),
                    self.keymap.browse_up.display(),
                ),
                "nav",
            ),
            (self.keymap.toggle.display(), "toggle"),
            (self.keymap.tab_complete.display(), "complete"),
            (self.keymap.filter_clear.display(), "clear filter"),
            (self.keymap.remove_selected.display(), "remove"),
            (self.keymap.paste.display(), "paste"),
            (self.keymap.submit.display(), "submit"),
            (self.keymap.cancel.display(), "cancel"),
        ];

        const SEP: usize = 2;
        let mut rows: Vec<Vec<(String, String)>> = vec![Vec::new()];
        let mut current_width: usize = 0;
        let budget = max_width as usize;

        for (keys, label) in entries {
            let entry_width = keys.chars().count() + 1 + label.chars().count();
            let last_empty = rows.last().map(|r| r.is_empty()).unwrap_or(true);
            let tentative = if last_empty {
                entry_width
            } else {
                current_width + SEP + entry_width
            };
            if !last_empty && tentative > budget {
                rows.push(Vec::new());
                current_width = entry_width;
            } else {
                current_width = tentative;
            }
            rows.last_mut().unwrap().push((keys, label.to_string()));
        }

        rows.into_iter()
            .map(|pairs| {
                let mut spans: Vec<Span<'static>> = Vec::new();
                for (i, (keys, label)) in pairs.into_iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::raw("  "));
                    }
                    spans.push(Span::styled(keys, keys_style));
                    spans.push(Span::styled(format!(" {label}"), labels_style));
                }
                Line::from(spans)
            })
            .collect()
    }
}

impl AppComponent<FilePickerEvent, NoUserEvent> for FilePicker {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<FilePickerEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        // Any keypress wipes a stale paste-error banner. A paste that
        // fails again re-sets it further down.
        self.paste_error = None;

        if self.keymap.submit.matches(&key_ev) {
            return Some(FilePickerEvent::Confirmed(self.confirmed_paths()));
        }
        if self.keymap.cancel.matches(&key_ev) {
            return Some(FilePickerEvent::Cancelled);
        }
        if self.keymap.focus_next.matches(&key_ev) {
            self.set_focus_slot(self.focus_slot.next());
            return Some(FilePickerEvent::FocusChanged);
        }
        if self.keymap.focus_prev.matches(&key_ev) {
            self.set_focus_slot(self.focus_slot.prev());
            return Some(FilePickerEvent::FocusChanged);
        }
        if self.keymap.paste.matches(&key_ev) {
            match self.paste_provider.as_ref().and_then(|p| p()) {
                Some(text) => self.paste_clipboard_path(&text),
                None => {
                    self.paste_error = Some("Clipboard is empty".to_string());
                }
            }
            return None;
        }

        // Directory focus has dedicated dropdown-nav keys:
        // browse_down/up move the autocomplete cursor; browse_navigate
        // (Enter) commits the highlighted entry into the input;
        // tab_complete (Tab) extends the input to the longest common
        // prefix of the literal-prefix matches in the dropdown. All
        // other keys fall through to the Directory TextInput so the
        // user keeps typing the path manually.
        if self.focus_slot == FilePickerFocus::Directory {
            if self.keymap.browse_down.matches(&key_ev) {
                self.dir_picker.perform(Cmd::Move(Direction::Down));
                return None;
            }
            if self.keymap.browse_up.matches(&key_ev) {
                self.dir_picker.perform(Cmd::Move(Direction::Up));
                return None;
            }
            if self.keymap.browse_navigate.matches(&key_ev) {
                if let Some(target) = self.dotdot_target() {
                    self.set_directory(&target);
                } else if let Some(target) = self.browse_target() {
                    self.set_directory(&target);
                }
                return None;
            }
            if self.keymap.tab_complete.matches(&key_ev) {
                self.tab_complete_directory();
                return None;
            }
        }

        // Files / Selected: picker-level intercepts for filter_clear and
        // remove_selected. Both fire before the key reaches the
        // SelectList so `,` (filter_clear) doesn't get typed into the
        // filter and `Ctrl+D` (remove_selected) reaches us before any
        // future SelectList default could ever bind to it.
        if matches!(
            self.focus_slot,
            FilePickerFocus::Files | FilePickerFocus::Selected
        ) {
            if self.keymap.filter_clear.matches(&key_ev) {
                match self.focus_slot {
                    FilePickerFocus::Files => self.files.filter_clear(),
                    FilePickerFocus::Selected => self.selected.filter_clear(),
                    _ => {}
                }
                return None;
            }
            if self.keymap.remove_selected.matches(&key_ev) {
                self.remove_at_cursor();
                return None;
            }
        }

        let forwarded = Event::Keyboard(key_ev);
        match self.focus_slot {
            FilePickerFocus::Directory => {
                let sub_ev = <TextInput as AppComponent<_, _>>::on(&mut self.directory, &forwarded);
                if matches!(sub_ev, Some(TextInputEvent::Changed(_))) {
                    self.reload();
                }
            }
            FilePickerFocus::Glob => {
                let sub_ev = <TextInput as AppComponent<_, _>>::on(&mut self.glob, &forwarded);
                if matches!(sub_ev, Some(TextInputEvent::Changed(_))) {
                    self.reload();
                }
            }
            FilePickerFocus::Files => {
                let sub_ev = <SelectList as AppComponent<_, _>>::on(&mut self.files, &forwarded);
                if let Some(SelectListEvent::SelectionChanged(idxs)) = sub_ev {
                    self.sync_selected_from_files(idxs);
                }
            }
            FilePickerFocus::Selected => {
                let _ = <SelectList as AppComponent<_, _>>::on(&mut self.selected, &forwarded);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tuirealm::event::{Key, KeyEvent, KeyModifiers};

    fn ctrl(c: char) -> Event<NoUserEvent> {
        Event::Keyboard(KeyEvent {
            code: Key::Char(c),
            modifiers: KeyModifiers::CONTROL,
        })
    }

    fn key(code: Key, mods: KeyModifiers) -> Event<NoUserEvent> {
        Event::Keyboard(KeyEvent {
            code,
            modifiers: mods,
        })
    }

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, b"").unwrap();
    }

    #[test]
    fn default_focus_is_files() {
        let p = FilePicker::default();
        assert_eq!(p.focus_slot(), FilePickerFocus::Files);
    }

    #[test]
    fn focus_next_cycles() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);
        assert!(matches!(
            <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('l')),
            Some(FilePickerEvent::FocusChanged)
        ));
        assert_eq!(p.focus_slot(), FilePickerFocus::Glob);
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('l'));
        assert_eq!(p.focus_slot(), FilePickerFocus::Files);
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('l'));
        assert_eq!(p.focus_slot(), FilePickerFocus::Selected);
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('l'));
        assert_eq!(p.focus_slot(), FilePickerFocus::Directory);
    }

    #[test]
    fn focus_prev_cycles_backwards() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('h'));
        assert_eq!(p.focus_slot(), FilePickerFocus::Selected);
    }

    #[test]
    fn esc_emits_cancelled() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        let out =
            <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Esc, KeyModifiers::NONE));
        assert_eq!(out, Some(FilePickerEvent::Cancelled));
    }

    #[test]
    fn ctrl_enter_descends_in_directory_picker() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("alpha")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::CONTROL));

        let alpha_str = format!("{}/", tmp.path().join("alpha").display());
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(alpha_str)),
        );
    }

    #[test]
    fn ctrl_enter_toggles_selection_in_files_pane() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Files);

        assert!(p.selected_paths.is_empty());
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::CONTROL));
        assert_eq!(p.selected_paths.len(), 1);
        // Ctrl+Enter again toggles it back off.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::CONTROL));
        assert!(p.selected_paths.is_empty());
    }

    #[test]
    fn custom_toggle_keymap_propagates_to_inner_lists() {
        let custom = FilePickerKeymap {
            toggle: crate::widgets::common::Keys::plain(Key::Char(' ')),
            ..FilePickerKeymap::default()
        };
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt")
            .with_keymap(custom);
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Files);

        // Enter is no longer bound for toggle (typing into filter falls
        // through to nothing for Enter, so nothing changes there).
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::NONE));
        assert!(p.selected_paths.is_empty());

        // Space now toggles.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char(' '), KeyModifiers::NONE));
        assert_eq!(p.selected_paths.len(), 1);
    }

    #[test]
    fn ctrl_o_emits_confirmed_with_paths() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        let out = <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('o'));
        assert_eq!(out, Some(FilePickerEvent::Confirmed(Vec::new())));
    }

    #[test]
    fn directory_focus_routes_chars_to_directory_input() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('a'), KeyModifiers::NONE));
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('/'), KeyModifiers::NONE));
        let v = p.directory.query(Attribute::Value).map(|q| q.into_attr());
        assert_eq!(v, Some(AttrValue::String("a/".into())));
    }

    #[test]
    fn reload_populates_files_from_enumeration() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");
        touch(tmp.path(), "beta.txt");
        touch(tmp.path(), "gamma.png");

        let p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        assert_eq!(p.files_paths.len(), 2);
    }

    #[test]
    fn selected_persists_across_directory_change() {
        let tmp_a = TempDir::new().unwrap();
        touch(tmp_a.path(), "a.txt");
        let tmp_b = TempDir::new().unwrap();
        touch(tmp_b.path(), "b.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp_a.path().display().to_string())
            .with_initial_glob("*.txt");
        assert_eq!(p.files_paths.len(), 1);

        // Pretend the user selected the only entry in dir A.
        let a_path = tmp_a.path().join("a.txt");
        p.selected_paths.push(a_path.clone());
        p.sync_files_marks_from_selected();

        // Switch the directory by re-applying the builder pattern is awkward,
        // so call the internal reload after rewriting the directory input.
        p.directory.attr(
            Attribute::Value,
            AttrValue::String(tmp_b.path().display().to_string()),
        );
        p.reload();

        // Dir B's enumeration replaces files_paths, but a_path is preserved
        // in selected_paths (it's not part of B's enumeration).
        assert_eq!(p.files_paths.len(), 1);
        assert!(p.files_paths[0].ends_with("b.txt"));
        assert!(p.selected_paths.contains(&a_path));
    }

    #[test]
    fn confirmed_paths_returns_selected_paths() {
        let mut p = FilePicker::default();
        let dummy = PathBuf::from("/tmp/synthetic.txt");
        p.selected_paths.push(dummy.clone());
        assert_eq!(p.confirmed_paths(), vec![dummy]);
    }

    #[test]
    fn dir_picker_populates_with_subdirs() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("alpha")).unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        touch(tmp.path(), "ignored_file.txt");

        let p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());

        // 2 subdirs, alphabetically; the file is excluded.
        assert_eq!(p.dir_picker_paths.len(), 2);
        assert!(p.dir_picker_paths[0].ends_with("alpha"));
        assert!(p.dir_picker_paths[1].ends_with("beta"));
    }

    #[test]
    fn dir_picker_filters_by_basename_prefix() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("hallo")).unwrap();
        fs::create_dir(tmp.path().join("hallo_welt")).unwrap();
        fs::create_dir(tmp.path().join("holla_welt")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        assert_eq!(p.dir_picker_paths.len(), 3);

        // Append "hall" so the input becomes ".../hall". The dropdown
        // should narrow to subdirs of the parent whose name starts
        // with "hall".
        let with_prefix = format!("{}/hall", tmp.path().display());
        p.directory
            .attr(Attribute::Value, AttrValue::String(with_prefix));
        p.reload();

        assert_eq!(p.dir_picker_paths.len(), 2);
        assert!(p.dir_picker_paths[0].ends_with("hallo"));
        assert!(p.dir_picker_paths[1].ends_with("hallo_welt"));
    }

    #[test]
    fn files_and_dir_picker_populate_independently() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        fs::create_dir(tmp.path().join("sub")).unwrap();

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));

        // files reflects the glob; dir_picker reflects subdirs. Focus
        // changes don't repopulate either.
        assert_eq!(p.files_paths.len(), 1);
        assert!(p.files_paths[0].ends_with("a.txt"));
        assert_eq!(p.dir_picker_paths.len(), 1);

        p.set_focus_slot(FilePickerFocus::Directory);
        assert_eq!(p.files_paths.len(), 1);
        assert_eq!(p.dir_picker_paths.len(), 1);

        p.set_focus_slot(FilePickerFocus::Files);
        assert_eq!(p.files_paths.len(), 1);
        assert_eq!(p.dir_picker_paths.len(), 1);
    }

    #[test]
    fn picker_visibility_follows_focus() {
        let p = FilePicker::default();
        // Default focus is Files → file picker body open, dir picker
        // body closed. Titles are always shown regardless.
        assert!(!p.dir_picker_open());
        assert!(p.files_picker_open());

        let mut p = p;
        p.set_focus_slot(FilePickerFocus::Directory);
        assert!(p.dir_picker_open());
        assert!(!p.files_picker_open());

        p.set_focus_slot(FilePickerFocus::Glob);
        assert!(!p.dir_picker_open());
        assert!(p.files_picker_open());

        // Selected focused → both dropdowns collapse, only Selected
        // body remains visible.
        p.set_focus_slot(FilePickerFocus::Selected);
        assert!(!p.dir_picker_open());
        assert!(!p.files_picker_open());
    }

    #[test]
    fn browse_navigate_descends_into_highlighted_subdir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("alpha")).unwrap();
        touch(&tmp.path().join("alpha"), "inside.txt");

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);

        // Cursor is on `alpha` (the only entry); Enter descends. The
        // resolved value gets a trailing slash so the next dropdown
        // enumeration lists alpha's children unfiltered.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::NONE));

        let dir_value = p.directory.query(Attribute::Value).map(|q| q.into_attr());
        let alpha_str = format!("{}/", tmp.path().join("alpha").display());
        assert_eq!(dir_value, Some(AttrValue::String(alpha_str)));

        // dir_picker now shows the contents of `alpha`: no subdirs (the
        // file `inside.txt` is filtered out).
        assert_eq!(p.dir_picker_paths.len(), 0);
    }

    #[test]
    fn expand_tilde_with_replaces_leading_tilde() {
        assert_eq!(expand_tilde_with("~", Some("/home/me")), "/home/me");
        assert_eq!(expand_tilde_with("~/", Some("/home/me")), "/home/me/");
        assert_eq!(
            expand_tilde_with("~/foo/bar", Some("/home/me")),
            "/home/me/foo/bar",
        );
        // Embedded tildes are untouched.
        assert_eq!(
            expand_tilde_with("/var/~/foo", Some("/home/me")),
            "/var/~/foo",
        );
        // No HOME → pass-through.
        assert_eq!(expand_tilde_with("~/foo", None), "~/foo");
    }

    #[test]
    fn tilde_directory_enumerates_against_home() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");

        // SAFETY: tests within this crate are run serially per-thread; we
        // need a real HOME for the picker to expand `~` against.
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }

        let p = FilePicker::default()
            .with_initial_directory("~")
            .with_initial_glob("*.txt");
        assert_eq!(p.files_paths.len(), 1);
        assert!(p.is_directory_valid());

        // Restore HOME so we don't leak across tests.
        unsafe {
            match prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn missing_directory_marks_invalid_and_retitles() {
        let p = FilePicker::default().with_initial_directory("/does/not/exist/hopefully");
        assert!(!p.is_directory_valid());
        assert_eq!(p.directory.title, "Directory (not found)");
    }

    #[test]
    fn empty_directory_is_treated_as_valid() {
        let p = FilePicker::default();
        assert!(p.is_directory_valid());
        assert_eq!(p.directory.title, "Directory");
    }

    #[test]
    fn fixing_directory_clears_invalid_flag() {
        let tmp = TempDir::new().unwrap();
        let mut p = FilePicker::default().with_initial_directory("/does/not/exist");
        assert!(!p.is_directory_valid());

        p.directory.attr(
            Attribute::Value,
            AttrValue::String(tmp.path().display().to_string()),
        );
        p.reload();
        assert!(p.is_directory_valid());
        assert_eq!(p.directory.title, "Directory");
    }

    #[test]
    fn paste_directory_replaces_directory_input() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let path_str = tmp.path().display().to_string();

        let captured = path_str.clone();
        let mut p = FilePicker::default()
            .with_initial_glob("*.txt")
            .with_paste_provider(move || Some(captured.clone()));
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        // Paste is global — focus on Files should NOT matter.
        p.set_focus_slot(FilePickerFocus::Files);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));

        // set_directory appends a trailing `/`.
        let expected = format!("{path_str}/");
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(expected)),
        );
        assert!(p.is_directory_valid());
        assert!(p.paste_error.is_none());
        assert!(p.dir_picker_paths.iter().any(|p| p.ends_with("sub")));
        assert!(p.files_paths.iter().any(|p| p.ends_with("a.txt")));
    }

    #[test]
    fn paste_file_sets_parent_dir_and_adds_to_selection() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");
        let file_path = tmp.path().join("alpha.txt");

        let captured = file_path.display().to_string();
        let mut p = FilePicker::default().with_paste_provider(move || Some(captured.clone()));
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        // Even from Glob focus, paste does the global "set dir + select" dance.
        p.set_focus_slot(FilePickerFocus::Glob);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));

        let expected_dir = format!("{}/", tmp.path().display());
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(expected_dir)),
        );
        assert!(p.selected_paths.contains(&file_path));
        assert!(p.paste_error.is_none());
    }

    #[test]
    fn paste_uses_first_non_empty_line_of_multiline_input() {
        let tmp = TempDir::new().unwrap();
        let dir_str = tmp.path().display().to_string();
        // First line blank, second is the real path, third is noise.
        let payload = format!("\n  {dir_str}  \n/ignored/trailing/line");
        let mut p = FilePicker::default().with_paste_provider(move || Some(payload.clone()));
        p.attr(Attribute::Focus, AttrValue::Flag(true));

        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));

        let expected = format!("{dir_str}/");
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(expected)),
        );
        assert!(p.paste_error.is_none());
    }

    #[test]
    fn paste_invalid_path_sets_error_banner() {
        let mut p =
            FilePicker::default().with_paste_provider(|| Some("/no/such/path/at/all".to_string()));
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));
        assert!(p.paste_error.is_some());
        // Directory + Glob remained untouched.
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(String::new())),
        );
    }

    #[test]
    fn paste_with_no_provider_sets_error_banner() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));
        assert!(p.paste_error.is_some());
    }

    #[test]
    fn paste_error_clears_on_next_keypress() {
        let mut p =
            FilePicker::default().with_paste_provider(|| Some("/no/such/thing".to_string()));
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('v'));
        assert!(p.paste_error.is_some());
        // Any subsequent key wipes the banner.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('a'), KeyModifiers::NONE));
        assert!(p.paste_error.is_none());
    }

    #[test]
    fn fuzzy_match_finds_substring_in_subdir_name() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("not-yet-done-core")).unwrap();
        fs::create_dir(tmp.path().join("not-yet-done-tui")).unwrap();
        fs::create_dir(tmp.path().join("other")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());

        // Type "core" — fuzzy matches the `-core` suffix, prunes `other`
        // and `not-yet-done-tui` (no `c` available).
        let value = format!("{}/core", tmp.path().display());
        p.directory.attr(Attribute::Value, AttrValue::String(value));
        p.reload();

        assert_eq!(p.dir_picker_paths.len(), 1);
        assert!(p.dir_picker_paths[0].ends_with("not-yet-done-core"));
    }

    #[test]
    fn tab_extends_to_longest_common_prefix() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("not-yet-done-core")).unwrap();
        fs::create_dir(tmp.path().join("not-yet-done-ratatui")).unwrap();
        fs::create_dir(tmp.path().join("other")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);

        // Type "not" → both `not-yet-done-*` entries literally start
        // with it; the LCP is `not-yet-done-`. `other` is excluded
        // (it doesn't fuzzy-match `not`).
        let value = format!("{}/not", tmp.path().display());
        p.directory.attr(Attribute::Value, AttrValue::String(value));
        p.reload();

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Tab, KeyModifiers::NONE));

        let expected = format!("{}/not-yet-done-", tmp.path().display());
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(expected)),
        );
    }

    #[test]
    fn tab_is_noop_when_only_fuzzy_matches() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("not-yet-done-core")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);

        // Type "core" — fuzzy-matches but no entry literally starts
        // with "core", so Tab has nothing to extend.
        let value = format!("{}/core", tmp.path().display());
        p.directory
            .attr(Attribute::Value, AttrValue::String(value.clone()));
        p.reload();
        assert_eq!(p.dir_picker_paths.len(), 1);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Tab, KeyModifiers::NONE));

        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(value)),
        );
    }

    #[test]
    fn longest_common_prefix_basics() {
        let empty: Vec<&str> = Vec::new();
        assert_eq!(longest_common_prefix(&empty), "");
        assert_eq!(longest_common_prefix(&["abc"]), "abc");
        assert_eq!(longest_common_prefix(&["abc", "abd"]), "ab");
        assert_eq!(longest_common_prefix(&["foo", "bar"]), "");
        // Multi-byte codepoint never gets cut mid-character.
        assert_eq!(longest_common_prefix(&["üab", "üac"]), "üa");
    }

    #[test]
    fn parse_dir_input_splits_on_last_slash() {
        let (parent, prefix) = parse_dir_input("/foo/bar/hall");
        assert_eq!(parent, PathBuf::from("/foo/bar/"));
        assert_eq!(prefix, "hall");

        // Trailing slash → whole path is the parent, no prefix filter.
        let (parent, prefix) = parse_dir_input("/foo/bar/");
        assert_eq!(parent, PathBuf::from("/foo/bar/"));
        assert_eq!(prefix, "");

        // Empty → empty path, empty prefix; dropdown stays empty.
        let (parent, prefix) = parse_dir_input("");
        assert_eq!(parent, PathBuf::from(""));
        assert_eq!(prefix, "");

        // Leading `.` (dotfile hunt) survives split — Path::file_name
        // would have returned None and erased it.
        let (parent, prefix) = parse_dir_input("/foo/bar/.");
        assert_eq!(parent, PathBuf::from("/foo/bar/"));
        assert_eq!(prefix, ".");

        let (parent, prefix) = parse_dir_input("/foo/bar/..config");
        assert_eq!(parent, PathBuf::from("/foo/bar/"));
        assert_eq!(prefix, "..config");
    }

    #[test]
    fn dotdot_enter_navigates_to_parent_directory() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::create_dir(nested.join("inner")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(nested.display().to_string());
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Directory);

        // After init, value is `<nested>/` and dropdown lists `inner`.
        // Append `..` so input becomes `<nested>/..`, then press Enter.
        let with_dotdot = format!("{}/..", nested.display());
        p.directory
            .attr(Attribute::Value, AttrValue::String(with_dotdot));
        p.reload();

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Enter, KeyModifiers::NONE));

        // Directory now points at tmp (the grandparent), with trailing
        // `/` appended so the dropdown enumerates its children.
        let expected = format!("{}/", tmp.path().display());
        assert_eq!(
            p.directory.query(Attribute::Value).map(|q| q.into_attr()),
            Some(AttrValue::String(expected)),
        );
    }

    #[test]
    fn filter_clear_default_wipes_files_filter() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");
        touch(tmp.path(), "beta.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Files);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('a'), KeyModifiers::NONE));
        assert_eq!(p.files.filter_query, "a");

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char(','), KeyModifiers::NONE));
        assert_eq!(p.files.filter_query, "");
    }

    #[test]
    fn filter_clear_default_wipes_selected_filter() {
        let mut p = FilePicker::default();
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Selected);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('x'), KeyModifiers::NONE));
        assert_eq!(p.selected.filter_query, "x");

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char(','), KeyModifiers::NONE));
        assert_eq!(p.selected.filter_query, "");
    }

    #[test]
    fn filter_clear_key_is_configurable() {
        let custom = FilePickerKeymap {
            filter_clear: crate::widgets::common::Keys::ctrl(Key::Char('u')),
            ..FilePickerKeymap::default()
        };
        let mut p = FilePicker::default().with_keymap(custom);
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Selected);

        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char('x'), KeyModifiers::NONE));
        // Comma is no longer bound — typing it appends to the filter.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &key(Key::Char(','), KeyModifiers::NONE));
        assert_eq!(p.selected.filter_query, "x,");
        // Ctrl+U wipes it.
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('u'));
        assert_eq!(p.selected.filter_query, "");
    }

    #[test]
    fn ctrl_d_in_files_pane_removes_cursor_path_from_selected() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");
        touch(tmp.path(), "beta.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Files);

        // Pre-select both. Cursor is on row 0 (alphabetically first).
        p.selected_paths = p.files_paths.clone();
        p.sync_files_marks_from_selected();
        assert_eq!(p.selected_paths.len(), 2);

        let cursor_path = p
            .files_paths
            .get(p.files.filtered_indices[p.files.cursor])
            .cloned()
            .unwrap();

        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('d'));

        assert_eq!(p.selected_paths.len(), 1);
        assert!(!p.selected_paths.contains(&cursor_path));
    }

    #[test]
    fn ctrl_d_in_files_pane_on_unselected_file_is_noop() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Files);

        assert!(p.selected_paths.is_empty());
        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('d'));
        assert!(p.selected_paths.is_empty());
    }

    #[test]
    fn ctrl_d_in_selected_pane_removes_cursor_entry() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "alpha.txt");
        touch(tmp.path(), "beta.txt");

        let mut p = FilePicker::default()
            .with_initial_directory(tmp.path().display().to_string())
            .with_initial_glob("*.txt");
        p.attr(Attribute::Focus, AttrValue::Flag(true));
        p.set_focus_slot(FilePickerFocus::Selected);

        p.selected_paths = p.files_paths.clone();
        p.refresh_selected_pane();
        let first = p.selected_paths[0].clone();

        <FilePicker as AppComponent<_, _>>::on(&mut p, &ctrl('d'));

        assert_eq!(p.selected_paths.len(), 1);
        assert!(!p.selected_paths.contains(&first));
    }

    #[test]
    fn dot_prefix_keeps_dotfiles_in_dropdown() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".config")).unwrap();
        fs::create_dir(tmp.path().join(".cache")).unwrap();
        fs::create_dir(tmp.path().join("plain")).unwrap();

        let mut p = FilePicker::default().with_initial_directory(tmp.path().display().to_string());
        // All 3 subdirs visible when no prefix has been typed.
        assert_eq!(p.dir_picker_paths.len(), 3);

        // Append `.` → only the two dotted entries remain.
        let with_dot = format!("{}/.", tmp.path().display());
        p.directory
            .attr(Attribute::Value, AttrValue::String(with_dot));
        p.reload();

        assert_eq!(p.dir_picker_paths.len(), 2);
        assert!(p.dir_picker_paths.iter().all(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
        }));
    }
}
