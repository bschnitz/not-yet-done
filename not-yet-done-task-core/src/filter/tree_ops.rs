//! Pre-processing step that resolves the `has_ancestor` and `in_tree`
//! predicates.
//!
//! They are host predicates to the filter language ([`FilterExpr::Custom`]),
//! which carries them through untouched because only this application knows
//! what a task hierarchy is. Resolving one means a database lookup for tasks
//! matching the description, then rewriting the predicate into `path LIKE`
//! conditions over their short IDs.
//!
//! Must be called **before** passing the FilterExpr to the FilterBuilder — it
//! is the step that turns something SQL cannot express into something it can.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::task;
use crate::error::AppError;
use crate::repository::task_short_id;
use not_yet_done_filter::{
    ColRef, FilterExpr, FilterLeaf, HAS_ANCESTOR, IN_TREE, Literal, Operator, Rhs,
};

/// Resolve every `has_ancestor` / `in_tree` predicate in a FilterExpr.
///
/// For each one, queries the database for tasks matching the description
/// (exact or LIKE), collects their short IDs, and rewrites the predicate into
/// an OR of `path LIKE` conditions.
pub fn resolve_tree_operators<'a>(
    expr: &'a FilterExpr,
    db: &'a DatabaseConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<FilterExpr, AppError>> + Send + 'a>>
{
    Box::pin(async move {
        match expr {
            FilterExpr::And(children) => {
                let mut resolved = Vec::with_capacity(children.len());
                for child in children {
                    resolved.push(resolve_tree_operators(child, db).await?);
                }
                Ok(FilterExpr::And(resolved))
            }
            FilterExpr::Or(children) => {
                let mut resolved = Vec::with_capacity(children.len());
                for child in children {
                    resolved.push(resolve_tree_operators(child, db).await?);
                }
                Ok(FilterExpr::Or(resolved))
            }
            FilterExpr::Not(inner) => Ok(FilterExpr::Not(Box::new(
                resolve_tree_operators(inner, db).await?,
            ))),
            FilterExpr::Leaf(_) => Ok(expr.clone()),
            FilterExpr::Custom { name, arg } if name == HAS_ANCESTOR || name == IN_TREE => {
                resolve_tree_predicate(name, arg, db).await
            }
            // Every other predicate is a load error already — `parse` checks
            // the names against the ones this application implements — so
            // reaching here means a filter built in code, and passing it on
            // unchanged lets the builder say what it cannot translate.
            FilterExpr::Custom { .. } => Ok(expr.clone()),
        }
    })
}

async fn resolve_tree_predicate(
    name: &str,
    arg: &Literal,
    db: &DatabaseConnection,
) -> Result<FilterExpr, AppError> {
    let Literal::String(search_str) = arg else {
        return Err(AppError::FilterError(
            "has_ancestor / in_tree requires a string value".into(),
        ));
    };

    // Find matching tasks by description — exact or LIKE if contains %.
    let matching_tasks: Vec<task::Model> = if search_str.contains('%') {
        task::Entity::find()
            .filter(task::Column::Description.like(search_str))
            .all(db)
            .await?
    } else {
        task::Entity::find()
            .filter(task::Column::Description.eq(search_str))
            .all(db)
            .await?
    };

    if matching_tasks.is_empty() {
        // No matches → condition that's always false.
        return Ok(FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("path"),
            op: Operator::Eq,
            rhs: Rhs::Lit(Literal::String("__no_match__".into())),
        }));
    }

    // Build OR of path LIKE conditions.
    let mut conditions = Vec::new();
    for task in &matching_tasks {
        let sid = task_short_id(task.id);
        let pattern = if name == HAS_ANCESTOR {
            // The task must be BELOW the matched node: the trailing `/_`
            // demands at least one more segment after it.
            format!("%/{sid}/_%")
        } else {
            // in_tree: the node itself, or anything below it.
            format!("%/{sid}%")
        };
        conditions.push(FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("path"),
            op: Operator::Like,
            rhs: Rhs::Lit(Literal::String(pattern)),
        }));
    }

    if conditions.len() == 1 {
        Ok(conditions.into_iter().next().unwrap())
    } else {
        Ok(FilterExpr::Or(conditions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_ancestor_needs_string() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
            resolve_tree_predicate(HAS_ANCESTOR, &Literal::Int(42), &db).await
        });
        assert!(result.is_err());
    }
}
