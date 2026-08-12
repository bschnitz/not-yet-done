pub mod layout;
pub mod render;
pub mod types;

// Flat re-exports for convenient use in downstream crates.
pub use layout::{GridLayout, compute_layout};
pub use render::{CharBuf, RenderTarget, draw_borders};
pub use types::{
    BORDER_DASHED, BORDER_DASHED_EXTENDED, BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
    BORDER_DOUBLE_EXTENDED, BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED, BORDER_SIMPLE,
    BORDER_SIMPLE_EXTENDED, BORDER_THICK_EXTENDED, BorderChars, BorderPos, BorderText, CellGroup,
    GapPos, GapSlot, GridConfig, SpannedBorder, TextAnchor,
};
