//! HTTP error formatting + opt-in request/error logging shared across
//! content adapters.
//!
//! Adapters wrap their `reqwest` calls with [`check_status`] and
//! [`network_error`] instead of the manual `if !resp.status().is_success()`
//! pattern, so error messages always include the URL.
//!
//! Set `NYD_DEBUG=1` in the environment to mirror every request, every
//! response status, and every error into a debug log
//! (default `/tmp/nyd-debug.log`, override with `NYD_DEBUG_LOG=/path`).
//! When the variable is unset, every function below is a no-op except
//! for the synchronous string formatting that `check_status` /
//! `network_error` need anyway.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use reqwest::Response;

const BODY_TRUNCATE_LEN: usize = 2000;

static DEBUG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

fn debug_file() -> Option<&'static Mutex<File>> {
    DEBUG_FILE
        .get_or_init(|| {
            if std::env::var_os("NYD_DEBUG").is_none() {
                return None;
            }
            let path = std::env::var("NYD_DEBUG_LOG")
                .unwrap_or_else(|_| "/tmp/nyd-debug.log".to_string());
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
}

fn write_line(line: &str) {
    let Some(lock) = debug_file() else { return };
    let Ok(mut f) = lock.lock() else { return };
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
    let _ = writeln!(f, "{ts} {line}");
}

/// Log an outbound request line.
pub fn log_request(method: &str, url: &str) {
    write_line(&format!("-> {method} {url}"));
}

/// Log a response status line.
pub fn log_response(method: &str, url: &str, status: u16) {
    write_line(&format!("<- {method} {url} [{status}]"));
}

/// Log a free-form error line (e.g. from the TUI's `notify_error` /
/// `set_query_error` paths) so the debug log mirrors the same error
/// stream the user sees in the UI.
pub fn log_error(context: &str, message: &str) {
    write_line(&format!("ERROR {context}: {message}"));
}

/// Log a free-form debug line. Same NYD_DEBUG gating as `log_error` —
/// no-op if the env var is unset.
pub fn log_debug(context: &str, message: &str) {
    write_line(&format!("DEBUG {context}: {message}"));
}

fn truncate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "<empty body>".to_string()
    } else if trimmed.len() > BODY_TRUNCATE_LEN {
        format!("{}…(truncated)", &trimmed[..BODY_TRUNCATE_LEN])
    } else {
        trimmed.to_string()
    }
}

/// Wrap a `reqwest::Response`: returns it unchanged on 2xx, otherwise
/// consumes the body and returns a uniform error string that always
/// includes method, URL, status, and a truncated response body. Also
/// logs the response status (and, on error, the formatted message).
pub async fn check_status(
    method: &str,
    url: &str,
    resp: Response,
) -> Result<Response, String> {
    let status = resp.status();
    log_response(method, url, status.as_u16());
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let snippet = truncate_body(&body);
    let msg = format!("{method} {url} -> {} {}: {snippet}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""));
    write_line(&format!("ERROR {msg}"));
    Err(msg)
}

/// Build the network-level error string when `reqwest::send()` itself
/// fails (DNS, TCP, TLS, timeout). Always includes the URL.
pub fn network_error(method: &str, url: &str, err: impl std::fmt::Display) -> String {
    let msg = format!("{method} {url}: {err}");
    write_line(&format!("ERROR {msg}"));
    msg
}
