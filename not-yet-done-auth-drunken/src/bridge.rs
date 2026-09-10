//! The translation itself: a run in a browser, said in nyd's words.
//!
//! The two vocabularies line up almost word for word, which is why this is
//! a match and not a state machine:
//!
//! - a step the run *began* → `step`, so nyd's connect line names the step
//!   of the flow that is stuck rather than "running the cookie script";
//! - the run *telling* somebody something → `attention`, because a `tell:`
//!   in a login flow is exactly the case nyd added the flag for: nothing to
//!   submit, a person acting somewhere this program cannot reach;
//! - the run *asking* for a value → `form`, answered through whichever
//!   frontend the user is in front of and handed back with `flow_answer`;
//! - the run being *over* → `result` or `error`.
//!
//! The one asymmetry is that a `tell:` has no end of its own. A run that
//! has said something to a person goes on being about that person until it
//! does something else, so the next step to begin is what takes the
//! attention back. The next *line* is not: a step that waits for somebody
//! keeps checking while it waits, and every one of those checks is a line
//! the run finished without anything having happened.

use std::collections::BTreeMap;

use not_yet_done_content::auth::{PluginLine, ScriptForm, ScriptFormField, ToPlugin};
use serde_json::{Value, json};

use crate::browser::Browser;
use crate::nyd::{Nyd, say};
use crate::options::{Options, Show};

/// The yield that is a deadline rather than a value.
///
/// A convention and not an option, because it is the same sentence in every
/// flow: a login that knows when its session dies says so under this name,
/// and one that does not, does not.
pub const EXPIRES_AT: &str = "expires_at_unix_ms";

pub enum Outcome {
    Values {
        values: BTreeMap<String, String>,
        expires_at_unix_ms: Option<u64>,
    },
    /// nyd gave up on the login, and said so.
    Cancelled,
}

/// Run the flow and translate it until it ends, one way or the other.
pub async fn run(
    options: &Options,
    nyd: &mut Nyd,
    browser: &mut Browser,
    request: &[String],
) -> Result<Outcome, String> {
    if options.show == Show::Always {
        show(browser, true).await;
    }

    // The last part of the path and not the whole of it: a status line is
    // read at a glance, and the flow names itself properly a moment later,
    // in `started`, with the name its author gave it.
    let called = options
        .flow
        .file_name()
        .unwrap_or(options.flow.as_os_str())
        .to_string_lossy()
        .into_owned();
    say(PluginLine::step(format!("opening `{called}`")));
    browser
        .line(format!("test-open {}", options.flow.display()))
        .await?;
    browser
        .ask(json!({ "ask": "flow_run", "what": "whole", "data": options.data }))
        .await?;

    let mut attending = false;
    let mut asking: Option<String> = None;
    loop {
        tokio::select! {
            // Biased, so that a `cancel` already in the pipe is acted on
            // before another step is announced for a login nobody wants.
            biased;
            from_nyd = nyd.next() => match from_nyd {
                // Stdin closed is "give up" without the word, and is what a
                // plugin whose runtime died sees.
                None | Some(Ok(ToPlugin::Cancel {})) => {
                    let _ = browser.line("test-stop").await;
                    return Ok(Outcome::Cancelled);
                }
                Some(Ok(ToPlugin::Input(answers))) => {
                    let Some(field) = asking.take() else {
                        return Err("nyd answered a form this plugin never asked".into());
                    };
                    // Every answer so far arrives each time, so the one
                    // this form asked for is picked out by name. Absent
                    // means the user left an optional field empty, which
                    // the run hears as a refusal.
                    let value = answers.get(&field).cloned();
                    browser
                        .ask(json!({ "ask": "flow_answer", "value": value }))
                        .await?;
                    took_the_person_back(options, browser, &mut attending).await;
                }
                Some(Ok(ToPlugin::Start { .. })) => {
                    return Err("nyd said `start` twice".into());
                }
                Some(Err(why)) => return Err(why),
            },
            news = browser.next_news() => {
                let news = news?;
                match news.get("news").and_then(Value::as_str) {
                    Some("started") => {
                        if let Some(name) = text(&news, "flow") {
                            say(PluginLine::step(format!("running `{name}`")));
                        }
                    }
                    Some("run") => {
                        // Only the beginning of a step is worth a line of
                        // nyd's: every action inside one would be a status
                        // that changes faster than anybody can read it, and
                        // the step is what the flow's author named. It is
                        // also the only place the attention can end, for the
                        // same reason read the other way round.
                        let happened = news.get("happened");
                        if happened.and_then(|h| h.get("happened")).and_then(Value::as_str)
                            == Some("began")
                        {
                            took_the_person_back(options, browser, &mut attending).await;
                            if let Some(name) = happened.and_then(|h| text(h, "name")) {
                                say(PluginLine::step(name));
                            }
                        }
                    }
                    Some("told") => {
                        let Some(said) = text(&news, "said") else { continue };
                        say(PluginLine::attention(said));
                        attending = true;
                        if options.show == Show::Needed {
                            show(browser, true).await;
                        }
                    }
                    Some("asking") => {
                        let Some(hole) = text(&news, "hole") else { continue };
                        let field = asked_for(&hole);
                        asking = Some(field.name.clone());
                        say(PluginLine::form(ScriptForm {
                            header: Some("The login in the browser is asking".to_string()),
                            error: None,
                            fields: vec![field],
                        }));
                        if options.show == Show::Needed {
                            show(browser, true).await;
                        }
                    }
                    Some("over") => {
                        return over(&news, options, request);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The run has moved on, so whatever it was waiting on a person for is done.
async fn took_the_person_back(options: &Options, browser: &mut Browser, attending: &mut bool) {
    if !*attending {
        return;
    }
    *attending = false;
    say(PluginLine::attention_over());
    if options.show == Show::Needed {
        show(browser, false).await;
    }
}

/// What the run yielded, as the values nyd asked for.
fn over(news: &Value, options: &Options, request: &[String]) -> Result<Outcome, String> {
    if news.get("stopped").and_then(Value::as_bool) == Some(true) {
        return Err("the login was stopped in the browser".into());
    }
    let tally = news.get("tally");
    let count = |what: &str| tally.and_then(|t| t.get(what)).and_then(Value::as_u64);
    let (failed, broke) = (count("failed").unwrap_or(0), count("broke").unwrap_or(0));
    if failed > 0 || broke > 0 {
        let ran = count("ran").unwrap_or(0);
        return Err(format!(
            "the login flow did not pass: {ran} steps, {failed} failed, {broke} broke"
        ));
    }

    let empty = serde_json::Map::new();
    let yielded = news
        .get("yielded")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let expires_at_unix_ms = yielded.get(EXPIRES_AT).and_then(|when| match when {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    });

    let mut values = BTreeMap::new();
    let mut missing = Vec::new();
    for field in request {
        let from = options.yield_for(field);
        match yielded.get(from) {
            Some(value) => {
                values.insert(field.clone(), as_text(value));
            }
            None => missing.push(format!("`{field}` (from `{from}`)")),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "the login flow passed but yielded nothing for {}; it yields {}",
            missing.join(", "),
            names(yielded)
        ));
    }

    Ok(Outcome::Values {
        values,
        expires_at_unix_ms,
    })
}

/// The field a hole asks for.
///
/// A hole is written `{{secret.otp}}`, and its first part is what makes the
/// difference here: `secret` is why the frontend masks the input, and every
/// other kind is a fact somebody may want to read back as they type it.
fn asked_for(hole: &str) -> ScriptFormField {
    let inside = hole
        .trim()
        .trim_start_matches("{{")
        .trim_end_matches("}}")
        .trim();
    let (kind, name) = match inside.split_once('.') {
        Some((kind, name)) => (kind, name),
        None => ("", inside),
    };
    ScriptFormField {
        name: name.to_string(),
        label: None,
        masked: kind == "secret",
        optional: false,
        prefill: None,
    }
}

/// A yielded value as the string a credential is.
///
/// A flow may yield anything JSON can hold, and a credential is text: a
/// string is itself, and everything else is the JSON it was written as,
/// which is what an adapter expecting a blob wants back.
fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn names(yielded: &serde_json::Map<String, Value>) -> String {
    if yielded.is_empty() {
        return "nothing".to_string();
    }
    yielded
        .keys()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Put a window around the browser, or take it away.
///
/// A refusal is logged and no more: a browser with no display to draw on is
/// a reason to go on headless, not a reason to fail a login that may well
/// need nobody.
async fn show(browser: &mut Browser, on: bool) {
    if let Err(why) = browser.ask(json!({ "ask": "show", "on": on })).await {
        eprintln!("nyd-auth-drunken: no window ({why})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_hole_is_asked_for_masked_and_any_other_is_not() {
        let secret = asked_for("{{secret.otp}}");
        assert_eq!(secret.name, "otp");
        assert!(secret.masked);

        let fact = asked_for("{{data.tenant}}");
        assert_eq!(fact.name, "tenant");
        assert!(!fact.masked);
    }

    /// A hole with no kind in front of it is still a name to ask under.
    #[test]
    fn a_bare_hole_is_its_own_name() {
        assert_eq!(asked_for("{{otp}}").name, "otp");
    }

    #[test]
    fn a_yielded_string_is_the_credential_and_anything_else_is_its_json() {
        assert_eq!(as_text(&json!("JSESSIONID=x")), "JSESSIONID=x");
        assert_eq!(as_text(&json!({ "cookie": "x" })), r#"{"cookie":"x"}"#);
    }
}
