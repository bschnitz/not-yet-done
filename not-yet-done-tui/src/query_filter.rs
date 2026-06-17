//! Re-export query filter parsing from core, plus TUI-specific templates.

pub use not_yet_done_core::filter::query_filter::parse;

/// Generate the YAML template for a new tracking query filter.
pub fn tracking_template() -> String {
    r#"query:
  and:
    - [deleted, =, false]
# Query — applied live on each save (:w)
#
# Tracking fields:
#   [started_at, '>=', '2 weeks ago']
#   [started_at, '<=', 'yesterday']
#   [ended_at, is_null]           # active (running) trackings
#   [ended_at, is_not_null]       # completed trackings
#   [deleted, =, false]
#
# Task fields (use t. prefix):
#   [t.description, has, meeting]
#   [t.status, =, done]           # todo, in_progress, done, cancelled
#   [t.priority, '>=', 5]
#   [t.deleted, =, false]
#
# Tree filters (searches task descriptions, filters by path):
#   [in_tree, Globex]              # trackings of Globex and all sub-tasks
#   [has_ancestor, Globex]         # trackings below Globex (not Globex itself)
#   [in_tree, '%Ticket%']         # LIKE match on task description
#
# Operators (aliases shown after each):
#   =        ==      eq          equal
#   !=       <>      ne          not equal
#   >                gt          greater than
#   >=       ge      gte         greater or equal
#   <                lt          less than
#   <=       le      lte         less or equal
#   like     not_like            SQL LIKE / NOT LIKE
#   has                          substring match (col LIKE '%value%')
#   is_null  is_not_null
#   in       not_in              list membership
#   in_tree  has_ancestor        tree filters (path-based)
#
# Combine with and/or/not:
#   and:
#     - [started_at, '>=', 'last monday']
#     - [in_tree, Globex]
#
# All tracking fields: id, task_id, predecessor_id, started_at, ended_at,
#   deleted, created_at
# All task fields (t.): id, description, status, deleted, deleted_at,
#   priority, parent_id, path, created_at, updated_at, last_tracked_at
"#
    .to_string()
}
