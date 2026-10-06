//! Languages: each one is a lexicon that rewrites a phrase into the crate's
//! canonical (English) vocabulary, which the [meanings](crate::Meaning) then
//! resolve. A language never computes a date itself — that is what keeps a new
//! language a matter of data and a new meaning a matter of one pass.
//!
//! Adding a language is one new file beside `en.rs`/`de.rs` that exposes a
//! `LEXICON` (and optionally a fold), plus one line in [`Language::builtin`].

mod de;
mod en;

/// One lexicon entry: a pattern in the language and its canonical rewrite.
///
/// Both sides are whitespace-separated tokens. A pattern token is either a
/// word (compared after the language's [fold](Language::with_fold)) or a
/// placeholder, whose captured text the rewrite may use by the same name:
///
/// | placeholder  | matches                     | captures       |
/// |--------------|-----------------------------|----------------|
/// | `{n}`, `{m}` | a whole number (`18`)       | the number     |
/// | `{t}`        | a clock time (`18:30`, `18.30`) | `18:30`    |
/// | `{d}`        | an ordinal day (`8.`)       | `8`            |
///
/// An empty rewrite drops the matched words (fillers such as German `um`).
pub type Entry = (&'static str, &'static str);

/// A language the resolver understands.
#[derive(Clone, Debug)]
pub struct Language {
    code: &'static str,
    lexicon: &'static [Entry],
    rules: Vec<Rule>,
    fold: fn(&str) -> String,
}

#[derive(Clone, Debug)]
struct Rule {
    pattern: Vec<Tok>,
    rewrite: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Num(&'static str),
    Clock(&'static str),
    Ordinal(&'static str),
}

impl Language {
    /// A language from its code (`"fr"`) and lexicon. Entries are tried
    /// longest first; among equally long matches the first listed wins.
    pub fn new(code: &'static str, lexicon: &'static [Entry]) -> Self {
        Self::build(code, lexicon, identity)
    }

    /// Replace the fold that both lexicon words and input tokens go through
    /// before they are compared (German folds `ä` → `ae`, so `naechste` and
    /// `nächste` are the same word).
    pub fn with_fold(self, fold: fn(&str) -> String) -> Self {
        Self::build(self.code, self.lexicon, fold)
    }

    /// English: the canonical vocabulary itself, so its lexicon is empty.
    pub fn english() -> Self {
        Self::new("en", en::LEXICON)
    }

    /// German (`morgen 18 Uhr`, `übermorgen`, `Ende nächster Woche`, `halb 3`).
    pub fn german() -> Self {
        Self::build("de", de::LEXICON, de::fold)
    }

    /// Every built-in language, in the order the default resolver tries them.
    /// A new language is registered here.
    pub fn builtin() -> Vec<Self> {
        vec![Self::english(), Self::german()]
    }

    /// The language's code (`"en"`, `"de"`).
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Rewrite a phrase into the canonical vocabulary. The phrase is
    /// lowercased and whitespace-collapsed first; tokens no entry matches are
    /// kept as they are, so mixed input (`morgen 5pm`) still resolves.
    pub fn rewrite(&self, phrase: &str) -> String {
        let lowered = phrase.to_lowercase();
        let tokens: Vec<&str> = lowered.split_whitespace().collect();
        let folded: Vec<String> = tokens.iter().map(|t| (self.fold)(t)).collect();
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            match self.longest_match(&tokens[i..], &folded[i..]) {
                Some((len, text)) => {
                    out.extend(text.split_whitespace().map(str::to_string));
                    i += len;
                }
                None => {
                    out.push(tokens[i].to_string());
                    i += 1;
                }
            }
        }
        out.join(" ")
    }

    fn build(code: &'static str, lexicon: &'static [Entry], fold: fn(&str) -> String) -> Self {
        let rules = lexicon
            .iter()
            .map(|(pattern, rewrite)| Rule {
                pattern: pattern
                    .split_whitespace()
                    .map(|t| match t {
                        "{n}" => Tok::Num("{n}"),
                        "{m}" => Tok::Num("{m}"),
                        "{t}" => Tok::Clock("{t}"),
                        "{d}" => Tok::Ordinal("{d}"),
                        w => Tok::Word(fold(w)),
                    })
                    .collect(),
                rewrite,
            })
            .collect();
        Self {
            code,
            lexicon,
            rules,
            fold,
        }
    }

    /// The longest rule matching at the start of `tokens`: how many tokens it
    /// consumed and its rewrite with the captures filled in.
    fn longest_match(&self, tokens: &[&str], folded: &[String]) -> Option<(usize, String)> {
        let mut best: Option<(usize, String)> = None;
        for rule in &self.rules {
            let len = rule.pattern.len();
            if len == 0 || len > tokens.len() || best.as_ref().is_some_and(|(l, _)| *l >= len) {
                continue;
            }
            let mut captures: Vec<(&str, String)> = Vec::new();
            let matched = rule.pattern.iter().enumerate().all(|(k, tok)| match tok {
                Tok::Word(w) => folded[k] == *w,
                Tok::Num(name) => number(tokens[k])
                    .map(|v| captures.push((name, v)))
                    .is_some(),
                Tok::Clock(name) => clock(tokens[k]).map(|v| captures.push((name, v))).is_some(),
                Tok::Ordinal(name) => ordinal(tokens[k])
                    .map(|v| captures.push((name, v)))
                    .is_some(),
            });
            if matched {
                let mut text = rule.rewrite.to_string();
                for (name, value) in captures {
                    text = text.replace(name, &value);
                }
                best = Some((len, text));
            }
        }
        best
    }
}

fn identity(s: &str) -> String {
    s.to_string()
}

fn number(t: &str) -> Option<String> {
    (!t.is_empty() && t.len() <= 4 && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.to_string())
}

fn clock(t: &str) -> Option<String> {
    let (h, m) = t.split_once(':').or_else(|| t.split_once('.'))?;
    let hour: u32 = number(h)?.parse().ok()?;
    let minute: u32 = number(m)?.parse().ok()?;
    (hour < 24 && minute < 60 && m.len() == 2).then(|| format!("{hour}:{m}"))
}

fn ordinal(t: &str) -> Option<String> {
    let day: u32 = number(t.strip_suffix('.')?)?.parse().ok()?;
    (1..=31).contains(&day).then(|| day.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_match_wins_and_placeholders_fill() {
        let de = Language::german();
        assert_eq!(de.rewrite("morgen früh"), "tomorrow morning");
        assert_eq!(de.rewrite("heute morgen"), "today morning");
        assert_eq!(de.rewrite("Morgen 18 Uhr"), "tomorrow 18:00");
        assert_eq!(de.rewrite("um 18.30 Uhr"), "18:30");
        assert_eq!(de.rewrite("viertel vor 5"), "quarter to 5");
        assert_eq!(de.rewrite("naechsten Montag"), "next monday");
        assert_eq!(de.rewrite("8. Oktober"), "8 october");
    }

    #[test]
    fn english_is_the_identity() {
        assert_eq!(
            Language::english().rewrite("Tomorrow  Morning"),
            "tomorrow morning"
        );
    }

    #[test]
    fn a_language_can_be_built_from_outside() {
        static FR: &[Entry] = &[("demain", "tomorrow"), ("{n} h", "{n}:00")];
        let fr = Language::new("fr", FR);
        assert_eq!(fr.rewrite("demain 18 h"), "tomorrow 18:00");
        assert_eq!(fr.code(), "fr");
    }
}
