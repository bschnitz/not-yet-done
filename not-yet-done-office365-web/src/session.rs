//! The session actor: owns one browser and serialises all access to it. A
//! [`SessionHandle`] is a cheap, clonable reference to the actor; when the
//! last handle drops, the actor task is aborted and its browser is asked to
//! quit (and killed if it does not).
//!
//! Access is serialised on purpose: a single browser session can't service
//! concurrent operations safely — a window has one flow open and one run
//! going — so the actor processes one command at a time.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::browser::{Browser, Happening, Link};
use crate::calendar;
use crate::dto::{MsCalEvent, MsTimeRange};
use crate::error::MsOfficeError;

/// How the browser is started and what it runs.
#[derive(Clone, Debug)]
pub struct BrowserConfig {
    /// The `drunken-browser` binary (default: `drunken-browser`, resolved via `PATH`).
    pub bin: PathBuf,
    /// The flow a calendar read runs: a name on the browser's shelf or a path to a flow
    /// directory, as `test-open` takes it. Default `nyd-calendar`.
    pub flow: String,
    /// How long one run may take, sign-on included. Generous by default: a sign-on that
    /// needs a second factor waits for a person to walk over to their phone, and the flow
    /// bounds each of its own steps anyway — this is the net under a window that stopped
    /// moving altogether.
    pub run_timeout: Duration,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            bin: PathBuf::from("drunken-browser"),
            flow: "nyd-calendar".into(),
            run_timeout: Duration::from_secs(20 * 60),
        }
    }
}

/// What this session answers when the run asks for a secret it has no command for —
/// `{{secret.NAME}}` answered with the value under `NAME`.
///
/// These are *already-resolved* secret values (from `pass`, a keyring, etc.), not a
/// provider spec — the wrapper stays app-agnostic and never learns where they came from.
/// They travel to the browser only as the answer to its own question, never on the
/// browser's command line or in its environment. Never logged or Debug-printed.
#[derive(Clone, Default)]
pub struct Answers(BTreeMap<String, String>);

impl Answers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer `{{secret.<name>}}` with `value`.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.insert(name.into(), value.into());
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

impl FromIterator<(String, String)> for Answers {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::fmt::Debug for Answers {
    /// Names only, so a secret never reaches a log or panic message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// Everything needed to open (or look up) a session.
///
/// `account_key` is the registry key: two configs sharing a key share one
/// browser session. `profile_dir` is the persistent browser profile so a login
/// (and its SSO cookies) survives restarts.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub account_key: String,
    pub profile_dir: PathBuf,
    /// Resting display mode: `true` keeps the browser hidden for silent
    /// polls, `false` always shows the window.
    pub headless: bool,
    /// When resting hidden, put the window up the moment a run has something to say to
    /// the person (a second factor to approve), and take it down again once the run is
    /// over. Ignored when `headless` is `false` (already visible).
    pub auto_headed: bool,
    /// What every run is about, beyond the range: read as `{{data.NAME}}` — the
    /// `account` the flow signs on as, for one. Never a secret.
    pub facts: BTreeMap<String, String>,
    /// Secrets the session answers the run's questions with. `None`/empty leaves every
    /// question to whoever attends the session.
    pub answers: Answers,
    pub browser: BrowserConfig,
}

/// Progress of the browser session's first load, pushed as that run goes through the
/// flow's steps.
///
/// `fraction` is a best-effort completion estimate in `(0, 1]` — steps done over steps
/// in the flow. `done` marks the terminal push: the run is over. Only the browser's first
/// run is announced (see [`SessionHandle::subscribe_loaded`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadStatus {
    pub fraction: Option<f32>,
    pub done: bool,
}

/// What a [`SessionPrompt`] asks of the user. App-agnostic on purpose — the
/// wrapper never learns how a frontend renders it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Nothing to answer: the run said something to the person (a number-match
    /// second factor — approve on the phone, the run goes on by itself).
    Acknowledge,
    /// A typed value is expected (a one-time code). `secret` masks the input.
    Text { secret: bool },
}

/// A mid-run request the browser session raises when the run has something
/// to say to the user, or needs something from them, before it can continue
/// (chiefly a second factor during the sign-on). Delivered on the stream from
/// [`SessionHandle::take_prompts`]; the consumer shows it and, for a
/// [`PromptKind::Text`], calls one of the answer methods, which hands the
/// value to the run.
///
/// App-agnostic: it reports the run's *facts* — what was said, or what is asked for, as
/// the hole is written — and owns the reply plumbing, but decides nothing about how a
/// frontend phrases or shows it.
pub struct SessionPrompt {
    kind: PromptKind,
    detail: Option<String>,
    /// Where a typed answer goes. Taken by the answer methods; still here on drop means
    /// the consumer let the prompt go, and the run is told so rather than left waiting.
    run: Option<Arc<Link>>,
}

impl SessionPrompt {
    /// What input is expected.
    pub fn kind(&self) -> PromptKind {
        self.kind
    }

    /// What to show the user: the sentence the run said, for an
    /// [`Acknowledge`](PromptKind::Acknowledge); the name of the value asked
    /// for (`{{secret.otp}}`), for a [`Text`](PromptKind::Text).
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Answer a [`PromptKind::Text`] prompt with the typed value.
    pub async fn answer_text(mut self, value: String) -> Result<(), MsOfficeError> {
        match self.run.take() {
            Some(link) => link.answer(Some(value)).await,
            None => Ok(()),
        }
    }

    /// Acknowledge a [`PromptKind::Acknowledge`] prompt (no value). Nothing travels: the
    /// run was not waiting for it.
    pub async fn acknowledge(mut self) -> Result<(), MsOfficeError> {
        self.run.take();
        Ok(())
    }

    /// Signal that the user dismissed the prompt without answering; the run is refused
    /// the value and ends with the question.
    pub async fn cancel(mut self) -> Result<(), MsOfficeError> {
        match self.run.take() {
            Some(link) => link.answer(None).await,
            None => Ok(()),
        }
    }
}

impl Drop for SessionPrompt {
    /// A prompt let go unanswered refuses the run its value, so a consumer that dropped
    /// the stream (no frontend) leaves no run hanging on a question nobody will answer.
    fn drop(&mut self) {
        if let Some(link) = self.run.take() {
            tokio::spawn(async move {
                let _ = link.answer(None).await;
            });
        }
    }
}

/// A cheap, clonable reference to a live session actor.
#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) inner: Arc<SessionInner>,
}

impl SessionHandle {
    /// Calendar operations on this session.
    pub fn calendar(&self) -> crate::calendar::CalendarApi {
        crate::calendar::CalendarApi::new(self.inner.clone())
    }

    /// Subscribe to the browser session's load-progress stream.
    ///
    /// Fires as the browser's **first** run goes through the flow: a [`LoadStatus`] per
    /// step done, then a terminal `done` push once the run is over. A consumer that showed
    /// empty data during login can listen here and re-fetch the instant the calendar is
    /// available, and surface a progress banner while it runs — instead of waiting for a
    /// periodic poll.
    ///
    /// Later runs are silent. They are fetches the consumer asked for itself, and a
    /// consumer that re-fetches on `done` would otherwise be answered with another `done`
    /// and fetch forever — seen 2026-09-07, a run every five seconds. A browser started
    /// afresh (the old one gone) announces its first run again, since that is the run that
    /// may have to sign on.
    pub fn subscribe_loaded(&self) -> broadcast::Receiver<LoadStatus> {
        self.inner.loaded_tx.subscribe()
    }

    /// Put the browser's window up if it is down, take it down if it is up. Returns
    /// whether it is up now. For whoever attends the session and wants to see, or stop
    /// seeing, what the browser is doing — a sign-on that got stuck, say.
    ///
    /// Only toggles: a session whose browser has not been started yet (nothing fetched so
    /// far) has no window to show and says so, rather than starting one for the purpose.
    pub async fn toggle_window(&self) -> Result<bool, MsOfficeError> {
        self.inner.toggle_window().await
    }

    /// Take the session's mid-run prompt stream (see [`SessionPrompt`]).
    /// **Single-consumer**: each prompt carries a one-shot reply path, so there
    /// is exactly one receiver. Callable once — later calls (and a session that
    /// raises no prompts) return `None`.
    pub fn take_prompts(&self) -> Option<mpsc::Receiver<SessionPrompt>> {
        self.inner.prompt_rx.lock().unwrap().take()
    }
}

/// The shared, ref-counted session state behind every [`SessionHandle`].
pub(crate) struct SessionInner {
    tx: mpsc::Sender<Command>,
    /// Broadcasts the run's progress ([`LoadStatus`]). Held here (not just inside the
    /// actor) so a [`SessionHandle`] can subscribe before the browser — hence the first
    /// push — even exists.
    pub(crate) loaded_tx: broadcast::Sender<LoadStatus>,
    /// The single receiver for [`SessionPrompt`]s, parked here until a consumer
    /// takes it via [`SessionHandle::take_prompts`]. `mpsc` (not broadcast)
    /// because each prompt owns a one-shot reply path; `Mutex<Option<…>>` gives
    /// the take-once semantics. The matching sender lives in the actor's forwarder.
    prompt_rx: std::sync::Mutex<Option<mpsc::Receiver<SessionPrompt>>>,
    // Aborts the actor task (and thus drops the browser) when the last handle
    // to this session is dropped.
    _actor: AbortOnDrop,
}

impl SessionInner {
    /// Start a new session actor for `config`. No browser yet: it is started on the
    /// first command, so merely holding a handle costs nothing.
    pub(crate) fn spawn(config: SessionConfig) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(32);
        let (loaded_tx, _) = broadcast::channel::<LoadStatus>(16);
        let (prompt_tx, prompt_rx) = mpsc::channel::<SessionPrompt>(8);
        let actor = tokio::spawn(actor_loop(config, rx, loaded_tx.clone(), prompt_tx));
        Arc::new(Self {
            tx,
            loaded_tx,
            prompt_rx: std::sync::Mutex::new(Some(prompt_rx)),
            _actor: AbortOnDrop(actor),
        })
    }

    pub(crate) async fn get_calendar_view(
        &self,
        range: MsTimeRange,
    ) -> Result<Vec<MsCalEvent>, MsOfficeError> {
        let (resp, rx) = oneshot::channel();
        self.tx
            .send(Command::GetCalendarView { range, resp })
            .await
            .map_err(|_| MsOfficeError::Other("session actor is gone".into()))?;
        rx.await
            .map_err(|_| MsOfficeError::Other("session actor dropped the response".into()))?
    }

    pub(crate) async fn toggle_window(&self) -> Result<bool, MsOfficeError> {
        let (resp, rx) = oneshot::channel();
        self.tx
            .send(Command::ToggleWindow { resp })
            .await
            .map_err(|_| MsOfficeError::Other("session actor is gone".into()))?;
        rx.await
            .map_err(|_| MsOfficeError::Other("session actor dropped the response".into()))?
    }
}

/// Commands the actor processes, one at a time.
enum Command {
    GetCalendarView {
        range: MsTimeRange,
        resp: oneshot::Sender<Result<Vec<MsCalEvent>, MsOfficeError>>,
    },
    /// Flip the window; answered with whether it is up now.
    ToggleWindow {
        resp: oneshot::Sender<Result<bool, MsOfficeError>>,
    },
}

/// What a toggle is told while there is no browser to have a window.
const NO_BROWSER_YET: &str = "no browser yet: nothing has been fetched through this session";

/// Owns the browser and answers commands sequentially. The browser is started
/// lazily on the first command.
async fn actor_loop(
    config: SessionConfig,
    mut rx: mpsc::Receiver<Command>,
    loaded_tx: broadcast::Sender<LoadStatus>,
    prompt_tx: mpsc::Sender<SessionPrompt>,
) {
    let mut browser: Option<Browser> = None;
    // Kept alive for the actor's lifetime: relays what the run says and asks. Spawned the
    // first time the browser is started (see `ensure_browser`).
    let mut _fwd: Option<AbortOnDrop> = None;
    let mut announcing = Announcing::default();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::GetCalendarView { range, resp } => {
                let result =
                    match ensure_browser(&mut browser, &mut _fwd, &config, &prompt_tx).await {
                        Ok(b) => {
                            let mut data = config.facts.clone();
                            data.extend(calendar::data_for(&range));
                            let loaded = loaded_tx.clone();
                            let outcome = b
                                .run(
                                    &config.browser.flow,
                                    &data,
                                    config.browser.run_timeout,
                                    |done, steps| {
                                        if let Some(status) = announcing.step(done, steps) {
                                            let _ = loaded.send(status);
                                        }
                                    },
                                )
                                .await;
                            // Over either way: the banner a consumer raised comes down.
                            if let Some(status) = announcing.over() {
                                let _ = loaded_tx.send(status);
                            }
                            // A browser that is gone is started afresh next time rather than
                            // asked again.
                            if matches!(outcome, Err(MsOfficeError::Browser(_))) {
                                browser = None;
                                _fwd = None;
                                announcing.browser_gone();
                            }
                            outcome.and_then(calendar::events_of)
                        }
                        Err(e) => Err(e),
                    };
                let _ = resp.send(result);
            }
            Command::ToggleWindow { resp } => {
                let result = match browser.as_ref() {
                    Some(b) => b.link().toggle().await,
                    None => Err(MsOfficeError::Browser(NO_BROWSER_YET.into())),
                };
                let _ = resp.send(result);
            }
        }
    }
}

/// Which runs the load stream announces, and what it says of them.
///
/// Only a browser's first run (see [`SessionHandle::subscribe_loaded`]): that is the run
/// that may have to sign on, and the one a consumer showing nothing yet is waiting for.
/// Every later run is a fetch the consumer asked for itself, and a consumer that re-fetches
/// on `done` must not be told `done` by the fetch it triggered — that was a run every five
/// seconds (2026-09-07). Apart from the actor so that it can be tested without a browser.
#[derive(Debug, Default)]
struct Announcing {
    /// Whether the browser now running has had its first run.
    first_run_over: bool,
}

impl Announcing {
    /// What to push for `done` of `steps` steps gone through — or nothing.
    ///
    /// Nothing for a run that is not announced, and nothing before a step is done: the
    /// consumer announced the load when it asked for the session, and a fraction-less
    /// push is what makes it fetch again.
    fn step(&self, done: usize, steps: usize) -> Option<LoadStatus> {
        if self.first_run_over {
            return None;
        }
        match (done, steps) {
            (0, _) | (_, 0) => None,
            (d, s) => Some(LoadStatus {
                fraction: Some(d as f32 / s as f32),
                done: false,
            }),
        }
    }

    /// The run is over: the terminal push, if this run was announced. No run after it is.
    fn over(&mut self) -> Option<LoadStatus> {
        let announced = !self.first_run_over;
        self.first_run_over = true;
        announced.then_some(LoadStatus {
            fraction: Some(1.0),
            done: true,
        })
    }

    /// The browser is gone. The next one's first run is announced again, since it may
    /// have to sign on.
    fn browser_gone(&mut self) {
        self.first_run_over = false;
    }
}

/// Lazily start the browser, reusing it across commands. On first start, also
/// start the forwarder that relays the run's `told`/`asking` news as [`SessionPrompt`]s.
async fn ensure_browser<'a>(
    slot: &'a mut Option<Browser>,
    fwd: &mut Option<AbortOnDrop>,
    config: &SessionConfig,
    prompt_tx: &mpsc::Sender<SessionPrompt>,
) -> Result<&'a mut Browser, MsOfficeError> {
    if slot.is_none() {
        let browser = Browser::start(config).await?;
        let link = browser.link();
        let news = link.news();
        *fwd = Some(AbortOnDrop(tokio::spawn(forward(
            link,
            news,
            Relay {
                show_for_a_word: config.headless && config.auto_headed,
                answers: config.answers.clone(),
            },
            prompt_tx.clone(),
        ))));
        *slot = Some(browser);
    }
    Ok(slot.as_mut().expect("just populated"))
}

/// How the forwarder treats what the run says and asks.
struct Relay {
    /// Put the window up when the run has something to say, and take it down when the
    /// run is over.
    show_for_a_word: bool,
    answers: Answers,
}

/// Turn the run's `told` and `asking` news into [`SessionPrompt`]s — except a question this
/// session can answer itself, which is answered without anyone hearing of it. Ends when the
/// link closes (browser gone).
async fn forward(
    link: Arc<Link>,
    mut news: broadcast::Receiver<Happening>,
    relay: Relay,
    prompt_tx: mpsc::Sender<SessionPrompt>,
) {
    let mut shown = false;
    loop {
        let happening = match news.recv().await {
            Ok(h) => h,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        match happening {
            Happening::Told { said } => {
                if relay.show_for_a_word && !shown {
                    shown = link.show(true).await.is_ok();
                }
                // A send with no consumer is a dropped word — no interactive frontend —
                // and the run was not waiting for it anyway.
                let _ = prompt_tx
                    .send(SessionPrompt {
                        kind: PromptKind::Acknowledge,
                        detail: Some(said),
                        run: None,
                    })
                    .await;
            }
            Happening::Asking { hole } => {
                let (secret, name) = hole_of(&hole);
                if let Some(value) = name.and_then(|n| relay.answers.get(n)) {
                    let _ = link.answer(Some(value.to_string())).await;
                    continue;
                }
                let prompt = SessionPrompt {
                    kind: PromptKind::Text { secret },
                    detail: Some(hole),
                    run: Some(Arc::clone(&link)),
                };
                // Nobody listening drops the prompt, whose `Drop` refuses the run its
                // value — so the run ends with the question rather than hanging.
                let _ = prompt_tx.send(prompt).await;
            }
            Happening::Over { .. } => {
                if shown {
                    let _ = link.show(false).await;
                    shown = false;
                }
            }
            _ => {}
        }
    }
}

/// Take a hole apart: `{{secret.otp}}` is a secret named `otp`, `{{data.week}}` is not a
/// secret and is named `week`. Anything else is a secret (the safe default for a masked
/// field) with no name.
fn hole_of(hole: &str) -> (bool, Option<&str>) {
    let inner = hole
        .strip_prefix("{{")
        .and_then(|h| h.strip_suffix("}}"))
        .map(str::trim);
    match inner.and_then(|i| i.split_once('.')) {
        Some(("secret", name)) => (true, Some(name)),
        Some(("data", name)) => (false, Some(name)),
        _ => (true, None),
    }
}

/// Aborts a spawned task on drop.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hole_is_a_kind_and_a_name() {
        assert_eq!(hole_of("{{secret.otp}}"), (true, Some("otp")));
        assert_eq!(
            hole_of("{{secret.calendar-password}}"),
            (true, Some("calendar-password"))
        );
        assert_eq!(hole_of("{{data.week}}"), (false, Some("week")));
        assert_eq!(hole_of("otp"), (true, None));
    }

    #[test]
    fn only_the_browsers_first_run_is_announced() {
        let mut announcing = Announcing::default();
        assert_eq!(
            announcing.step(1, 3),
            Some(LoadStatus {
                fraction: Some(1.0 / 3.0),
                done: false
            })
        );
        assert_eq!(
            announcing.over(),
            Some(LoadStatus {
                fraction: Some(1.0),
                done: true
            })
        );
        // The second run is a fetch the consumer asked for: nothing to announce, and above
        // all no `done` for it to fetch again on.
        assert_eq!(announcing.step(1, 3), None);
        assert_eq!(announcing.over(), None);
    }

    #[test]
    fn nothing_is_said_before_a_step_is_done() {
        let announcing = Announcing::default();
        assert_eq!(announcing.step(0, 3), None);
        assert_eq!(announcing.step(1, 0), None);
    }

    #[test]
    fn a_browser_started_afresh_is_announced_again() {
        let mut announcing = Announcing::default();
        announcing.over();
        announcing.browser_gone();
        assert!(announcing.step(1, 3).is_some());
        assert!(announcing.over().is_some());
    }

    #[tokio::test]
    async fn a_toggle_before_the_first_fetch_finds_no_window_and_starts_no_browser() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = SessionConfig {
            account_key: "toggle-probe".into(),
            profile_dir: dir.path().join("profile"),
            headless: true,
            auto_headed: false,
            facts: BTreeMap::new(),
            answers: Answers::new(),
            browser: BrowserConfig {
                // Nothing to start: were a toggle to start the browser, this would fail
                // with a very different sentence.
                bin: dir.path().join("no-such-browser"),
                ..BrowserConfig::default()
            },
        };
        let session = SessionHandle {
            inner: SessionInner::spawn(config),
        };
        let err = session.toggle_window().await.expect_err("no window");
        assert!(matches!(err, MsOfficeError::Browser(why) if why == NO_BROWSER_YET));
    }

    #[test]
    fn answers_debug_names_no_value() {
        let mut answers = Answers::new();
        answers.insert("calendar-password", "hunter2");
        let shown = format!("{answers:?}");
        assert!(shown.contains("calendar-password"));
        assert!(!shown.contains("hunter2"));
    }

    // --- the forwarder, against a fake window ---

    use crate::browser::fake::{self, ask_of, news};
    use serde_json::{Value, json};
    use std::time::Duration;

    /// Wait until the window has heard `n` lines, and return them.
    async fn heard_by(window: &fake::Window, n: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let heard = window.heard.lock().unwrap().clone();
            if heard.len() >= n {
                return heard;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "heard only {heard:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A window that, on connect, says `lines` as news, and answers everything else politely.
    fn a_window_saying(lines: Vec<Value>) -> fake::Window {
        fake::window(move |request| {
            request
                .is_null()
                .then(|| lines.iter().cloned().map(news).collect())
        })
    }

    async fn forwarding(
        window: &fake::Window,
        relay: Relay,
    ) -> (AbortOnDrop, mpsc::Receiver<SessionPrompt>) {
        let link = Link::connect(&window.socket).await.expect("connect");
        let news = link.news();
        let (prompt_tx, prompt_rx) = mpsc::channel(4);
        let fwd = AbortOnDrop(tokio::spawn(forward(link, news, relay, prompt_tx)));
        (fwd, prompt_rx)
    }

    #[tokio::test]
    async fn a_question_the_session_can_answer_is_answered_unheard() {
        let window = a_window_saying(vec![
            json!({ "news": "asking", "hole": "{{secret.calendar-password}}" }),
        ]);
        let mut answers = Answers::new();
        answers.insert("calendar-password", "hunter2");
        let relay = Relay {
            show_for_a_word: true,
            answers,
        };
        let (_fwd, mut prompts) = forwarding(&window, relay).await;

        let heard = heard_by(&window, 1).await;
        assert_eq!(
            heard[0]["host"],
            json!({ "ask": "flow_answer", "value": "hunter2" })
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), prompts.recv())
                .await
                .is_err(),
            "nobody should hear of it"
        );
    }

    #[tokio::test]
    async fn a_word_from_the_run_is_a_prompt_and_the_window_comes_up_for_it() {
        let window = a_window_saying(vec![
            json!({ "news": "told", "said": "Tap 42 on your phone to sign in" }),
            json!({ "news": "over", "tally": { "ran": 1, "failed": 0, "broke": 0 }, "stopped": false, "yielded": {} }),
        ]);
        let relay = Relay {
            show_for_a_word: true,
            answers: Answers::new(),
        };
        let (_fwd, mut prompts) = forwarding(&window, relay).await;

        let prompt = tokio::time::timeout(Duration::from_secs(5), prompts.recv())
            .await
            .expect("in time")
            .expect("a prompt");
        assert_eq!(prompt.kind(), PromptKind::Acknowledge);
        assert_eq!(prompt.detail(), Some("Tap 42 on your phone to sign in"));
        prompt.acknowledge().await.expect("nothing travels");

        let heard = heard_by(&window, 2).await;
        let shows: Vec<_> = heard
            .iter()
            .filter(|r| ask_of(r) == Some("show"))
            .map(|r| r["host"]["on"].as_bool())
            .collect();
        assert_eq!(shows, vec![Some(true), Some(false)]);
    }

    #[tokio::test]
    async fn a_question_nobody_answers_is_a_text_prompt_that_refuses_when_let_go() {
        let window = a_window_saying(vec![json!({ "news": "asking", "hole": "{{secret.otp}}" })]);
        let relay = Relay {
            show_for_a_word: false,
            answers: Answers::new(),
        };
        let (_fwd, mut prompts) = forwarding(&window, relay).await;

        let prompt = tokio::time::timeout(Duration::from_secs(5), prompts.recv())
            .await
            .expect("in time")
            .expect("a prompt");
        assert_eq!(prompt.kind(), PromptKind::Text { secret: true });
        assert_eq!(prompt.detail(), Some("{{secret.otp}}"));
        drop(prompt);

        let heard = heard_by(&window, 1).await;
        assert_eq!(
            heard[0]["host"],
            json!({ "ask": "flow_answer", "value": null })
        );
    }
}
