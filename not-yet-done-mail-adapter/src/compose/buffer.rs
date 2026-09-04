//! The editor buffer: what the user sees, and what comes back.
//!
//! A composed message is edited as one text file — a small header block, a
//! blank line, the message as Markdown, and (for a reply) the original below a
//! marker line. Everything in this module is string work: no MIME, no network,
//! no adapter state. That is deliberate. The header block and the quote guard
//! are the two places where a mistake costs the user text they have written,
//! and they are the two cheapest things here to test.

use crate::error::{MailError, MailResult};

/// Separates the message from the quoted original.
///
/// It reads as an instruction because it is one: the text below it is *not*
/// what gets sent — the original's own markup is (see [`QuoteState`]).
pub(crate) const QUOTE_MARKER: &str =
    ">>> quoted original — sent verbatim as the sender wrote it; edits below are discarded";

/// The headers a composed message may carry. Deliberately five and not
/// "whatever the user typed": a buffer is not a place to inject `Bcc`-alikes
/// or a `Reply-To` we would then have to reason about at send time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Headers {
    /// The sending identity. Present even with one account: it says who is
    /// about to send, and in a multi-account instance it is what decides
    /// *which* mailbox does.
    pub(crate) from: String,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
}

/// What happened to the quoted region between the template and the save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuoteState {
    /// Untouched: the original's own HTML is what gets quoted.
    Intact,
    /// The marker line is gone. Two readings, one behaviour: the user either
    /// deleted the quote outright (nothing is quoted) or deleted just the
    /// marker and kept the text — in which case those `> …` lines are now
    /// ordinary Markdown in the body and travel as the user's own words.
    /// Both are deliberate gestures, and both do what they look like.
    Dropped,
    /// The marker is there and the text below it is not the text we wrote.
    /// Refused: sending would silently throw those edits away.
    Modified,
}

/// One parsed buffer.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub(crate) headers: Headers,
    /// The message itself, as Markdown, without the quoted region.
    pub(crate) body: String,
    pub(crate) quote: QuoteState,
}

/// Build the buffer the editor opens on.
pub(crate) fn render(headers: &Headers, body: &str, quote: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!("From: {}\n", headers.from));
    out.push_str(&format!("To: {}\n", headers.to.join(", ")));
    out.push_str(&format!("Cc: {}\n", headers.cc.join(", ")));
    if !headers.bcc.is_empty() {
        out.push_str(&format!("Bcc: {}\n", headers.bcc.join(", ")));
    }
    out.push_str(&format!("Subject: {}\n", headers.subject));
    out.push('\n');
    out.push_str(body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }
    if let Some(quote) = quote {
        out.push('\n');
        out.push_str(QUOTE_MARKER);
        out.push('\n');
        out.push_str(quote);
        if !quote.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Read a saved buffer back, judging the quoted region against the template
/// the editor was opened with.
///
/// `template` is what [`render`] produced — the frontend hands it back with
/// the edit, so no state has to be kept between opening the editor and saving
/// it.
pub(crate) fn parse(text: &str, template: &str) -> MailResult<Draft> {
    let (head, rest) = split_head(text)?;
    let headers = parse_headers(head)?;
    let (body, quote) = split_quote(rest);
    let quote_state = match (
        quote,
        split_quote(split_head(template).map(|(_, r)| r).unwrap_or("")).1,
    ) {
        (None, _) => QuoteState::Dropped,
        (Some(now), Some(before)) if now.trim_end() == before.trim_end() => QuoteState::Intact,
        // A template that carried no quote cannot have had one edited: a
        // marker the user typed themselves is text, not a promise we made.
        (Some(_), None) => QuoteState::Dropped,
        (Some(_), Some(_)) => QuoteState::Modified,
    };
    Ok(Draft {
        headers,
        body: body.trim_end().to_string(),
        quote: quote_state,
    })
}

/// Read a saved buffer back, judging its quoted region against a quote
/// rendered fresh from the original.
///
/// The difference to [`parse`] is which baseline the guard uses, and it
/// matters exactly once: a draft that is resumed *is* the template the editor
/// was handed, so an edit that was refused before would read as untouched the
/// second time round and travel as the sender's own words. The original is
/// re-read when a reply is sent anyway, so the quote can simply be rendered
/// again — no state has to survive between the two.
pub(crate) fn parse_with_quote(text: &str, quoted: Option<&str>) -> MailResult<Draft> {
    let baseline = render(&Headers::default(), "", quoted);
    parse(text, &baseline)
}

/// Split at the blank line that ends the header block.
fn split_head(text: &str) -> MailResult<(&str, &str)> {
    // Normalising CRLF here would mean copying the whole buffer; editors on
    // this platform write LF, and a stray CR is handled per line instead.
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            return Ok((&text[..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(MailError::Draft(
        "the message has no blank line after its headers — a mail is a header \
         block, an empty line, and then the text"
            .into(),
    ))
}

/// Split the body from the quoted region at [`QUOTE_MARKER`].
fn split_quote(text: &str) -> (&str, Option<&str>) {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == QUOTE_MARKER {
            let after = offset + line.len();
            return (&text[..offset], Some(&text[after..]));
        }
        offset += line.len();
    }
    (text, None)
}

fn parse_headers(head: &str) -> MailResult<Headers> {
    let mut headers = Headers::default();
    let mut seen: Vec<String> = Vec::new();
    // A folded value (a continuation line starting with whitespace) is legal
    // in a mail header and cheap to accept here; some editors soft-wrap long
    // recipient lists into exactly that shape.
    let mut unfolded: Vec<String> = Vec::new();
    for raw in head.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            match unfolded.last_mut() {
                Some(last) => {
                    last.push(' ');
                    last.push_str(line.trim());
                    continue;
                }
                None => {
                    return Err(MailError::Draft(
                        "the message starts with an indented line — the first line must be a header".into(),
                    ));
                }
            }
        }
        unfolded.push(line.to_string());
    }
    for line in unfolded {
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(MailError::Draft(format!(
                "`{line}` is not a header — every line above the blank one reads `Name: value`"
            )));
        };
        let name = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if seen.contains(&name) {
            return Err(MailError::Draft(format!("`{key}` is given twice")));
        }
        seen.push(name.clone());
        match name.as_str() {
            "from" => headers.from = value.to_string(),
            "to" => headers.to = split_addresses(value),
            "cc" => headers.cc = split_addresses(value),
            "bcc" => headers.bcc = split_addresses(value),
            "subject" => headers.subject = value.to_string(),
            other => {
                return Err(MailError::Draft(format!(
                    "`{other}` is not a header this buffer understands — \
                     From, To, Cc, Bcc and Subject are"
                )));
            }
        }
    }
    Ok(headers)
}

/// Split a recipient list on commas — except the ones inside a quoted display
/// name or an address literal.
///
/// `"Beispiel, Anna" <anna@example.invalid>` is one recipient, and splitting
/// it into two is how a mail goes to a stranger called `Anna`.
pub(crate) fn split_addresses(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut in_angle = false;
    for c in value.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            '<' if !in_quotes => {
                in_angle = true;
                current.push(c);
            }
            '>' if !in_quotes => {
                in_angle = false;
                current.push(c);
            }
            ',' if !in_quotes && !in_angle => {
                push_address(&mut out, &current);
                current.clear();
            }
            _ => current.push(c),
        }
    }
    push_address(&mut out, &current);
    out
}

fn push_address(out: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> Headers {
        Headers {
            from: "Me <me@example.invalid>".into(),
            to: vec!["Anna <anna@example.invalid>".into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Re: Angebot".into(),
        }
    }

    /// The round trip every other test stands on: what is rendered parses
    /// back into what it was built from.
    #[test]
    fn a_rendered_buffer_parses_back_to_its_own_values() {
        let text = render(&headers(), "Hallo,\n\ndanke!", Some("> alt\n"));
        let draft = parse(&text, &text).expect("parses");
        assert_eq!(draft.headers, headers());
        assert_eq!(draft.body, "Hallo,\n\ndanke!");
        assert_eq!(draft.quote, QuoteState::Intact);
    }

    /// The whole point of the marker: an untouched quote is quoted from the
    /// original, an edited one is refused, and a deleted one is a decision.
    #[test]
    fn the_quote_region_is_judged_against_the_template() {
        let template = render(&headers(), "", Some("> alt\n> mehr\n"));

        let untouched = render(&headers(), "Hallo", Some("> alt\n> mehr\n"));
        assert_eq!(
            parse(&untouched, &template).expect("parses").quote,
            QuoteState::Intact
        );

        let edited = render(&headers(), "Hallo", Some("> alt\n> ANTWORT DAZWISCHEN\n"));
        assert_eq!(
            parse(&edited, &template).expect("parses").quote,
            QuoteState::Modified
        );

        let dropped = render(&headers(), "Hallo", None);
        assert_eq!(
            parse(&dropped, &template).expect("parses").quote,
            QuoteState::Dropped
        );
    }

    /// An editor that adds or eats a trailing newline must not read as an
    /// edit — that is the difference between "saved" and "changed".
    #[test]
    fn trailing_whitespace_is_not_an_edit() {
        let template = render(&headers(), "", Some("> alt\n"));
        let saved = format!("{}\n\n", template.trim_end());
        assert_eq!(
            parse(&saved, &template).expect("parses").quote,
            QuoteState::Intact
        );
    }

    /// A marker the user typed into a message that never had a quote is
    /// text, not a promise the adapter made.
    #[test]
    fn a_marker_without_a_template_quote_is_not_a_modified_quote() {
        let template = render(&headers(), "", None);
        let typed = render(&headers(), "siehe unten", Some("> eigenes Zitat\n"));
        assert_eq!(
            parse(&typed, &template).expect("parses").quote,
            QuoteState::Dropped
        );
    }

    /// A resumed draft is its own template, so the guard cannot compare the
    /// two: judged against the quote rendered fresh from the original, an
    /// edit stays an edit however often the draft is re-opened.
    #[test]
    fn a_resumed_draft_is_judged_against_the_original_quote() {
        let quoted = "> alt\n> mehr\n";
        let edited = render(&headers(), "Hallo", Some("> alt\n> ANTWORT\n"));
        assert_eq!(
            parse(&edited, &edited).expect("parses").quote,
            QuoteState::Intact,
            "against itself a draft always looks untouched"
        );
        assert_eq!(
            parse_with_quote(&edited, Some(quoted))
                .expect("parses")
                .quote,
            QuoteState::Modified
        );
        let kept = render(&headers(), "Hallo", Some(quoted));
        assert_eq!(
            parse_with_quote(&kept, Some(quoted)).expect("parses").quote,
            QuoteState::Intact
        );
    }

    #[test]
    fn a_display_name_may_contain_a_comma() {
        let got = split_addresses("\"Beispiel, Anna\" <anna@example.invalid>, b@example.invalid");
        assert_eq!(
            got,
            vec![
                "\"Beispiel, Anna\" <anna@example.invalid>".to_string(),
                "b@example.invalid".to_string()
            ]
        );
    }

    #[test]
    fn an_unknown_header_is_refused_by_name() {
        let text = "From: a@b.invalid\nX-Spam: yes\n\ntext\n";
        let err = parse(text, text).expect_err("unknown header");
        assert!(err.to_string().contains("x-spam"), "names it: {err}");
    }

    #[test]
    fn a_buffer_without_a_blank_line_says_what_is_missing() {
        let text = "From: a@b.invalid\nTo: c@d.invalid\nSubject: hi\n";
        let err = parse(text, text).expect_err("no body");
        assert!(err.to_string().contains("blank line"), "explains it: {err}");
    }

    #[test]
    fn a_folded_recipient_list_stays_one_header() {
        let text = "From: a@b.invalid\nTo: one@x.invalid,\n  two@x.invalid\nSubject: hi\n\nhallo\n";
        let draft = parse(text, text).expect("parses");
        assert_eq!(draft.headers.to.len(), 2, "both recipients survive folding");
    }

    #[test]
    fn the_same_header_twice_is_refused() {
        let text = "From: a@b.invalid\nTo: c@d.invalid\nTo: e@f.invalid\n\nhi\n";
        let err = parse(text, text).expect_err("duplicate header");
        assert!(err.to_string().contains("twice"), "explains it: {err}");
    }
}
