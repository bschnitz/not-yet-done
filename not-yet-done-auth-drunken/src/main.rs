//! `nyd-auth-drunken` — log in with a browser, for adapters that cannot.
//!
//! The first plugin of ADR 0010, and the reason the ADR exists: an SSO
//! login that only a real browser can perform, driven by a
//! [drunken-browser](https://github.com/bschnitz/drunken-browser) flow and
//! reported to nyd step by step while it happens.
//!
//! It is a translator between two protocols and holds no policy of its own.
//! What to click, where to sign in, what to hand back is the flow's; when
//! to log in again and how long a session lasts is nyd's.
//!
//! # Why the browser is never linked
//!
//! drunken-browser is spoken to over its control socket, in JSON this crate
//! writes by hand, and none of its crates is a dependency here. Two
//! programs that share a socket can be built, released and broken
//! separately; two that share a crate cannot. It keeps nyd's vocabulary out
//! of a browser and a browser's out of nyd — and it is what lets a user
//! swap this plugin for one driving something else entirely without nyd
//! noticing.
//!
//! The nyd half *is* a dependency ([`not_yet_done_content`]), because
//! writing the line protocol down twice in one workspace is how the two
//! spellings start to differ.
//!
//! # What it does not use
//!
//! nyd offers a plugin the session that just expired, so it can be
//! refreshed rather than logged in again. This one ignores it: a browser's
//! session is its profile, which is on disk and outlives every one of these
//! processes. Handing the blob back would mean putting a live session
//! cookie through a run's facts, which are shown as the run goes.
//!
//! # Example
//!
//! ```yaml
//! auth:
//!   mechanism: cookie
//!   plugins:
//!     - name: browser
//!       command: nyd-auth-drunken --flow ~/flows/jira-sso --field cookie=session
//!       attention_timeout_secs: 600
//!   bindings:
//!     - field: cookie
//!       provider: { type: plugin, use: browser }
//! ```

mod bridge;
mod browser;
mod nyd;
mod options;

use not_yet_done_content::auth::{PLUGIN_PROTOCOL, PluginLine, ToPlugin};

use crate::bridge::Outcome;
use crate::browser::Browser;
use crate::nyd::{Nyd, say};
use crate::options::{Parsed, USAGE};

/// Every path out of this program ends here, and none of them by returning.
///
/// Returning would drop the tokio runtime, which waits for the blocking
/// read this plugin has outstanding on its stdin — and nyd holds that pipe
/// open until it has the answer, which it is waiting for us to print. The
/// plugin would say `result` and then hang until nyd killed it. Every line
/// is written and flushed as it is said, so there is nothing here for a
/// destructor to finish.
fn done(code: i32) -> ! {
    std::process::exit(code)
}

#[tokio::main]
async fn main() -> ! {
    let options = match options::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Usage) => {
            println!("{USAGE}");
            done(0);
        }
        // Before `start`, so nyd is not listening yet and there is nobody
        // to say `error` to: the usage goes where a person will find it.
        Err(why) => {
            eprintln!("nyd-auth-drunken: {why}\n\n{USAGE}");
            done(2);
        }
    };

    match login(&options).await {
        Ok(()) => done(0),
        // Every failure from here on is nyd's to show: `error` is the one
        // word that reaches the user, and exiting quietly would leave them
        // with "the plugin stopped talking" instead of a reason.
        Err(why) => {
            say(PluginLine::error(why));
            done(1);
        }
    }
}

async fn login(options: &options::Options) -> Result<(), String> {
    let mut nyd = Nyd::on_stdin();
    let request = opening(&mut nyd).await?;

    let mut browser = Browser::open(options).await?;
    let outcome = bridge::run(options, &mut nyd, &mut browser, &request).await;
    // Before the answer and before the error, either way: a browser this
    // process started is this process's to close, and the login is over
    // whichever of the two it is.
    browser.close().await;

    match outcome? {
        Outcome::Values {
            values,
            expires_at_unix_ms,
        } => {
            say(PluginLine::result(values, expires_at_unix_ms));
            Ok(())
        }
        // nyd asked us to stop, so it is not waiting for a word — and an
        // `error` would be a failure reported for something it did itself.
        Outcome::Cancelled => Ok(()),
    }
}

/// The `start` line, which every conversation begins with.
async fn opening(nyd: &mut Nyd) -> Result<Vec<String>, String> {
    match nyd.next().await {
        Some(Ok(ToPlugin::Start {
            protocol, request, ..
        })) => {
            if protocol != PLUGIN_PROTOCOL {
                return Err(format!(
                    "this plugin speaks auth protocol {PLUGIN_PROTOCOL}, nyd speaks {protocol}"
                ));
            }
            if request.is_empty() {
                return Err("nyd asked this plugin for no fields at all".into());
            }
            Ok(request)
        }
        Some(Ok(_)) => Err("nyd said something before `start`".into()),
        Some(Err(why)) => Err(why),
        None => Err("nyd closed this plugin's input before saying `start`".into()),
    }
}
