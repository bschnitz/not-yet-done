//! The command line, which is the whole of this plugin's configuration.
//!
//! A plugin is configured by the `command:` string of its `plugins:` entry
//! and by nothing else — nyd has no place to put a second config file for
//! one, and inventing one would mean a user editing two files to change
//! which flow logs them in.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub const USAGE: &str = "\
nyd-auth-drunken — log in with a drunken-browser flow, as an nyd auth plugin

Usage: nyd-auth-drunken --flow <path> [options]

  --flow <path>           the flow to run. Required.
  --field <name>[=<yield>] which of the flow's yields answers which of the
                          fields nyd asked for. Named alone, the yield has
                          the field's own name — which is also what every
                          field nobody named falls back to.
  --data <name>=<value>   a fact for the run, read as {{data.<name>}}, the
                          way --data is on drunken-test's line. Never a
                          secret: a run's facts are shown as it goes.
  --show <when>           when to put a window around the browser:
                          needed (default), always, never.
  --socket <path>         drive a browser already running on this socket
                          instead of starting one. For working on a flow:
                          a login run unattended should own its browser.
  --browser <command>     how to start one. Default: drunken-browser
  --start-timeout <secs>  how long to wait for that browser. Default: 60
  -h, --help              this
";

/// When a window goes up around the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// Never. The login must go through without a person seeing it.
    Never,
    /// While the run needs a person — it is asking for a value, or telling
    /// them to do something. Down again when the run moves on.
    ///
    /// The default because it is the honest reading of both halves: a login
    /// nobody has to touch should not steal focus, and one that stops for a
    /// person is unanswerable if they cannot see the page it stopped on.
    Needed,
    /// From the first moment, so a person can watch the whole login.
    Always,
}

#[derive(Debug)]
pub struct Options {
    pub flow: PathBuf,
    /// nyd's field name → the yield that answers it. Only the ones named on
    /// the line; anything else is answered by the yield of its own name.
    pub fields: BTreeMap<String, String>,
    pub data: BTreeMap<String, String>,
    pub show: Show,
    pub socket: Option<PathBuf>,
    pub browser: String,
    pub start_timeout_secs: u64,
}

/// What `main` does with a line it could not read.
pub enum Parsed {
    Run(Box<Options>),
    /// `--help`, which is not a failure and must not be reported as one.
    Usage,
}

pub fn parse(args: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut flow = None;
    let mut fields = BTreeMap::new();
    let mut data = BTreeMap::new();
    let mut show = Show::Needed;
    let mut socket = None;
    let mut browser = "drunken-browser".to_string();
    let mut start_timeout_secs = 60;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        // Both spellings of every option, because a user who writes
        // `--flow=x` in one place and `--flow x` in another is not making a
        // mistake either time.
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = || match inline.clone() {
            Some(value) => Ok(value),
            None => args.next().ok_or_else(|| format!("{name} wants a value")),
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(Parsed::Usage),
            "--flow" => flow = Some(PathBuf::from(value()?)),
            "--field" => {
                let spec = value()?;
                let (field, from) = match spec.split_once('=') {
                    Some((field, from)) => (field.to_string(), from.to_string()),
                    None => (spec.clone(), spec),
                };
                fields.insert(field, from);
            }
            "--data" => {
                let spec = value()?;
                let (key, val) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--data wants <name>=<value>, got `{spec}`"))?;
                data.insert(key.to_string(), val.to_string());
            }
            "--show" => {
                show = match value()?.as_str() {
                    "never" => Show::Never,
                    "needed" => Show::Needed,
                    "always" => Show::Always,
                    other => {
                        return Err(format!("--show is never, needed or always, not `{other}`"));
                    }
                }
            }
            "--socket" => socket = Some(PathBuf::from(value()?)),
            "--browser" => browser = value()?,
            "--start-timeout" => {
                let secs = value()?;
                start_timeout_secs = secs
                    .parse()
                    .map_err(|_| format!("--start-timeout wants whole seconds, got `{secs}`"))?;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }

    let flow = flow.ok_or("no --flow: there is nothing to run")?;
    Ok(Parsed::Run(Box::new(Options {
        flow,
        fields,
        data,
        show,
        socket,
        browser,
        start_timeout_secs,
    })))
}

impl Options {
    /// The yield that answers `field`.
    pub fn yield_for<'a>(&'a self, field: &'a str) -> &'a str {
        self.fields.get(field).map_or(field, String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Options {
        match parse(line.split_whitespace().map(str::to_string)) {
            Ok(Parsed::Run(options)) => *options,
            Ok(Parsed::Usage) => panic!("asked for usage"),
            Err(why) => panic!("{why}"),
        }
    }

    fn refused(line: &str) -> String {
        match parse(line.split_whitespace().map(str::to_string)) {
            Err(why) => why,
            _ => panic!("`{line}` should not have been accepted"),
        }
    }

    #[test]
    fn a_flow_is_the_one_thing_that_must_be_said() {
        assert_eq!(
            parsed("--flow /tmp/login.yaml").flow,
            PathBuf::from("/tmp/login.yaml")
        );
        assert!(refused("--show always").contains("nothing to run"));
    }

    #[test]
    fn an_option_takes_its_value_either_way_round() {
        assert_eq!(
            parsed("--flow=/tmp/a.yaml").flow,
            PathBuf::from("/tmp/a.yaml")
        );
        assert_eq!(
            parsed("--flow /tmp/a.yaml --show=always").show,
            Show::Always
        );
    }

    /// The mapping is only for the names that differ, which is why a field
    /// nobody mentioned still has an answer.
    #[test]
    fn a_field_is_answered_by_the_yield_of_its_name_unless_told_otherwise() {
        let options = parsed("--flow /tmp/a.yaml --field cookie=session");
        assert_eq!(options.yield_for("cookie"), "session");
        assert_eq!(options.yield_for("token"), "token");
    }

    #[test]
    fn a_data_fact_is_a_pair() {
        let options = parsed("--flow /tmp/a.yaml --data tenant=acme");
        assert_eq!(options.data.get("tenant").map(String::as_str), Some("acme"));
        assert!(refused("--flow /tmp/a.yaml --data tenant").contains("<name>=<value>"));
    }
}
