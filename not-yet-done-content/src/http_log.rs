//! HTTP error formatting + configurable request/error logging shared across
//! content adapters.
//!
//! Adapters wrap their `reqwest` calls with [`check_status`] and
//! [`network_error`] instead of the manual `if !resp.status().is_success()`
//! pattern, so error messages always include the URL.
//!
//! ## Logging
//!
//! The log has two levels: *errors* (everything the user also sees via
//! `notify_error`, plus failed responses and network failures) and *verbose*
//! (every outbound request and response status). Errors are written whenever
//! logging is enabled; verbose lines only when verbose mode is on.
//!
//! Enable logging by calling [`configure`] once at startup (the TUI does this
//! from its config file) or via environment variables, which always take
//! precedence:
//!
//! - `NYD_DEBUG=1` — force logging on *and* verbose (mirror every request).
//! - `NYD_LOG_DIR=/path` — directory for the rotating daily log files.
//! - `NYD_LOG_RETENTION_DAYS=N` — how many days of log files to keep.
//! - `NYD_DEBUG_LOG=/path` — legacy escape hatch: log to this exact file with
//!   no rotation.
//!
//! When enabled with a directory, one file per day is written as
//! `<dir>/nyd-YYYY-MM-DD.log`; files older than the retention window are pruned
//! on each day roll-over. When nothing is configured and no env var is set,
//! every function below is a no-op except the synchronous string formatting
//! that `check_status` / `network_error` need anyway.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Local};
use reqwest::Response;

const BODY_TRUNCATE_LEN: usize = 2000;

/// Path fragments that mark a redirect target as a login / SSO flow rather
/// than an ordinary relocation. Matched case-insensitively against the
/// `Location` header (see [`redirect_reason`]).
const LOGIN_MARKERS: &[&str] = &[
    "login",
    "saml",
    "sso",
    "oauth",
    "openid",
    "adfs",
    "signin",
    "auth/realms",
];

/// Opening of the message [`redirect_reason`] builds for a login bounce.
/// Shared with [`is_auth_rejection`] so the wording and its detection cannot
/// drift apart.
const SESSION_EXPIRED_MARKER: &str = "session expired — the server answered with the login flow";

/// Strip the query string from a URL so a redirect target can be named in an
/// error message without dragging along a multi-kilobyte `SAMLRequest`.
fn url_without_query(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Whether a redirect target is a login / SSO endpoint rather than an
/// ordinary relocation.
fn is_login_target(url_without_query: &str) -> bool {
    let lower = url_without_query.to_ascii_lowercase();
    LOGIN_MARKERS.iter().any(|m| lower.contains(m))
}

/// Redirect policy for JSON-API clients behind an SSO reverse proxy.
///
/// Follows ordinary redirects (trailing slashes, attachment CDNs, …) but
/// **stops** at a login flow and hands the 3xx back to the caller, where
/// [`check_status`] turns it into an actionable "session expired" error.
///
/// Following that bounce is what the default policy does, and it fails twice
/// over: the API call comes back as a 200 HTML login page, which surfaces
/// far downstream as an unexplained JSON parse error, and the client's
/// `Cookie` / `Authorization` default headers get replayed to the identity
/// provider on the way.
pub fn api_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let target = format!(
            "{}{}",
            attempt.url().host_str().unwrap_or(""),
            attempt.url().path()
        );
        if is_login_target(&target) {
            attempt.stop()
        } else if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// Explain a 3xx response. A redirect whose target looks like a login flow
/// means the session was silently rejected: SSO-fronted Jira/Confluence
/// deployments answer an API call with the login bounce instead of a 401, so
/// the caller has no other way to tell an expired session from a real answer.
fn redirect_reason(location: Option<&str>) -> String {
    let Some(location) = location else {
        return "unexpected redirect (no Location header)".to_string();
    };
    let target = url_without_query(location);
    if is_login_target(target) {
        format!(
            "{SESSION_EXPIRED_MARKER} at {target} instead of data. \
             Reconnect this instance to log in again."
        )
    } else {
        format!("unexpected redirect to {target}")
    }
}

/// Logging configuration handed in by the host at startup. Environment
/// variables override individual fields (see the module docs).
#[derive(Debug, Clone)]
pub struct LogSettings {
    /// Whether to write a log at all (errors always, verbose only if `verbose`).
    pub enabled: bool,
    /// Directory holding the rotating daily log files.
    pub directory: PathBuf,
    /// Number of days of log files to keep (including today). Older files are
    /// pruned on each day roll-over.
    pub retention_days: i64,
    /// Also mirror every request/response line, not just errors.
    pub verbose: bool,
}

/// Where the log lines go: a fixed file (legacy `NYD_DEBUG_LOG`) or a directory
/// of per-day files pruned to a retention window.
enum Target {
    Fixed(PathBuf),
    Rotating { dir: PathBuf, retention_days: i64 },
}

struct Logger {
    verbose: bool,
    target: Target,
    /// `(bucket_key, open append handle)`. The key is the date (`YYYY-MM-DD`)
    /// for a rotating target — a mismatch triggers a reopen + prune — and the
    /// empty string for a fixed file (opened once).
    current: Mutex<Option<(String, File)>>,
}

impl Logger {
    fn new(target: Target, verbose: bool) -> Self {
        Self {
            verbose,
            target,
            current: Mutex::new(None),
        }
    }

    /// Ensure `current` holds the correct open file for `now`, reopening (and
    /// pruning old files) when the day rolls over on a rotating target.
    fn ensure_file(
        &self,
        current: &mut Option<(String, File)>,
        now: DateTime<Local>,
    ) -> std::io::Result<()> {
        match &self.target {
            Target::Fixed(path) => {
                if current.is_none() {
                    let f = OpenOptions::new().create(true).append(true).open(path)?;
                    *current = Some((String::new(), f));
                }
            }
            Target::Rotating {
                dir,
                retention_days,
            } => {
                let date = now.format("%Y-%m-%d").to_string();
                let stale = current.as_ref().map(|(d, _)| d != &date).unwrap_or(true);
                if stale {
                    fs::create_dir_all(dir)?;
                    let path = dir.join(format!("nyd-{date}.log"));
                    let f = OpenOptions::new().create(true).append(true).open(&path)?;
                    *current = Some((date, f));
                    prune_old(dir, *retention_days, now);
                }
            }
        }
        Ok(())
    }
}

/// Host-supplied settings, consulted (once) when the logger is first resolved.
static CONFIGURED: OnceLock<LogSettings> = OnceLock::new();
/// The resolved logger, built lazily on first use from [`CONFIGURED`] + env.
static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();

/// Install the host's logging settings. Call once at startup, before any
/// logging happens; later calls are ignored (the first wins). Environment
/// variables still override individual fields when the logger is resolved.
pub fn configure(settings: LogSettings) {
    let _ = CONFIGURED.set(settings);
}

fn logger() -> Option<&'static Logger> {
    LOGGER.get_or_init(resolve_logger).as_ref()
}

/// Build the logger from the configured settings and environment overrides.
/// Precedence per field: env var > configured value > built-in default.
fn resolve_logger() -> Option<Logger> {
    let env_debug = std::env::var_os("NYD_DEBUG").is_some();
    let cfg = CONFIGURED.get();

    let enabled = env_debug || cfg.map(|c| c.enabled).unwrap_or(false);
    if !enabled {
        return None;
    }
    let verbose = env_debug || cfg.map(|c| c.verbose).unwrap_or(false);

    // Legacy escape hatch: an explicit file path disables rotation.
    if let Some(path) = std::env::var_os("NYD_DEBUG_LOG") {
        return Some(Logger::new(Target::Fixed(PathBuf::from(path)), verbose));
    }

    let dir = std::env::var_os("NYD_LOG_DIR")
        .map(PathBuf::from)
        .or_else(|| cfg.map(|c| c.directory.clone()))
        .unwrap_or_else(std::env::temp_dir);
    let retention_days = std::env::var("NYD_LOG_RETENTION_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| cfg.map(|c| c.retention_days))
        .unwrap_or(3);
    Some(Logger::new(
        Target::Rotating {
            dir,
            retention_days,
        },
        verbose,
    ))
}

/// Delete rotating log files older than the retention window (`retention_days`
/// counting back from `now`, inclusive of today). Best-effort: any unparsable
/// name or IO error is skipped.
fn prune_old(dir: &Path, retention_days: i64, now: DateTime<Local>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = now.date_naive() - chrono::Duration::days((retention_days - 1).max(0));
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(date_str) = name
            .strip_prefix("nyd-")
            .and_then(|s| s.strip_suffix(".log"))
        else {
            continue;
        };
        let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };
        if date < cutoff {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Write an error line (always, whenever logging is enabled).
fn write_line(line: &str) {
    emit(line, false);
}

/// Write a verbose line (only when verbose mode is on).
fn write_verbose(line: &str) {
    emit(line, true);
}

fn emit(line: &str, verbose_only: bool) {
    let Some(lg) = logger() else { return };
    if verbose_only && !lg.verbose {
        return;
    }
    let now = chrono::Local::now();
    let Ok(mut current) = lg.current.lock() else {
        return;
    };
    if lg.ensure_file(&mut current, now).is_err() {
        return;
    }
    if let Some((_, f)) = current.as_mut() {
        let ts = now.format("%Y-%m-%dT%H:%M:%S%.3f");
        let _ = writeln!(f, "{ts} {line}");
    }
}

/// Log an outbound request line (verbose).
pub fn log_request(method: &str, url: &str) {
    write_verbose(&format!("-> {method} {url}"));
}

/// Log a response status line (verbose).
pub fn log_response(method: &str, url: &str, status: u16) {
    write_verbose(&format!("<- {method} {url} [{status}]"));
}

/// Log a free-form error line (e.g. from the TUI's `notify_error` /
/// `set_query_error` paths) so the log mirrors the same error stream the user
/// sees in the UI. Always written when logging is enabled.
pub fn log_error(context: &str, message: &str) {
    write_line(&format!("ERROR {context}: {message}"));
}

/// Log a free-form debug line (verbose).
pub fn log_debug(context: &str, message: &str) {
    write_verbose(&format!("DEBUG {context}: {message}"));
}

fn truncate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "<empty body>".to_string()
    } else {
        // Character-based: an error page in a language with umlauts would make
        // a byte slice at a fixed index panic *while reporting an error*.
        crate::text::truncate_with_ellipsis(trimmed, BODY_TRUNCATE_LEN, "…(truncated)")
    }
}

/// Wrap a `reqwest::Response`: returns it unchanged on 2xx, otherwise
/// consumes the body and returns a uniform error string that always
/// includes method, URL, status, and a truncated response body. Also
/// logs the response status (verbose) and, on error, the formatted message.
///
/// A 3xx is reported by naming the `Location` target instead of the body —
/// the body of a redirect is boilerplate, the target is the information.
/// Adapters that talk to an SSO-fronted server must build their client with
/// [`api_redirect_policy`] for this to fire; under `reqwest`'s default policy
/// the bounce is followed and the login page comes back with a 200, which then
/// fails somewhere far downstream as an unexplained JSON parse error.
pub async fn check_status(method: &str, url: &str, resp: Response) -> Result<Response, String> {
    let status = resp.status();
    log_response(method, url, status.as_u16());
    if status.is_success() {
        return Ok(resp);
    }
    let snippet = if status.is_redirection() {
        redirect_reason(
            resp.headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok()),
        )
    } else {
        truncate_body(&resp.text().await.unwrap_or_default())
    };
    let msg = format!(
        "{method} {url} -> {} {}: {snippet}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    write_line(&format!("ERROR {msg}"));
    Err(msg)
}

/// The status code out of a [`check_status`] error string, i.e. the number
/// right behind the `" -> "` separator. `None` for anything not shaped like
/// one (a network error, a parse failure, a message from somewhere else).
fn error_status(err: &str) -> Option<u16> {
    // URLs never contain a space, so the first separator is the real one.
    let (_, rest) = err.split_once(" -> ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether an error string produced by [`check_status`] means *the server
/// rejected this session* — as opposed to any other failure.
///
/// Two shapes count: a plain `401`, and the SSO login bounce that
/// [`redirect_reason`] explains (a 3xx into an identity provider, which is
/// how an SSO-fronted Jira / Confluence says "expired" without a 401).
///
/// A `403` deliberately does **not** count. Jira and Confluence answer with
/// it for an intact session that simply may not see a page, and treating
/// that as a rejection would re-run the credential provider — for a
/// `command` binding a password-manager call, for a `prompt` binding a
/// dialog — on every ordinary permission error.
pub fn is_auth_rejection(err: &str) -> bool {
    err.contains(SESSION_EXPIRED_MARKER) || error_status(err) == Some(401)
}

/// Latching "the server rejected this session" flag, shared between an
/// adapter's HTTP client and the auth bridge that built it.
///
/// The bridge caches a client for the whole session, so a cookie that
/// expires mid-session would otherwise turn every later call into a 401
/// forever: nothing on the request path can reach the orchestrator, and
/// dropping the session was only ever a manual action. Clients therefore
/// funnel their responses through [`AuthRejection::check_status`], and the
/// bridge consults [`AuthRejection::is_rejected`] before handing the cached
/// client out again — that is what `session_cache: until-rejected` promises.
///
/// One flag belongs to exactly one client: it is set once and never reset,
/// because a rejected session stays rejected. The fresh client built by the
/// re-login starts with a fresh flag.
#[derive(Clone, Debug, Default)]
pub struct AuthRejection(Arc<AtomicBool>);

impl AuthRejection {
    pub fn new() -> Self {
        Self::default()
    }

    /// [`check_status`], additionally recording a rejected session.
    pub async fn check_status(
        &self,
        method: &str,
        url: &str,
        resp: Response,
    ) -> Result<Response, String> {
        check_status(method, url, resp).await.inspect_err(|e| {
            if is_auth_rejection(e) {
                self.0.store(true, Ordering::Relaxed);
            }
        })
    }

    /// Whether the server has rejected this session at any point.
    pub fn is_rejected(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Build the network-level error string when `reqwest::send()` itself
/// fails (DNS, TCP, TLS, timeout). Always includes the URL.
pub fn network_error(method: &str, url: &str, err: impl std::fmt::Display) -> String {
    let msg = format!("{method} {url}: {err}");
    write_line(&format!("ERROR {msg}"));
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn prune_keeps_retention_window_and_ignores_foreign_files() {
        let dir = std::env::temp_dir().join(format!("nyd-log-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for d in [
            "2026-07-10",
            "2026-07-11",
            "2026-07-12",
            "2026-07-13",
            "2026-07-14",
        ] {
            File::create(dir.join(format!("nyd-{d}.log"))).unwrap();
        }
        // A file that does not match the rotating-log naming must be left alone.
        File::create(dir.join("unrelated.txt")).unwrap();

        let now = Local.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        // retention 3 → keep today and the two days before it (12th–14th).
        prune_old(&dir, 3, now);

        let exists = |n: &str| dir.join(n).exists();
        assert!(!exists("nyd-2026-07-10.log"), "10th should be pruned");
        assert!(!exists("nyd-2026-07-11.log"), "11th should be pruned");
        assert!(exists("nyd-2026-07-12.log"), "12th should be kept");
        assert!(exists("nyd-2026-07-13.log"), "13th should be kept");
        assert!(exists("nyd-2026-07-14.log"), "today should be kept");
        assert!(exists("unrelated.txt"), "foreign file must be untouched");

        let _ = fs::remove_dir_all(&dir);
    }

    /// An SSO bounce must be named as an expired session, and the giant
    /// `SAMLRequest` query string must not leak into the message.
    #[test]
    fn redirect_to_sso_reads_as_session_expired_without_the_query() {
        let msg = redirect_reason(Some(
            "https://idp.example/0000/saml2?SAMLRequest=AAAA&RelayState=%2Fjira",
        ));
        assert!(msg.contains("session expired"), "got: {msg}");
        assert!(msg.contains("https://idp.example/0000/saml2"), "got: {msg}");
        assert!(!msg.contains("SAMLRequest"), "query string leaked: {msg}");
    }

    #[test]
    fn ordinary_redirect_is_not_reported_as_an_auth_problem() {
        let msg = redirect_reason(Some("https://jira.example/browse/ABC-1"));
        assert!(!msg.contains("session expired"), "got: {msg}");
        assert!(
            msg.contains("https://jira.example/browse/ABC-1"),
            "got: {msg}"
        );
    }

    #[test]
    fn redirect_without_location_still_explains_itself() {
        assert!(redirect_reason(None).contains("no Location header"));
    }

    /// Run a synthetic response through the real [`check_status`] and return
    /// its error, so the rejection tests below are coupled to the message the
    /// producer actually builds instead of a hand-written copy of it.
    async fn status_error(status: u16, location: Option<&str>) -> String {
        let mut builder = http::Response::builder().status(status);
        if let Some(location) = location {
            builder = builder.header("location", location);
        }
        let resp = Response::from(builder.body("nope").unwrap());
        check_status("GET", "https://jira.example/rest/api/2/myself", resp)
            .await
            .expect_err("non-2xx is an error")
    }

    #[tokio::test]
    async fn a_401_from_check_status_reads_as_a_rejected_session() {
        let err = status_error(401, None).await;
        assert!(is_auth_rejection(&err), "got: {err}");
    }

    #[tokio::test]
    async fn an_sso_login_bounce_reads_as_a_rejected_session() {
        let err = status_error(302, Some("https://idp.example/saml2?SAMLRequest=AAAA")).await;
        assert!(is_auth_rejection(&err), "got: {err}");
    }

    /// A 403 is Jira's answer for "you may not see this", not "log in again";
    /// treating it as a rejection would re-run the credential provider on
    /// every ordinary permission error.
    #[tokio::test]
    async fn a_403_is_not_a_rejected_session() {
        let err = status_error(403, None).await;
        assert!(!is_auth_rejection(&err), "got: {err}");
    }

    #[tokio::test]
    async fn ordinary_failures_are_not_rejected_sessions() {
        for status in [404, 429, 500] {
            let err = status_error(status, None).await;
            assert!(!is_auth_rejection(&err), "{status} got: {err}");
        }
        assert!(!is_auth_rejection(&network_error(
            "GET",
            "https://jira.example/rest/api/2/myself",
            "connection refused"
        )));
    }

    #[tokio::test]
    async fn the_flag_latches_only_on_a_rejection() {
        let flag = AuthRejection::new();
        assert!(!flag.is_rejected());

        let resp = Response::from(http::Response::builder().status(500).body("boom").unwrap());
        let _ = flag
            .check_status("GET", "https://jira.example/x", resp)
            .await;
        assert!(!flag.is_rejected(), "a 500 must not drop the session");

        let resp = Response::from(http::Response::builder().status(401).body("nope").unwrap());
        let _ = flag
            .check_status("GET", "https://jira.example/x", resp)
            .await;
        assert!(flag.is_rejected(), "a 401 must drop the session");
    }

    #[test]
    fn login_target_matching_ignores_case_and_looks_at_host_and_path() {
        assert!(is_login_target("login.microsoftonline.com/x/SAML2"));
        assert!(is_login_target("crowd.example/plugins/servlet/saml-login"));
        assert!(!is_login_target("jira.example/rest/api/2/issue/ABC-1"));
    }
}
