//! Sending a request, and repeating it when the first attempt produced no
//! answer at all.
//!
//! A transport failure — the connection refused, the TLS handshake lost, the
//! answer never arriving — says nothing about the server's health a second
//! later. Most of them are gone by the time the user has read the error, which
//! is why the honest fix for the class is to try again rather than to hand the
//! user a message.
//!
//! What must not be repeated blindly is the *effect*. Two failures look alike
//! from here and mean opposite things:
//!
//! - the connection was never established — the request provably never
//!   reached the server, so sending it again cannot duplicate anything;
//! - the answer did not arrive in time — the request may well have arrived and
//!   run to completion, and only the answer was lost. Repeating a transition,
//!   a comment, or a create here means doing it twice.
//!
//! So a repeat after anything but a connect failure needs the call site to
//! declare itself side-effect free ([`Repeat::Safe`]). That is not the same as
//! "is a GET": Jira's issue search is a `POST` carrying a JQL body and reads
//! nothing but data, and it is precisely the call that hangs when the network
//! hiccups.
//!
//! Adapters configure the policy in their YAML as a `retry:` block; see
//! [`RetryConfig`].

use std::time::Duration;

use fieldsmith::Buildable;
use reqwest::{RequestBuilder, Response};
use serde::Deserialize;

use crate::http_log;

/// Attempts per request when the config says nothing: the original plus one
/// repeat. Enough to ride out a single hiccup without doubling the worst case
/// more than once.
const DEFAULT_ATTEMPTS: u32 = 2;
/// Wait before the second attempt when the config says nothing.
const DEFAULT_BACKOFF_MS: u64 = 250;

fn default_attempts() -> u32 {
    DEFAULT_ATTEMPTS
}

fn default_backoff_ms() -> u64 {
    DEFAULT_BACKOFF_MS
}

/// The `retry:` block of an adapter's YAML config.
///
/// ```yaml
/// retry:
///   attempts: 3
///   backoff_ms: 250
/// ```
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    /// Attempts per request, the first one included. `1` switches repeating
    /// off entirely.
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Wait before the second attempt, in milliseconds, doubled before each
    /// further one. Keep it short: this sits in front of the user, who is
    /// staring at a pane that has not filled in yet.
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_ATTEMPTS,
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }
}

/// Whether repeating a request is safe when its outcome is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    /// The call only reads — a search, a listing, a single fetch. Sending it
    /// twice can at worst cost time.
    Safe,
    /// The call changes something on the server. It is repeated only when the
    /// request provably never arrived.
    Unsafe,
}

impl Repeat {
    /// The safety HTTP itself promises for a method. The right answer for the
    /// ordinary case, and the wrong one for a read that happens to be posted —
    /// those call sites pass [`Repeat::Safe`] themselves.
    pub fn of_method(method: &str) -> Self {
        match method.to_ascii_uppercase().as_str() {
            "GET" | "HEAD" | "OPTIONS" => Repeat::Safe,
            _ => Repeat::Unsafe,
        }
    }
}

/// Send a request, repeating it per `retry` while the failure is one worth
/// repeating, and turn a final failure into the usual
/// [`http_log::network_error`] string.
///
/// Only *transport* failures are repeated. A response is a response: a 500 or
/// a 429 comes back to the caller untouched, because deciding what a status
/// means is [`http_log::check_status`]'s job and retrying a server error
/// behind the caller's back would hide it.
pub async fn send(
    retry: &RetryConfig,
    repeat: Repeat,
    method: &str,
    url: &str,
    req: RequestBuilder,
) -> Result<Response, String> {
    let attempts = retry.attempts.max(1);
    let mut backoff = Duration::from_millis(retry.backoff_ms);
    let mut req = req;

    for attempt in 1..attempts {
        // A streaming body cannot be copied, so such a request is sent once
        // whatever the policy says — better than holding it in memory for a
        // repeat that may never happen.
        let Some(again) = req.try_clone() else { break };
        http_log::log_request(method, url);
        match req.send().await {
            Ok(resp) => return Ok(resp),
            Err(err) if worth_repeating(&err, repeat) => {
                http_log::log_retry(method, url, attempt, attempts, &err);
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2);
                req = again;
            }
            Err(err) => return Err(http_log::network_error(method, url, err)),
        }
    }

    http_log::log_request(method, url);
    req.send()
        .await
        .map_err(|e| http_log::network_error(method, url, e))
}

/// Whether this failure is one where sending the same request again is both
/// harmless and likely to help.
fn worth_repeating(err: &reqwest::Error, repeat: Repeat) -> bool {
    if err.is_builder() {
        // The request never took shape — our own bug. Repeating it produces
        // the identical error, slower.
        return false;
    }
    if err.is_connect() {
        // Nothing reached the server, so no effect can have happened there.
        return true;
    }
    repeat == Repeat::Safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn fast(attempts: u32) -> RetryConfig {
        RetryConfig {
            attempts,
            backoff_ms: 1,
        }
    }

    /// A `reqwest::Error` from a port nothing listens on.
    async fn connect_failure() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("nothing listens on port 1")
    }

    /// A `reqwest::Error` from a server that accepts the connection and then
    /// never answers — the shape a hanging Jira produces.
    async fn timeout_failure() -> reqwest::Error {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = std::thread::spawn(move || {
            let _accepted = listener.accept();
            std::thread::sleep(Duration::from_millis(500));
        });
        let err = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("the server never answers");
        let _ = hold.join();
        err
    }

    #[tokio::test]
    async fn a_connect_failure_is_repeated_even_for_a_writing_call() {
        let err = connect_failure().await;
        assert!(err.is_connect(), "expected a connect error: {err}");
        assert!(
            worth_repeating(&err, Repeat::Unsafe),
            "the request never reached the server, so no effect can be doubled"
        );
    }

    /// The one that matters: a timeout may mean the server did the work and
    /// only the answer was lost, so a create/transition must not be replayed.
    #[tokio::test]
    async fn a_timeout_is_repeated_only_for_a_reading_call() {
        let err = timeout_failure().await;
        assert!(err.is_timeout(), "expected a timeout: {err}");
        assert!(worth_repeating(&err, Repeat::Safe));
        assert!(!worth_repeating(&err, Repeat::Unsafe));
    }

    #[tokio::test]
    async fn attempts_of_one_sends_once() {
        let started = std::time::Instant::now();
        let err = send(
            &RetryConfig {
                attempts: 1,
                backoff_ms: 10_000,
            },
            Repeat::Safe,
            "GET",
            "http://127.0.0.1:1/",
            reqwest::Client::new().get("http://127.0.0.1:1/"),
        )
        .await
        .expect_err("nothing listens there");
        assert!(err.contains("127.0.0.1:1"), "names the url: {err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "no backoff may be waited when repeating is switched off"
        );
    }

    /// End to end through the real loop: a server that drops the first
    /// connection mid-request and answers the second.
    #[tokio::test]
    async fn a_reading_call_survives_a_dropped_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut incoming = listener.incoming();
            // First attempt: read the request line, then hang up on it.
            if let Some(Ok(first)) = incoming.next() {
                let mut buf = [0u8; 64];
                let _ = (&first).read(&mut buf);
                drop(first);
            }
            // Second attempt: a well-formed empty 200.
            if let Some(Ok(mut second)) = incoming.next() {
                let mut buf = [0u8; 1024];
                let _ = second.read(&mut buf);
                let _ = second.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = second.flush();
            }
        });

        let url = format!("http://{addr}/rest/api/2/search");
        let client = reqwest::Client::new();
        let resp = send(
            &fast(3),
            Repeat::Safe,
            "POST",
            &url,
            client.post(&url).body("{}"),
        )
        .await
        .expect("the repeat reaches the server that is now answering");
        assert_eq!(resp.status(), 200);
        let _ = server.join();
    }

    /// The same failure, for a call that writes: handed back rather than
    /// silently done twice.
    #[tokio::test]
    async fn a_writing_call_is_not_repeated_after_a_dropped_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let Ok(stream) = stream else { break };
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 64];
                let _ = (&stream).read(&mut buf);
                drop(stream);
            }
        });

        let url = format!("http://{addr}/rest/api/2/issue");
        let client = reqwest::Client::new();
        let err = send(
            &fast(3),
            Repeat::Unsafe,
            "POST",
            &url,
            client.post(&url).body("{}"),
        )
        .await
        .expect_err("the connection died mid-request");
        assert!(err.contains(&url), "names the url: {err}");
        // Read the count before unblocking: the connection that frees the
        // server's accept loop would otherwise be counted as an attempt.
        let reached_server = seen.load(std::sync::atomic::Ordering::SeqCst);
        let _ = TcpStream::connect(addr);
        let _ = server.join();
        assert_eq!(
            reached_server, 1,
            "a writing call must reach the server exactly once"
        );
    }

    #[test]
    fn method_safety_follows_http() {
        assert_eq!(Repeat::of_method("get"), Repeat::Safe);
        assert_eq!(Repeat::of_method("HEAD"), Repeat::Safe);
        assert_eq!(Repeat::of_method("POST"), Repeat::Unsafe);
        assert_eq!(Repeat::of_method("DELETE"), Repeat::Unsafe);
    }

    #[test]
    fn an_absent_retry_block_means_one_repeat() {
        let cfg: RetryConfig = serde_yaml::from_str("{}").expect("all fields default");
        assert_eq!(cfg, RetryConfig::default());
        assert_eq!(cfg.attempts, 2);
    }

    #[test]
    fn a_partial_retry_block_keeps_the_other_default() {
        let cfg: RetryConfig = serde_yaml::from_str("attempts: 4").expect("parses");
        assert_eq!(cfg.attempts, 4);
        assert_eq!(cfg.backoff_ms, DEFAULT_BACKOFF_MS);
    }

    #[test]
    fn a_misspelled_retry_field_is_rejected() {
        let err = serde_yaml::from_str::<RetryConfig>("attempt: 4")
            .err()
            .expect("deny_unknown_fields");
        assert!(err.to_string().contains("attempt"), "got: {err}");
    }
}
