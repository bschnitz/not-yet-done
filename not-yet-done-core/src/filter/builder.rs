use sea_orm::{
    sea_query::{ColumnRef as SqColumnRef, Expr, SimpleExpr, Value},
    Condition, ExprTrait,
};

use crate::error::AppError;
use crate::filter::expr::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};

// ---------------------------------------------------------------------------
// ColumnRegistry trait
// ---------------------------------------------------------------------------

/// Maps a [`ColRef`] to a SeaQuery [`SqColumnRef`].
///
/// For single-entity repositories the `table` field is typically ignored.
/// For join queries (future), it selects the right alias.
///
/// Implementations are generated automatically via `#[derive(ColumnRegistry)]`
/// in `not-yet-done-macros`.
pub trait ColumnRegistry {
    fn resolve(&self, table: Option<&str>, column: &str) -> Option<SqColumnRef>;
}

// ---------------------------------------------------------------------------
// FilterBuilder
// ---------------------------------------------------------------------------

pub struct FilterBuilder<'r, R: ColumnRegistry> {
    registry: &'r R,
}

impl<'r, R: ColumnRegistry> FilterBuilder<'r, R> {
    pub fn new(registry: &'r R) -> Self {
        Self { registry }
    }

    /// Build a [`Condition`] from a [`FilterExpr`].
    pub fn build(&self, expr: &FilterExpr) -> Result<Condition, AppError> {
        self.build_expr(expr)
    }

    fn build_expr(&self, expr: &FilterExpr) -> Result<Condition, AppError> {
        match expr {
            FilterExpr::And(children) => {
                let mut cond = Condition::all();
                for child in children {
                    cond = cond.add(self.build_expr(child)?);
                }
                Ok(cond)
            }
            FilterExpr::Or(children) => {
                let mut cond = Condition::any();
                for child in children {
                    cond = cond.add(self.build_expr(child)?);
                }
                Ok(cond)
            }
            FilterExpr::Not(inner) => {
                let inner_cond = self.build_expr(inner)?;
                Ok(Condition::all().not().add(inner_cond))
            }
            FilterExpr::Leaf(leaf) => self.build_leaf(leaf),
        }
    }

    fn build_leaf(&self, leaf: &FilterLeaf) -> Result<Condition, AppError> {
        let lhs = self.col_ref_to_expr(&leaf.lhs)?;

        let simple: SimpleExpr = match &leaf.op {
            // Null-checks — no rhs needed
            Operator::IsNull    => lhs.is_null(),
            Operator::IsNotNull => lhs.is_not_null(),

            op => match &leaf.rhs {
                // Column-vs-column
                Rhs::Col(col_ref) => {
                    let rhs = self.col_ref_to_expr(col_ref)?;
                    self.apply_op_col(op, lhs, rhs)?
                }
                // Column-vs-literal
                Rhs::Lit(lit) => {
                    self.apply_op_lit(op, lhs, lit)?
                }
                Rhs::None => return Err(AppError::FilterError(format!(
                    "operator {op:?} requires a right-hand side"
                ))),
            },
        };

        Ok(Condition::all().add(simple))
    }

    /// Apply a binary operator where the rhs is another column expression.
    fn apply_op_col(
        &self,
        op: &Operator,
        lhs: Expr,
        rhs: Expr,
    ) -> Result<SimpleExpr, AppError> {
        // Use sea_query's binary() to avoid collisions with PartialEq::eq / PartialOrd::lt etc.
        use sea_orm::sea_query::{BinOper};
        let bin_oper = match op {
            Operator::Eq  => BinOper::Equal,
            Operator::Ne  => BinOper::NotEqual,
            Operator::Gt  => BinOper::GreaterThan,
            Operator::Gte => BinOper::GreaterThanOrEqual,
            Operator::Lt  => BinOper::SmallerThan,
            Operator::Lte => BinOper::SmallerThanOrEqual,
            other => return Err(AppError::FilterError(format!(
                "operator {other:?} is not supported for column-vs-column comparisons"
            ))),
        };
        Ok(lhs.binary(bin_oper, rhs))
    }

    /// Apply a binary operator where the rhs is a literal value.
    fn apply_op_lit(
        &self,
        op: &Operator,
        lhs: Expr,
        lit: &Literal,
    ) -> Result<SimpleExpr, AppError> {
        use sea_orm::sea_query::BinOper;
        match op {
            // Comparison operators — go through BinOper to avoid PartialEq/PartialOrd collisions
            Operator::Eq  => Ok(lhs.binary(BinOper::Equal,              self.lit_to_value(lit)?)),
            Operator::Ne  => Ok(lhs.binary(BinOper::NotEqual,           self.lit_to_value(lit)?)),
            Operator::Gt  => Ok(lhs.binary(BinOper::GreaterThan,        self.lit_to_value(lit)?)),
            Operator::Gte => Ok(lhs.binary(BinOper::GreaterThanOrEqual, self.lit_to_value(lit)?)),
            Operator::Lt  => Ok(lhs.binary(BinOper::SmallerThan,        self.lit_to_value(lit)?)),
            Operator::Lte => Ok(lhs.binary(BinOper::SmallerThanOrEqual, self.lit_to_value(lit)?)),
            // String operators — ExprTrait methods (no collision risk)
            Operator::Like    => Ok(lhs.like(self.lit_to_string(lit)?)),
            Operator::NotLike => Ok(lhs.not_like(self.lit_to_string(lit)?)),
            // Set operators
            Operator::In    => Ok(lhs.is_in(self.lit_to_value_list(lit)?)),
            Operator::NotIn => Ok(lhs.is_not_in(self.lit_to_value_list(lit)?)),
            // Null-checks handled above, never reach here
            Operator::IsNull | Operator::IsNotNull => unreachable!(),
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn col_ref_to_expr(&self, col: &ColRef) -> Result<Expr, AppError> {
        let sq_col = self
            .registry
            .resolve(col.table.as_deref(), &col.column)
            .ok_or_else(|| {
                let name = match &col.table {
                    Some(t) => format!("{t}.{}", col.column),
                    None    => format!(".{}", col.column),
                };
                AppError::FilterError(format!("unknown column reference: '{name}'"))
            })?;
        Ok(Expr::col(sq_col))
    }

    fn lit_to_value(&self, lit: &Literal) -> Result<Value, AppError> {
        match lit {
            // SeaORM 2.x: Value::String(Option<String>) — no Box
            Literal::String(s) => Ok(Value::String(Some(s.clone()))),
            Literal::Int(i)    => Ok(Value::BigInt(Some(*i))),
            Literal::Float(f)  => Ok(Value::Double(Some(*f))),
            Literal::Bool(b)   => Ok(Value::Bool(Some(*b))),
            Literal::List(_)   => Err(AppError::FilterError(
                "a list literal is not valid as a scalar value".to_string(),
            )),
        }
    }

    fn lit_to_string(&self, lit: &Literal) -> Result<String, AppError> {
        match lit {
            Literal::String(s) => Ok(s.clone()),
            other => Err(AppError::FilterError(format!(
                "LIKE / NOT LIKE requires a string literal, got: {other:?}"
            ))),
        }
    }

    fn lit_to_value_list(&self, lit: &Literal) -> Result<Vec<Value>, AppError> {
        match lit {
            Literal::List(items) => items.iter().map(|i| self.lit_to_value(i)).collect(),
            single => Ok(vec![self.lit_to_value(single)?]),
        }
    }
}
