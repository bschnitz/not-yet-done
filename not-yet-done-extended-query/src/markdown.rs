//! Splitting an extended-query document into its specification and its
//! library of named queries.
//!
//! Only fenced code blocks matter here; everything between them is prose the
//! user wrote for themselves and is discarded. The scanner is deliberately a
//! small CommonMark subset rather than a Markdown library: fences are the only
//! construct with any meaning in this format, and a dependency that also
//! parses tables, links and emphasis would buy nothing.
//!
//! The one CommonMark rule that has to be honoured is *fence length* — a
//! block opened with four backticks ends only at four backticks, so a document
//! can quote an example containing three-backtick fences without the scanner
//! mistaking the inner fences for real library entries.

/// One fenced code block, with its info string already split.
///
/// `line` is the 1-based line of the opening fence, so errors can point at the
/// place in the file the user is looking at.
#[derive(Debug, Clone, PartialEq)]
pub struct Fence {
    /// First word of the info string — the query language, not a highlighting
    /// hint. `None` when the info string was empty.
    pub language: Option<String>,
    /// Second word of the info string — the name `query-ref:` addresses.
    pub name: Option<String>,
    /// The block's content, without the fence lines.
    pub text: String,
    pub line: usize,
}

/// A document split into the parts the parser consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The first unnamed `yaml` fence: the query tree.
    pub spec: Fence,
    /// Every named fence, in document order.
    pub library: Vec<Fence>,
}

impl Document {
    /// The library entry with this name, if any.
    pub fn library_entry(&self, name: &str) -> Option<&Fence> {
        self.library
            .iter()
            .find(|f| f.name.as_deref() == Some(name))
    }

    /// Every library name, in document order — used to make an unresolved
    /// `query-ref` say what *is* available.
    pub fn library_names(&self) -> Vec<&str> {
        self.library
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarkdownError {
    #[error("line {line}: unterminated `{fence}` fence")]
    Unterminated { line: usize, fence: String },

    #[error("line {line}: a fence info string takes at most `<language> <name>`, found `{info}`")]
    InfoString { line: usize, info: String },

    #[error(
        "line {line}: duplicate fence name `{name}`, already declared at line {first} — \
         `query-ref: {name}` would be ambiguous"
    )]
    DuplicateName {
        name: String,
        line: usize,
        first: usize,
    },

    #[error(
        "no specification found: the document needs an unnamed ```yaml fence holding the \
         query tree"
    )]
    MissingSpec,

    #[error(
        "line {line}: a second unnamed `yaml` fence, the first is at line {first} — the \
         specification must be unique; give this one a name to make it a library entry"
    )]
    SecondSpec { line: usize, first: usize },
}

/// Split a document into specification and library.
///
/// Unnamed fences in a language other than `yaml` are ignored rather than
/// rejected: they are how a user illustrates something in the prose. An
/// unnamed *`yaml`* fence, on the other hand, is either the specification or a
/// mistake, so a second one is an error.
pub fn split(source: &str) -> Result<Document, MarkdownError> {
    let lines: Vec<&str> = source.lines().collect();
    let mut spec: Option<Fence> = None;
    let mut library: Vec<Fence> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let Some(opener) = fence_opener(lines[i]) else {
            i += 1;
            continue;
        };
        let line_no = i + 1;
        let (language, name) = split_info(&opener.info, line_no)?;

        let mut end = None;
        for (offset, candidate) in lines[i + 1..].iter().enumerate() {
            if closes(candidate, &opener) {
                end = Some(i + 1 + offset);
                break;
            }
        }
        let Some(end) = end else {
            return Err(MarkdownError::Unterminated {
                line: line_no,
                fence: opener.marker.to_string().repeat(opener.width),
            });
        };

        let fence = Fence {
            language,
            name,
            text: dedent(&lines[i + 1..end], opener.indent),
            line: line_no,
        };

        if let Some(name) = fence.name.as_deref() {
            if let Some(previous) = library.iter().find(|f| f.name.as_deref() == Some(name)) {
                return Err(MarkdownError::DuplicateName {
                    name: name.to_string(),
                    line: fence.line,
                    first: previous.line,
                });
            }
            library.push(fence);
        } else if fence
            .language
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("yaml"))
        {
            if let Some(first) = &spec {
                return Err(MarkdownError::SecondSpec {
                    line: fence.line,
                    first: first.line,
                });
            }
            spec = Some(fence);
        }

        i = end + 1;
    }

    Ok(Document {
        spec: spec.ok_or(MarkdownError::MissingSpec)?,
        library,
    })
}

/// An opening fence: its marker character, run length, indentation and info
/// string. Closing fences must match the marker and be at least as long.
struct Opener {
    marker: char,
    width: usize,
    indent: usize,
    info: String,
}

fn fence_opener(line: &str) -> Option<Opener> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = rest.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let width = rest.chars().take_while(|c| *c == marker).count();
    if width < 3 {
        return None;
    }
    let info = rest[width..].trim().to_string();
    // CommonMark: a backtick fence's info string may not contain a backtick,
    // which is what keeps inline code spans from opening a block.
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some(Opener {
        marker,
        width,
        indent,
        info,
    })
}

fn closes(line: &str, opener: &Opener) -> bool {
    let indent = leading_spaces(line);
    if indent > 3 {
        return false;
    }
    let rest = line[indent..].trim_end();
    !rest.is_empty()
        && rest.chars().all(|c| c == opener.marker)
        && rest.chars().count() >= opener.width
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Strip the opening fence's indentation from each content line — no more, so
/// deeper indentation inside a block is preserved (it is significant in YAML).
fn dedent(lines: &[&str], indent: usize) -> String {
    let mut out = String::new();
    for line in lines {
        let strip = leading_spaces(line).min(indent);
        out.push_str(&line[strip..]);
        out.push('\n');
    }
    out
}

fn split_info(info: &str, line: usize) -> Result<(Option<String>, Option<String>), MarkdownError> {
    let mut words = info.split_whitespace();
    let language = words.next().map(str::to_string);
    let name = words.next().map(str::to_string);
    if words.next().is_some() {
        return Err(MarkdownError::InfoString {
            line,
            info: info.to_string(),
        });
    }
    Ok((language, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> &'static str {
        "\
Some prose.

```yaml
and:
  - query-ref: mine
```

More prose.

```jql mine
assignee = currentUser()
```
"
    }

    #[test]
    fn spec_is_the_unnamed_yaml_fence_and_named_fences_form_the_library() {
        let d = split(doc()).unwrap();
        assert_eq!(d.spec.language.as_deref(), Some("yaml"));
        assert_eq!(d.spec.line, 3);
        assert_eq!(d.spec.text, "and:\n  - query-ref: mine\n");
        assert_eq!(d.library_names(), vec!["mine"]);
        assert_eq!(
            d.library_entry("mine").unwrap().text,
            "assignee = currentUser()\n"
        );
        assert_eq!(
            d.library_entry("mine").unwrap().language.as_deref(),
            Some("jql")
        );
    }

    #[test]
    fn a_longer_fence_swallows_shorter_ones() {
        // A quoted example must not contribute library entries — otherwise a
        // document that documents itself would fail to parse.
        let src = "\
````markdown
```jql mine
example
```
````

```yaml
query: real
```
";
        let d = split(src).unwrap();
        assert_eq!(d.spec.text, "query: real\n");
        assert!(d.library.is_empty(), "quoted fences must not register");
    }

    #[test]
    fn unnamed_non_yaml_fences_are_prose_and_ignored() {
        let src = "```sh\ncargo build\n```\n\n```yaml\nquery: x\n```\n";
        let d = split(src).unwrap();
        assert_eq!(d.spec.text, "query: x\n");
        assert!(d.library.is_empty());
    }

    #[test]
    fn indented_fence_content_keeps_its_relative_indentation() {
        let src = "  ```yaml\n  or:\n    - query: a\n  ```\n";
        let d = split(src).unwrap();
        assert_eq!(d.spec.text, "or:\n  - query: a\n");
    }

    #[test]
    fn errors_are_reported_with_line_numbers() {
        let cases: Vec<(&str, MarkdownError)> = vec![
            (
                "```yaml\nquery: a\n",
                MarkdownError::Unterminated {
                    line: 1,
                    fence: "```".into(),
                },
            ),
            (
                "```yaml\nquery: a\n```\n\n```jql one two\nx\n```\n",
                MarkdownError::InfoString {
                    line: 5,
                    info: "jql one two".into(),
                },
            ),
            (
                "```yaml\nquery: a\n```\n\n```jql dup\nx\n```\n\n```jql dup\ny\n```\n",
                MarkdownError::DuplicateName {
                    name: "dup".into(),
                    line: 9,
                    first: 5,
                },
            ),
            ("no fences here\n", MarkdownError::MissingSpec),
            (
                "```yaml\nquery: a\n```\n\n```yaml\nquery: b\n```\n",
                MarkdownError::SecondSpec { line: 5, first: 1 },
            ),
        ];
        for (src, want) in cases {
            assert_eq!(split(src).unwrap_err(), want, "source: {src:?}");
        }
    }

    #[test]
    fn tilde_fences_work_and_may_contain_backticks() {
        let src = "~~~yaml\nquery: \"a ` b\"\n~~~\n";
        let d = split(src).unwrap();
        assert_eq!(d.spec.text, "query: \"a ` b\"\n");
    }
}
