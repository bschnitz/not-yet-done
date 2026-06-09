pub mod utils;
pub mod widgets;

pub use utils::open_editor::{
    open_editor, open_editor_inline, open_editor_launch, open_editor_detached,
    open_editor_inline_in, open_editor_launch_in, open_editor_detached_in,
    render_env_prefix, DetachedEditor, EditorError,
};

// --- shared primitives ---
pub use widgets::common::{hex_color, Keys, SelectionMarker, SelectionMode};

// --- text_input ---
pub use widgets::text_input::{
    TextInput, TextInputEvent, TextInputKeymap,
    TextInputStyle, TextInputStyleType, ATTR_ERROR,
};

// --- multi_choice ---
pub use widgets::multi_choice::{
    MultiChoice, MultiChoiceEvent, MultiChoiceKeymap,
    MultiChoiceStyle, MultiChoiceStyleType, ATTR_SELECTED,
};

// --- select_list ---
pub use widgets::select_list::{
    SelectList, SelectListEvent, SelectListKeymap,
    SelectListStyle, SelectListStyleType,
    SelectListItemData,
    ATTR_ITEMS, ATTR_SELECTED as SELECT_LIST_ATTR_SELECTED,
};

// --- table ---
pub use widgets::table::{
    Table, JumpPhase, TableEvent, TableKeymap, TableStyle, TableStyleType,
    TableWidgetCell, TableWidgetLine, TableWidgetRow, ColumnStyles, StyleMap,
};

// --- file_picker ---
pub use widgets::file_picker::{
    enumerate, EnumerationOptions, FilePicker, FilePickerEvent,
    FilePickerFocus, FilePickerKeymap, FilePickerStyle,
};

// --- grid ---
pub use widgets::grid::{
    Grid, GridEvent, GridKeymap, GridChild,
    BorderPos, BorderChars, GapPos, CellGroup, TextAnchor,
    BORDER_SIMPLE, BORDER_SIMPLE_EXTENDED,
    BORDER_DOUBLE_EXTENDED, BORDER_THICK_EXTENDED,
    BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED,
    BORDER_DASHED, BORDER_DASHED_EXTENDED,
    BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
};
