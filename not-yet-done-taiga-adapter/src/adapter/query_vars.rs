//! Inline variable parsing for Taiga saved queries.
//!
//! Syntax: `${name:default}` or `${name}` (no default → required).
//! - `name` matches `[A-Za-z_][A-Za-z0-9_]*`.
//! - `default` is everything between the first `:` and the closing `}`,
//!   with `\}` as an escape for a literal `}`. Whitespace is preserved.
//! - Anything that doesn't parse cleanly is left in the output verbatim
//!   (so a stray `${` doesn't blow up the query).

use std::collections::HashMap;

use not_yet_done_content::QueryVariable;

/// Extract variables from a raw Taiga saved-query string. Returns one
/// `QueryVariable` per distinct `name` in source order (first occurrence
/// wins for the default).
pub fn parse_variables(query: &str) -> Vec<QueryVariable> {
    let mut out: Vec<QueryVariable> = Vec::new();
    for span in iter_placeholders(query) {
        if !out.iter().any(|v| v.name == span.name) {
            out.push(QueryVariable {
                name: span.name,
                default: span.default,
            });
        }
    }
    out
}

/// Substitute variables in `query` using `vars`. Missing variables fall
/// back to the inline default; if no default exists either, the literal
/// placeholder is left in place (caller is expected to validate
/// presence before invoking).
pub fn render(query: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(query.len());
    let mut cursor = 0;
    for span in iter_placeholders(query) {
        out.push_str(&query[cursor..span.start]);
        if let Some(val) = vars.get(&span.name) {
            out.push_str(val);
        } else if let Some(default) = &span.default {
            out.push_str(default);
        } else {
            out.push_str(&query[span.start..span.end]);
        }
        cursor = span.end;
    }
    out.push_str(&query[cursor..]);
    out
}

#[derive(Debug)]
struct PlaceholderSpan {
    start: usize,
    end: usize,
    name: String,
    default: Option<String>,
}

fn iter_placeholders(input: &str) -> Vec<PlaceholderSpan> {
    let bytes = input.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'{' {
            if let Some(span) = parse_placeholder(input, i) {
                let end = span.end;
                spans.push(span);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    spans
}

fn parse_placeholder(input: &str, start: usize) -> Option<PlaceholderSpan> {
    let bytes = input.as_bytes();
    debug_assert_eq!(bytes[start], b'$');
    debug_assert_eq!(bytes[start + 1], b'{');
    let name_start = start + 2;
    let mut p = name_start;
    while p < bytes.len() {
        let c = bytes[p];
        let valid = if p == name_start {
            (c as char).is_ascii_alphabetic() || c == b'_'
        } else {
            (c as char).is_ascii_alphanumeric() || c == b'_'
        };
        if !valid {
            break;
        }
        p += 1;
    }
    if p == name_start {
        return None;
    }
    let name = input[name_start..p].to_string();
    match bytes.get(p) {
        Some(&b'}') => Some(PlaceholderSpan {
            start,
            end: p + 1,
            name,
            default: None,
        }),
        Some(&b':') => {
            let value_start = p + 1;
            let value_end = find_unescaped_close(bytes, value_start)?;
            let raw = &input[value_start..value_end];
            Some(PlaceholderSpan {
                start,
                end: value_end + 1,
                name,
                default: Some(unescape(raw)),
            })
        }
        _ => None,
    }
}

fn find_unescaped_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut p = from;
    while p < bytes.len() {
        match bytes[p] {
            b'\\' if p + 1 < bytes.len() && bytes[p + 1] == b'}' => p += 2,
            b'}' => return Some(p),
            _ => p += 1,
        }
    }
    None
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'}') {
            out.push('}');
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_placeholders_passthrough() {
        let vars = HashMap::new();
        assert_eq!(render("status=open&project=42", &vars), "status=open&project=42");
        assert!(parse_variables("status=open&project=42").is_empty());
    }

    #[test]
    fn parses_name_and_default() {
        let v = parse_variables("project=${proj:alpha}&open=true");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "proj");
        assert_eq!(v[0].default.as_deref(), Some("alpha"));
    }

    #[test]
    fn parses_required_no_default() {
        let v = parse_variables("project=${proj}");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "proj");
        assert!(v[0].default.is_none());
    }

    #[test]
    fn substitutes_with_user_value() {
        let mut vars = HashMap::new();
        vars.insert("proj".to_string(), "beta".to_string());
        assert_eq!(render("project=${proj:alpha}", &vars), "project=beta");
    }

    #[test]
    fn substitutes_with_default_when_missing() {
        let vars = HashMap::new();
        assert_eq!(render("project=${proj:alpha}", &vars), "project=alpha");
    }

    #[test]
    fn leaves_placeholder_when_no_default_and_no_value() {
        let vars = HashMap::new();
        assert_eq!(render("project=${proj}", &vars), "project=${proj}");
    }

    #[test]
    fn dedupes_repeated_variables() {
        let v = parse_variables("a=${x:1}&b=${x}");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "x");
        assert_eq!(v[0].default.as_deref(), Some("1"));
    }

    #[test]
    fn ignores_malformed_placeholders() {
        assert!(parse_variables("foo=${}").is_empty());
        assert!(parse_variables("foo=${1abc}").is_empty());
        assert_eq!(render("foo=${}", &HashMap::new()), "foo=${}");
    }

    #[test]
    fn multiple_distinct_variables_in_order() {
        let v = parse_variables("a=${x:1}&b=${y:2}&c=${z}");
        assert_eq!(v.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), vec!["x", "y", "z"]);
    }

    #[test]
    fn escapes_closing_brace_in_default() {
        let v = parse_variables("re=${pat:foo\\}bar}");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].default.as_deref(), Some("foo}bar"));
    }

    #[test]
    fn render_substitutes_multiple_in_order() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), "X".to_string());
        vars.insert("y".to_string(), "Y".to_string());
        assert_eq!(
            render("a=${x:1}&b=${y:2}&c=${z:3}", &vars),
            "a=X&b=Y&c=3"
        );
    }
}
