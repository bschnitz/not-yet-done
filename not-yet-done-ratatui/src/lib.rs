pub mod utils;
pub mod widgets;

pub use utils::open_editor::{
    DetachedEditor, EditorError, open_editor, open_editor_detached, open_editor_detached_in,
    open_editor_inline, open_editor_inline_in, open_editor_launch, open_editor_launch_in,
    render_env_prefix,
};

// --- shared primitives ---
pub use widgets::common::{Keys, SelectionMarker, SelectionMode, hex_color};

// --- text_input ---
pub use widgets::text_input::{
    ATTR_ERROR, TextInput, TextInputEvent, TextInputKeymap, TextInputStyle, TextInputStyleType,
};

// --- multi_choice ---
pub use widgets::multi_choice::{
    ATTR_SELECTED, MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceStyle,
    MultiChoiceStyleType,
};

// --- toggle ---
pub use widgets::toggle::{Toggle, ToggleStyle, ToggleStyleType};

// --- form ---
#[cfg(feature = "natural-date")]
pub use widgets::form::datetime_preview;
pub use widgets::form::{
    FieldCondition, Form, FormEvent, FormFieldKind, FormFieldSpec, FormNotice, FormOptions,
    FormPalette, FormStyle, SelectStyle,
};

// --- select_list ---
pub use widgets::select_list::{
    ATTR_ITEMS, ATTR_SELECTED as SELECT_LIST_ATTR_SELECTED, SelectList, SelectListEvent,
    SelectListItemData, SelectListKeymap, SelectListStyle, SelectListStyleType,
};

// --- table ---
pub use widgets::table::{
    ColumnStyles, ImageDraw, ImageLineRef, ImagePainter, JumpPhase, LinkHopOutcome, LinkMatch,
    LinkPhase, StyleMap, Table, TableEvent, TableKeymap, TableStyle, TableStyleType,
    TableWidgetCell, TableWidgetLine, TableWidgetRow,
};

// --- file_picker ---
pub use widgets::file_picker::{
    EnumerationOptions, FilePicker, FilePickerEvent, FilePickerFocus, FilePickerKeymap,
    FilePickerStyle, enumerate,
};

// --- grid ---
pub use widgets::grid::{
    BORDER_DASHED, BORDER_DASHED_EXTENDED, BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
    BORDER_DOUBLE_EXTENDED, BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED, BORDER_SIMPLE,
    BORDER_SIMPLE_EXTENDED, BORDER_THICK_EXTENDED, BorderChars, BorderPos, CellGroup, GapPos, Grid,
    GridChild, GridEvent, GridKeymap, TextAnchor,
};

// --- leader_list ---
pub use widgets::leader_list::{
    LeaderEntry, LeaderList, LeaderListEvent, LeaderListKeymap, LeaderListStyle,
    LeaderListStyleType, LeaderWidth,
};
