//! Shared task/forest helpers. The legacy Tasks tab rendering that used
//! to live here was removed when that tab was retired in favour of the
//! generic ContentAdapter "Tasks" tab; the submodules below survive
//! because the Trackings tab and content rendering still use them
//! (`forest` for the in-memory tree, `view_helpers` for centered
//! messages / date formatting).
pub mod forest;
pub mod highlight;
pub mod sort;
pub mod view_helpers;
