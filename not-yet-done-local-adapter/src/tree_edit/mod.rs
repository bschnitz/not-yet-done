//! Markdown-based tree editor: serialize a task subtree to a checkbox list,
//! parse it back, diff against the original, and apply changes.

mod diff;
#[cfg(test)]
mod integration_tests;
mod parse;
mod serialize;

pub use diff::apply_changes;
#[allow(unused_imports)]
pub use serialize::serialize;
pub use serialize::serialize_with_indent;
