//! A tiny expression evaluator for route **guards** (Phase 6b2).
//!
//! A step's routing can carry a raw guard expression (stored verbatim in
//! [`RouteCondition::Expr`]) that is evaluated against the *outcome* of running
//! that step — its exit code, whether it succeeded, and its captured streams.
//! The convenience conditions `on_success` / `on_failure` cover the common case;
//! this evaluator handles everything else, e.g.
//!
//! ```text
//! exit == 0:        deploy
//! exit != 0:        rollback
//! stdout contains WARN:  review
//! success == true:  next
//! ```
//!
//! It is deliberately **not** [`crate::model`]'s filter DSL nor SQL: guards run
//! in-memory against one step's [`GuardVars`], so this is a self-contained
//! `<variable> <operator> <value>` grammar with no dependencies.
//!
//! # Grammar
//!
//! A guard is exactly three tokens: a **variable**, an **operator**, and a
//! **value** literal (whitespace is optional, so `exit==0` and `exit == 0` are
//! the same).
//!
//! * Variables: `exit` (integer, absent if the process was signalled or the step
//!   ran no command), `success` (bool), `stdout`, `stderr` (text).
//! * Operators: `==` `!=` `>` `>=` `<` `<=` and the word `contains`.
//! * Values: an integer, `true`/`false`, or a string (bare word or quoted with
//!   `"`/`'`). A quoted string may contain spaces.
//!
//! Numeric operators (`>` etc.) apply to `exit` only; `contains` applies to text
//! only; `==` / `!=` apply to every variable. A guard that names an unknown
//! variable, uses an operator the variable does not support, or is malformed
//! returns [`Err`] with a short reason — the caller records that and treats the
//! guard as *not matched* rather than silently taking it.
//!
//! [`RouteCondition::Expr`]: crate::model::RouteCondition::Expr

/// The outcome variables a guard is evaluated against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuardVars {
    /// The process exit code, or `None` when the step ran no command (manual /
    /// skipped) or the process was terminated by a signal.
    pub exit: Option<i32>,
    /// Whether the step settled as a success (`done` or `skipped`).
    pub success: bool,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

/// Evaluate a guard expression against `vars`.
///
/// Returns `Ok(true)` when the guard holds, `Ok(false)` when it does not (this
/// includes an `exit` comparison when no exit code is available), and `Err` with
/// a short human reason when the guard is malformed or nonsensical.
pub fn eval(expr: &str, vars: &GuardVars) -> Result<bool, String> {
    let toks = tokenize(expr)?;
    if toks.len() != 3 {
        return Err(format!(
            "expected `<variable> <operator> <value>`, got {} token(s)",
            toks.len()
        ));
    }
    let var = match &toks[0] {
        Tok::Word(w) => Var::parse(w)?,
        _ => return Err("the left side must be a variable".into()),
    };
    let op = match &toks[1] {
        Tok::Op(o) => *o,
        _ => return Err("expected an operator (== != > >= < <= contains)".into()),
    };
    let rhs = match &toks[2] {
        Tok::Word(w) => Lit::parse(w),
        Tok::Str(s) => Lit::Str(s.clone()),
        Tok::Op(_) => return Err("the right side must be a value".into()),
    };

    match var {
        Var::Exit => eval_exit(vars.exit, op, &rhs),
        Var::Success => eval_success(vars.success, op, &rhs),
        Var::Stdout => eval_text(&vars.stdout, op, &rhs),
        Var::Stderr => eval_text(&vars.stderr, op, &rhs),
    }
}

fn eval_exit(exit: Option<i32>, op: Op, rhs: &Lit) -> Result<bool, String> {
    let rhs = match rhs {
        Lit::Num(n) => *n,
        _ => return Err("`exit` compares against a number".into()),
    };
    // No exit code (manual/skipped step or a signal): the guard cannot hold.
    let Some(exit) = exit else { return Ok(false) };
    let exit = exit as i64;
    match op {
        Op::Eq => Ok(exit == rhs),
        Op::Ne => Ok(exit != rhs),
        Op::Gt => Ok(exit > rhs),
        Op::Ge => Ok(exit >= rhs),
        Op::Lt => Ok(exit < rhs),
        Op::Le => Ok(exit <= rhs),
        Op::Contains => Err("`contains` does not apply to `exit`".into()),
    }
}

fn eval_success(success: bool, op: Op, rhs: &Lit) -> Result<bool, String> {
    let rhs = match rhs {
        Lit::Bool(b) => *b,
        _ => return Err("`success` compares against true/false".into()),
    };
    match op {
        Op::Eq => Ok(success == rhs),
        Op::Ne => Ok(success != rhs),
        _ => Err("`success` supports only == and !=".into()),
    }
}

fn eval_text(text: &str, op: Op, rhs: &Lit) -> Result<bool, String> {
    let rhs = rhs.as_text();
    match op {
        Op::Contains => Ok(text.contains(&rhs)),
        Op::Eq => Ok(text.trim() == rhs),
        Op::Ne => Ok(text.trim() != rhs),
        _ => Err("text supports only ==, != and contains".into()),
    }
}

/// A known guard variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Var {
    Exit,
    Success,
    Stdout,
    Stderr,
}

impl Var {
    fn parse(w: &str) -> Result<Self, String> {
        match w {
            "exit" => Ok(Var::Exit),
            "success" => Ok(Var::Success),
            "stdout" => Ok(Var::Stdout),
            "stderr" => Ok(Var::Stderr),
            other => Err(format!(
                "unknown variable `{other}` (expected exit/success/stdout/stderr)"
            )),
        }
    }
}

/// A comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
}

/// A right-hand-side literal.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Lit {
    Num(i64),
    Bool(bool),
    Str(String),
}

impl Lit {
    /// Parse a bare (unquoted) word into the most specific literal it fits.
    fn parse(w: &str) -> Self {
        if let Ok(n) = w.parse::<i64>() {
            Lit::Num(n)
        } else if w == "true" {
            Lit::Bool(true)
        } else if w == "false" {
            Lit::Bool(false)
        } else {
            Lit::Str(w.to_string())
        }
    }

    /// The literal as text (numbers/bools stringify) for text comparisons.
    fn as_text(&self) -> String {
        match self {
            Lit::Num(n) => n.to_string(),
            Lit::Bool(b) => b.to_string(),
            Lit::Str(s) => s.clone(),
        }
    }
}

/// A lexical token: an unquoted word, a quoted string, or an operator.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(String),
    Str(String),
    Op(Op),
}

/// Split a guard expression into tokens. Operators need no surrounding
/// whitespace; quoted strings (`"`/`'`) may hold spaces; the word `contains` is
/// recognised as an operator.
fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let start = i;
            while i < cs.len() && cs[i] != quote {
                i += 1;
            }
            if i >= cs.len() {
                return Err("unterminated string literal".into());
            }
            toks.push(Tok::Str(cs[start..i].iter().collect()));
            i += 1;
            continue;
        }
        if matches!(c, '=' | '!' | '<' | '>') {
            let next = cs.get(i + 1).copied();
            let (op, len) = match (c, next) {
                ('=', Some('=')) => (Op::Eq, 2),
                ('!', Some('=')) => (Op::Ne, 2),
                ('>', Some('=')) => (Op::Ge, 2),
                ('<', Some('=')) => (Op::Le, 2),
                ('>', _) => (Op::Gt, 1),
                ('<', _) => (Op::Lt, 1),
                ('=', _) => return Err("use `==` for equality".into()),
                ('!', _) => return Err("use `!=` for inequality".into()),
                _ => unreachable!(),
            };
            toks.push(Tok::Op(op));
            i += len;
            continue;
        }
        // A bare word runs until whitespace, a quote, or an operator character.
        let start = i;
        while i < cs.len()
            && !matches!(cs[i], ' ' | '\t' | '"' | '\'' | '=' | '!' | '<' | '>')
            && !cs[i].is_whitespace()
        {
            i += 1;
        }
        let w: String = cs[start..i].iter().collect();
        if w == "contains" {
            toks.push(Tok::Op(Op::Contains));
        } else {
            toks.push(Tok::Word(w));
        }
    }
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(exit: Option<i32>, success: bool, stdout: &str, stderr: &str) -> GuardVars {
        GuardVars {
            exit,
            success,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn exit_numeric_comparisons() {
        let v = vars(Some(0), true, "", "");
        assert_eq!(eval("exit == 0", &v), Ok(true));
        assert_eq!(eval("exit != 0", &v), Ok(false));
        assert_eq!(eval("exit >= 0", &v), Ok(true));
        assert_eq!(eval("exit > 0", &v), Ok(false));
        let v = vars(Some(3), false, "", "");
        assert_eq!(eval("exit > 0", &v), Ok(true));
        assert_eq!(eval("exit <= 2", &v), Ok(false));
    }

    #[test]
    fn no_whitespace_is_fine() {
        let v = vars(Some(2), false, "", "");
        assert_eq!(eval("exit==2", &v), Ok(true));
        assert_eq!(eval("exit>=2", &v), Ok(true));
    }

    #[test]
    fn missing_exit_never_matches() {
        let v = vars(None, true, "", "");
        assert_eq!(eval("exit == 0", &v), Ok(false));
        assert_eq!(eval("exit != 0", &v), Ok(false));
    }

    #[test]
    fn success_boolean() {
        let v = vars(None, true, "", "");
        assert_eq!(eval("success == true", &v), Ok(true));
        assert_eq!(eval("success != true", &v), Ok(false));
        assert_eq!(eval("success == false", &v), Ok(false));
    }

    #[test]
    fn text_contains_and_equals() {
        let v = vars(Some(0), true, "build WARN: deprecated\n", "");
        assert_eq!(eval("stdout contains WARN", &v), Ok(true));
        assert_eq!(eval("stdout contains nope", &v), Ok(false));
        let v = vars(Some(0), true, "ready\n", "boom");
        assert_eq!(eval("stdout == ready", &v), Ok(true));
        assert_eq!(eval("stderr contains boom", &v), Ok(true));
    }

    #[test]
    fn quoted_string_with_spaces() {
        let v = vars(Some(0), true, "all systems go", "");
        assert_eq!(eval("stdout contains \"systems go\"", &v), Ok(true));
        assert_eq!(eval("stdout == 'all systems go'", &v), Ok(true));
    }

    #[test]
    fn errors_are_reported_not_matched() {
        let v = vars(Some(0), true, "", "");
        assert!(eval("nope == 1", &v).is_err());
        assert!(eval("exit contains 0", &v).is_err());
        assert!(eval("success > 1", &v).is_err());
        assert!(eval("exit == foo", &v).is_err());
        assert!(eval("exit ==", &v).is_err());
        assert!(eval("exit = 0", &v).is_err());
    }
}
