//! Pre-processing step that resolves `has_ancestor` and `in_tree` operators.
//!
//! These operators require a database lookup to find matching task IDs by
//! description, then rewrite the filter into `path LIKE` conditions.
//!
//! Must be called **before** passing the FilterExpr to the FilterBuilder.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::entity::task;
use crate::error::AppError;
use crate::repository::task_short_id;
use super::expr::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};

/// Resolve all `has_ancestor` and `in_tree` operators in a FilterExpr.
///
/// For each such leaf, queries the database for tasks matching the
/// description (exact or LIKE), collects their short IDs, and rewrites
/// the leaf into an OR of `path LIKE` conditions.
pub fn resolve_tree_operators<'a>(
    expr: &'a FilterExpr,
    db: &'a DatabaseConnection,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<FilterExpr, AppError>> + Send + 'a>> {
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
            FilterExpr::Not(inner) => {
                Ok(FilterExpr::Not(Box::new(
                    resolve_tree_operators(inner, db).await?,
                )))
            }
            FilterExpr::Leaf(leaf) => {
                match leaf.op {
                    Operator::HasAncestor | Operator::InTree => {
                        resolve_tree_leaf(leaf, db).await
                    }
                    _ => Ok(expr.clone()),
                }
            }
        }
    })
}

async fn resolve_tree_leaf(
    leaf: &FilterLeaf,
    db: &DatabaseConnection,
) -> Result<FilterExpr, AppError> {
    let search_str = match &leaf.rhs {
        Rhs::Lit(Literal::String(s)) => s.clone(),
        _ => return Err(AppError::FilterError(
            "has_ancestor / in_tree requires a string value".into(),
        )),
    };

    // Find matching tasks by description — exact or LIKE if contains %.
    let matching_tasks: Vec<task::Model> = if search_str.contains('%') {
        task::Entity::find()
            .filter(task::Column::Description.like(&search_str))
            .all(db)
            .await?
    } else {
        task::Entity::find()
            .filter(task::Column::Description.eq(&search_str))
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
        let pattern = match leaf.op {
            // has_ancestor: task must be BELOW the matched node.
            // path LIKE '%/<sid>/_%' — the /_ ensures at least one more segment after.
            Operator::HasAncestor => format!("%/{sid}/_%"),
            // in_tree: task is the node OR below it.
            // path LIKE '%/<sid>%'
            Operator::InTree => format!("%/{sid}%"),
            _ => unreachable!(),
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
        let leaf = FilterLeaf {
            lhs: ColRef::unqualified("ignored"),
            op: Operator::HasAncestor,
            rhs: Rhs::Lit(Literal::Int(42)),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
            resolve_tree_leaf(&leaf, &db).await
        });
        assert!(result.is_err());
    }
}
