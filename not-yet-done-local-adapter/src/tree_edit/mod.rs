//! Markdown-based tree editor: serialize a task subtree to a checkbox list,
//! parse it back, diff against the original, and apply changes.

mod serialize;
mod parse;
mod diff;
#[cfg(test)]
mod integration_tests;

#[allow(unused_imports)]
pub use serialize::serialize;
pub use serialize::serialize_with_indent;
pub use diff::apply_changes;
