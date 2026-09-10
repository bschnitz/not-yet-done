//! What an adapter is doing right now, said once and read by anyone.
//!
//! An adapter that takes its time owes the user two things: *what* it is
//! busy with, and *how long* it has been at it. Both travel on the status
//! channel every frontend already subscribes to
//! ([`ContentAdapter::subscribe_status`](crate::ContentAdapter::subscribe_status)),
//! so an adapter reports them without knowing whether a TUI, the CLI, or
//! nothing at all is listening — the reporter is a sink, not a callback into
//! a frontend.
//!
//! Two kinds of report live here, because a slow adapter is slow in two
//! different places:
//!
//! - **connecting** — the login phase, up to the point where the adapter has
//!   a usable session. It is the phase that can stall for minutes (an SSO
//!   script waiting for a browser), and the one the user has the least
//!   insight into, so it carries a named step and a per-step clock.
//! - **busy** — a request against an established connection. Announced with
//!   [`StatusReporter::busy`], which hands back a guard: the status returns
//!   to what it was when the guard drops, so no code path can leave a
//!   spinner running after its request came back.
//!
//! Neither is mandatory. An adapter that reports nothing behaves exactly as
//! before; the frontend then falls back to counting its own in-flight loads,
//! which can say "still loading" but never what for.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::AdapterStatus;

/// Wall-clock now in milliseconds since the Unix epoch — the unit every
/// timestamp on [`AdapterStatus`] uses, so an adapter's clock and a
/// frontend's are the same clock.
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whole seconds since a wall-clock stamp, floored at 0 — a clock that ran
/// backwards (an NTP step mid-login) shows `0s`, never a wrapped number.
pub fn elapsed_secs(started_at_unix_ms: u64) -> u64 {
    now_unix_ms().saturating_sub(started_at_unix_ms) / 1000
}

/// The connect phase as the adapter currently sees it. Kept whole so a
/// report about one part (a new attempt) does not wipe another (the step
/// name that attempt belongs to).
#[derive(Clone, Debug)]
struct ConnectState {
    retry: u32,
    max_retries: u32,
    timeout_secs: u64,
    started_at_unix_ms: u64,
    step: Option<String>,
    attention: bool,
}

impl ConnectState {
    fn fresh() -> Self {
        Self {
            retry: 1,
            max_retries: 1,
            timeout_secs: 0,
            started_at_unix_ms: now_unix_ms(),
            step: None,
            attention: false,
        }
    }

    fn to_status(&self) -> AdapterStatus {
        AdapterStatus::Connecting {
            retry: self.retry,
            max_retries: self.max_retries,
            timeout_secs: self.timeout_secs,
            started_at_unix_ms: self.started_at_unix_ms,
            step: self.step.clone(),
            attention: self.attention,
        }
    }
}

/// One announced `Busy`, plus what to go back to when it ends.
#[derive(Clone, Debug)]
struct BusyFrame {
    published: AdapterStatus,
    restore: AdapterStatus,
}

struct Inner {
    tx: watch::Sender<AdapterStatus>,
    connect: Mutex<ConnectState>,
    /// Innermost last. A nested announcement (a list that fetches a second
    /// resource) restores its caller's line rather than clearing the banner
    /// while the outer request is still out.
    busy: Mutex<Vec<BusyFrame>>,
}

/// An adapter's handle on its own status channel.
///
/// Cloneable and cheap: the auth layer, the client and the adapter itself
/// hold the same handle, so their reports arrive on one channel in the order
/// they happened. Create one per adapter *instance* — the status of one Jira
/// connection has nothing to say about another's.
#[derive(Clone)]
pub struct StatusReporter {
    inner: Arc<Inner>,
}

impl Default for StatusReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusReporter {
    /// A reporter over a fresh channel that starts out [`AdapterStatus::Idle`].
    pub fn new() -> Self {
        Self::over(watch::channel(AdapterStatus::Idle).0)
    }

    /// A reporter over an existing sender — for an adapter that already owns
    /// its channel and wants the auth layer to publish onto the same one.
    pub fn over(tx: watch::Sender<AdapterStatus>) -> Self {
        Self {
            inner: Arc::new(Inner {
                tx,
                connect: Mutex::new(ConnectState::fresh()),
                busy: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Subscribe to the reported statuses. Adapters hand this straight
    /// through their `subscribe_status`.
    pub fn subscribe(&self) -> watch::Receiver<AdapterStatus> {
        self.inner.tx.subscribe()
    }

    /// The status as last reported.
    pub fn current(&self) -> AdapterStatus {
        self.inner.tx.borrow().clone()
    }

    /// Publish a status verbatim. For the states that are not a phase of
    /// something ongoing — [`AdapterStatus::NeedsCreds`],
    /// [`AdapterStatus::Failed`] — where the caller knows the whole story.
    pub fn send(&self, status: AdapterStatus) {
        let _ = self.inner.tx.send(status);
    }

    /// A login has started: reset the phase and start its clock.
    pub fn begin_connect(&self) {
        let mut state = self.lock_connect();
        *state = ConnectState::fresh();
        let _ = self.inner.tx.send(state.to_status());
    }

    /// Name the phase the login is in — "running the login script",
    /// "checking the session". Restarts the clock, which therefore counts
    /// the *current* step rather than the whole login: a step that has been
    /// running for 90 seconds is the thing worth showing, and a login made
    /// of many quick steps shows that it is moving.
    pub fn connect_step(&self, step: impl Into<String>) {
        let mut state = self.lock_connect();
        state.step = Some(step.into());
        state.attention = false;
        state.started_at_unix_ms = now_unix_ms();
        let _ = self.inner.tx.send(state.to_status());
    }

    /// Enter a named phase that runs under its own limits — the usual case,
    /// where naming the step and stating its deadline are one event and
    /// should reach the frontend as one update rather than two.
    pub fn connect_phase(&self, step: impl Into<String>, max_retries: u32, timeout_secs: u64) {
        let mut state = self.lock_connect();
        state.step = Some(step.into());
        state.attention = false;
        state.retry = 1;
        state.max_retries = max_retries;
        state.timeout_secs = timeout_secs;
        state.started_at_unix_ms = now_unix_ms();
        let _ = self.inner.tx.send(state.to_status());
    }

    /// Which attempt the current step is on, and the deadline it runs under.
    /// `max_retries <= 1` and `timeout_secs == 0` both mean "no such limit",
    /// and a frontend leaves them out rather than printing a bound that does
    /// not exist.
    pub fn connect_attempt(&self, retry: u32, max_retries: u32, timeout_secs: u64) {
        let mut state = self.lock_connect();
        state.attention = false;
        state.retry = retry;
        state.max_retries = max_retries;
        state.timeout_secs = timeout_secs;
        state.started_at_unix_ms = now_unix_ms();
        let _ = self.inner.tx.send(state.to_status());
    }

    /// The login is waiting on the person, somewhere this program cannot
    /// reach — a push notification to approve, a hardware key to touch.
    /// `step` says what to do ("Approve the sign-in in your Authenticator");
    /// the flag on the status says nothing will move until they do it, so a
    /// frontend can be loud about a line it would otherwise let scroll past.
    ///
    /// Still a step, not a form: there is nothing to submit here, and the
    /// login carries on by itself once the person has acted. `timeout_secs`
    /// is the *longer* patience such a wait runs under — a deadline against
    /// a person is a different number from a deadline against a machine, and
    /// showing the machine's would be a countdown to a failure that is not
    /// coming. Whatever is reported next takes the flag back.
    pub fn connect_attention(&self, step: impl Into<String>, timeout_secs: u64) {
        let mut state = self.lock_connect();
        state.step = Some(step.into());
        state.attention = true;
        state.retry = 1;
        state.max_retries = 1;
        state.timeout_secs = timeout_secs;
        state.started_at_unix_ms = now_unix_ms();
        let _ = self.inner.tx.send(state.to_status());
    }

    /// The adapter is connected and idle.
    pub fn ready(&self) {
        let _ = self.inner.tx.send(AdapterStatus::Ready);
    }

    /// The connect phase is over: the adapter has a usable connection.
    ///
    /// The counterpart of [`begin_connect`](Self::begin_connect), and the
    /// call every login path owes: a `Connecting` nobody ends keeps its
    /// clock running forever, and — because it is what an announcement
    /// restores to — reappears under every finished request.
    ///
    /// Unlike [`ready`](Self::ready) this only ends a *connect*. A status
    /// that arrived after the last connect report (a request already
    /// announcing itself, an auth rejection) is newer and stays.
    pub fn connected(&self) {
        let connecting = matches!(*self.inner.tx.borrow(), AdapterStatus::Connecting { .. });
        if connecting {
            let _ = self.inner.tx.send(AdapterStatus::Ready);
        }
    }

    /// The login gave up. `reason` is shown to the user as-is.
    pub fn failed(&self, reason: impl Into<String>) {
        let _ = self.inner.tx.send(AdapterStatus::Failed {
            reason: reason.into(),
        });
    }

    /// Announce a request against the established connection, e.g.
    /// `busy("Loading issues", 0)`. The returned guard ends the
    /// announcement when it drops — including on the `?` that leaves the
    /// function early, which is the whole reason it is a guard.
    ///
    /// `timeout_secs` is the deadline the request actually runs under, or
    /// `0` when it has none.
    ///
    /// Announcements nest as a stack, which is the right shape for a request
    /// that makes a smaller one on the way. Two *concurrent* requests share
    /// one line: the later one names it until it ends, then the earlier one
    /// does. A status line has room for one thing at a time, and the newest
    /// is the honest choice.
    pub fn busy(&self, label: impl Into<String>, timeout_secs: u64) -> BusyGuard {
        let published = AdapterStatus::Busy {
            label: label.into(),
            started_at_unix_ms: now_unix_ms(),
            timeout_secs,
            progress: None,
        };
        let restore = self.current();
        self.lock_busy().push(BusyFrame {
            published: published.clone(),
            restore,
        });
        let _ = self.inner.tx.send(published);
        BusyGuard {
            reporter: self.clone(),
        }
    }

    /// Update the completion estimate of the innermost announcement, for a
    /// load that arrives in pieces. Ignored when nothing is announced.
    pub fn set_progress(&self, progress: Option<f32>) {
        let mut frames = self.lock_busy();
        let Some(frame) = frames.last_mut() else {
            return;
        };
        if let AdapterStatus::Busy { progress: p, .. } = &mut frame.published {
            *p = progress;
        }
        let published = frame.published.clone();
        drop(frames);
        // Only if the announcement is still the visible one — an auth
        // rejection that landed meanwhile outranks a percentage.
        if self.is_innermost_visible(&published) {
            let _ = self.inner.tx.send(published);
        }
    }

    /// End the innermost announcement, restoring what it interrupted.
    ///
    /// A status that arrived *while* the request was out (a `Failed` from a
    /// mid-flight auth rejection) is left standing: it is newer than what we
    /// saved, and outranks the news that a request finished.
    fn end_busy(&self) {
        let Some(frame) = self.lock_busy().pop() else {
            return;
        };
        if *self.inner.tx.borrow() == frame.published {
            let _ = self.inner.tx.send(frame.restore);
        }
    }

    fn is_innermost_visible(&self, published: &AdapterStatus) -> bool {
        match (&*self.inner.tx.borrow(), published) {
            (AdapterStatus::Busy { label: shown, .. }, AdapterStatus::Busy { label: ours, .. }) => {
                shown == ours
            }
            _ => false,
        }
    }

    fn lock_connect(&self) -> std::sync::MutexGuard<'_, ConnectState> {
        self.inner.connect.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_busy(&self) -> std::sync::MutexGuard<'_, Vec<BusyFrame>> {
        self.inner.busy.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Keeps one `Busy` announcement alive for as long as the request is out.
/// See [`StatusReporter::busy`].
pub struct BusyGuard {
    reporter: StatusReporter,
}

impl BusyGuard {
    /// Set the completion estimate of this announcement, in `[0, 1]`.
    pub fn progress(&self, fraction: f32) {
        self.reporter.set_progress(Some(fraction));
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.reporter.end_busy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_reported_with_a_clock_that_starts_at_that_step() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.connect_step("running the login script");
        match &*rx.borrow() {
            AdapterStatus::Connecting {
                step,
                started_at_unix_ms,
                ..
            } => {
                assert_eq!(step.as_deref(), Some("running the login script"));
                assert!(*started_at_unix_ms > 0, "the step's clock must be stamped");
            }
            other => panic!("expected Connecting, got {other:?}"),
        }
    }

    #[test]
    fn an_attempt_keeps_the_step_it_belongs_to() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.connect_step("running the login script");
        r.connect_attempt(2, 3, 120);
        match &*rx.borrow() {
            AdapterStatus::Connecting {
                step,
                retry,
                max_retries,
                timeout_secs,
                ..
            } => {
                assert_eq!(step.as_deref(), Some("running the login script"));
                assert_eq!((*retry, *max_retries, *timeout_secs), (2, 3, 120));
            }
            other => panic!("expected Connecting, got {other:?}"),
        }
    }

    #[test]
    fn waiting_on_the_person_is_a_step_that_says_so() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.connect_step("signing in");
        r.connect_attention("Approve the sign-in in your Authenticator", 300);
        match &*rx.borrow() {
            AdapterStatus::Connecting {
                step,
                attention,
                timeout_secs,
                ..
            } => {
                assert_eq!(
                    step.as_deref(),
                    Some("Approve the sign-in in your Authenticator")
                );
                assert!(*attention, "nothing moves until the person acts");
                assert_eq!(
                    *timeout_secs, 300,
                    "the countdown shown must be the one a person is given"
                );
            }
            other => panic!("expected Connecting, got {other:?}"),
        }
    }

    #[test]
    fn the_next_step_takes_the_attention_back() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.connect_attention("Approve the sign-in in your Authenticator", 300);
        r.connect_step("fetching the cookie");
        assert!(
            matches!(
                &*rx.borrow(),
                AdapterStatus::Connecting {
                    attention: false,
                    ..
                }
            ),
            "a login that moved on must not keep asking for a tap"
        );
    }

    #[test]
    fn a_finished_request_gives_the_status_back() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.ready();
        {
            let _busy = r.busy("Loading issues", 0);
            assert!(
                matches!(&*rx.borrow(), AdapterStatus::Busy { label, .. } if label == "Loading issues")
            );
        }
        assert_eq!(*rx.borrow(), AdapterStatus::Ready);
    }

    #[test]
    fn a_nested_request_gives_back_its_callers_line_not_a_blank_one() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.ready();
        let _outer = r.busy("Loading issues", 0);
        {
            let _inner = r.busy("Looking up custom fields", 0);
            assert!(
                matches!(&*rx.borrow(), AdapterStatus::Busy { label, .. } if label == "Looking up custom fields")
            );
        }
        assert!(
            matches!(&*rx.borrow(), AdapterStatus::Busy { label, .. } if label == "Loading issues"),
            "the outer request is still out and must keep saying so"
        );
    }

    #[test]
    fn news_that_arrived_during_the_request_outranks_the_restore() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.ready();
        {
            let _busy = r.busy("Loading issues", 0);
            r.failed("session rejected");
        }
        assert!(
            matches!(&*rx.borrow(), AdapterStatus::Failed { .. }),
            "a failure that landed mid-request must not be overwritten by Ready"
        );
    }

    #[test]
    fn a_finished_connect_stops_the_clock() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.connect_step("checking the session");
        r.connected();
        assert_eq!(
            *rx.borrow(),
            AdapterStatus::Ready,
            "a connect nobody ends keeps counting and reappears under every request"
        );
    }

    #[test]
    fn a_finished_connect_does_not_overwrite_newer_news() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        r.failed("session rejected");
        r.connected();
        assert!(
            matches!(&*rx.borrow(), AdapterStatus::Failed { .. }),
            "the connect is over either way, but the reason must survive"
        );

        let r = StatusReporter::new();
        let rx = r.subscribe();
        r.begin_connect();
        let _busy = r.busy("Loading issues", 0);
        r.connected();
        assert!(
            matches!(&*rx.borrow(), AdapterStatus::Busy { label, .. } if label == "Loading issues"),
            "a request that already announced itself is the newer line"
        );
    }

    #[test]
    fn a_progress_estimate_updates_the_running_announcement() {
        let r = StatusReporter::new();
        let rx = r.subscribe();
        let busy = r.busy("Loading calendar", 0);
        busy.progress(0.5);
        match &*rx.borrow() {
            AdapterStatus::Busy { progress, .. } => assert_eq!(*progress, Some(0.5)),
            other => panic!("expected Busy, got {other:?}"),
        }
    }
}
