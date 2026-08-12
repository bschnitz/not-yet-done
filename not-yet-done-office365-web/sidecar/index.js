#!/usr/bin/env node
// Office 365 web session sidecar.
//
// Protocol: one JSON object per line on stdin (requests) and stdout
// (responses + unsolicited events). stdout is PURE JSONL — all logging goes to
// stderr. The Rust side runs a background reader that demuxes each line by id.
//
//   → {"id":1,"op":"configure","params":{"username":"…","password":"…"}}
//   ← {"id":1,"ok":true,"result":{"ok":true}}
//   → {"id":2,"op":"ensureLogin","params":{"loginHint":"…"}}
//   ← {"id":2,"ok":true,"result":{"state":"loggedIn"}}
//   → {"id":3,"op":"getCalendarView","params":{"start":"…Z","end":"…Z"}}
//   ← {"id":3,"pending":true}          // heartbeat while blocked on sign-in
//   ← {"id":3,"ok":true,"result":{"events":[ … ]}}
//   ← {"id":*,"ok":false,"error":{"kind":"loginRequired","message":"…"}}
//   ← {"event":"calendarProgress","fraction":null}  // UNSOLICITED push (no id):
//                                      // the authenticated calendar surface just
//                                      // became available (login done); no % yet.
//   ← {"event":"calendarProgress","fraction":0.5}   // paging tick: window is 50%
//                                      // captured — banner-only, no refetch.
//   ← {"event":"calendarLoaded"}       // UNSOLICITED push (no id): the whole
//                                      // getCalendarView paging load finished.
//
// Session config arrives via env (set by the Rust launcher):
//   NYD_O365_PROFILE_DIR  persistent browser profile (login/SSO survives here)
//   NYD_O365_HEADLESS     "1" | "0"
//   NYD_O365_LOGIN_HINT   UPN to prefill (optional)
//   NYD_O365_START_URL    entry URL (optional; defaults to Outlook web calendar)
//
// Credentials do NOT arrive via env (the browser child inherits env → leak);
// the launcher pushes them once over the `configure` op above. With a password
// set, the sign-in runs unattended up to MFA (see login.js).
//
// How it reads the calendar (no hardcoded tenant/host — everything the tenant
// specific, MCAS-proxied endpoint needs is captured at runtime):
//   1. Drive a persistent browser to the Outlook web calendar.
//   2. Observe the app's OWN outgoing `/owa/service.svc?action=GetCalendarView`
//      request and capture its exact URL + headers (incl. the short-lived MSAL
//      bearer) + body — cookies alone are NOT enough for this data plane.
//   3. Replay that request with the body's date range rewritten to the
//      requested window, and parse the EWS JSON into MsCalEvent objects.
//   4. Fallback: if the request can't be captured, scrape the rendered DOM.

import readline from "node:readline";
import fs from "node:fs";

import { runLogin, SEL, LoginState } from "./login.js";

// Wall-clock ms timestamp on every line so latencies (e.g. how long the account
// picker sits before we click) are measurable straight from the log.
const log = (...a) =>
  console.error(`[o365-sidecar ${new Date().toISOString().slice(11, 23)}]`, ...a);

// Resting display mode. Headless by default: the browser stays invisible for
// every silent poll. When "0", the window is always visible (fully manual
// setups).
const HEADLESS = process.env.NYD_O365_HEADLESS !== "0";
// When resting headless, temporarily surface a visible window the moment the
// sign-in reaches an inherently interactive step (typically MFA), then drop
// back to headless once it completes. "0" disables the auto-switch: a headless
// session that hits an interactive step never pops a window — the second factor
// is surfaced through the event bus instead (number-match to the alert bar,
// approved on the phone; a one-time-code typed back via the bus reply). Only a
// non-promptable stop (bad/missing credentials) then reports loginRequired.
// Requires an event-bus consumer wired on the Rust/TUI side; without one the
// prompt is cancelled and a headless MFA would wait indefinitely.
const AUTO_HEADED = process.env.NYD_O365_AUTO_HEADED !== "0";
// Display mode for the FIRST launch. When the auto-switch is on we deliberately
// start VISIBLE and only drop to headless once the sign-in is fully complete
// (see restoreRestingMode): relaunching headless→headed *mid* sign-in proved
// unreliable (the window woke but couldn't be interacted with), so the whole
// login — auto-fill and MFA alike — happens in one stable visible window. A
// still-valid session confirms fast and drops to headless with only a brief
// flash. Only a headless-only setup (auto-switch disabled) starts invisible.
const INITIAL_HEADLESS = HEADLESS && !AUTO_HEADED;
const PROFILE_DIR = process.env.NYD_O365_PROFILE_DIR || "";
const LOGIN_HINT = process.env.NYD_O365_LOGIN_HINT || "";

// Resolved sign-in credentials. The Rust launcher pushes these once, up front,
// via the `configure` op (NOT the environment — a browser child inherits env,
// which would leak the password). Username defaults to the login hint so the
// account picker / email field is filled even with only a hint configured.
let _creds = { username: LOGIN_HINT, password: "" };
// Open the MONTH view: this tenant proxies the data-plane call through MCAS,
// which bakes the requested window into the (unreadable, streamed) request — so
// a replayed request always returns whatever window the visible page currently
// shows, and our own StartDate/EndDate is ignored. The month grid (~6 weeks)
// is the widest single natural view, so each captured month yields ~6 weeks.
const START_URL =
  process.env.NYD_O365_START_URL ||
  "https://outlook.office.com/calendar/view/month";

// Minimum number of months AHEAD of the current one to capture, as a FLOOR only.
// The real count is derived per request from the query's time span (see
// monthsAheadForRange / getCalendarView): a query reaching only into this month
// pages nothing, one reaching into next year pages that far. This env var just
// lets a test force extra paging regardless of the query; default 0 = the query
// alone drives how far we page. The window a replay returns is fixed to whatever
// month view was displayed when its request was minted, so each extra month means
// one more template, captured by navigating the page forward once (ensureTemplates).
const MONTHS_AHEAD_MIN = Math.max(
  0,
  parseInt(process.env.NYD_O365_MONTHS_AHEAD || "0", 10) || 0,
);

// Matches the calendar data-plane call regardless of the (MCAS-rewritten) host.
const CALENDAR_VIEW_RX = /\/owa\/service\.svc\?.*action=GetCalendarView/i;
// Hosts that mean "not signed in yet".
const LOGIN_HOST_RX =
  /(login\.microsoftonline|login\.live|\/common\/oauth2|sso)/i;

// --- browser session (lazy) -------------------------------------------------

let _context = null;
let _headed = false; // whether the live context runs headed (login in progress)
// The most recent authenticated GetCalendarView request the app issued, cached
// as a replay template ({url, headers, body}). Captured passively by a
// context-wide listener so we never have to reload the page to get a fresh
// bearer once the app has fetched at least once.
let _calTemplate = null;
// Auth truth: `true` only once a GetCalendarView RESPONSE came back HTTP 200,
// `false` after a 401/403. This is the SOLE signal that the calendar is actually
// readable — never the URL, never the mere presence of `_calTemplate`. A template
// is captured from the REQUEST, which the app ALSO issues with a stale/interim
// bearer mid-login and gets a 401 back; trusting the request alone reported
// "authed" before MFA and dropped to headless too early. Set by
// `installReadableWatch` (the page's own fetches) and by the replay path
// (getCalendarView) — so readability is re-checked on every poll, not just at
// first load. Everything that decides "authed" reads this.
let _calReadable = false;
// True once we observe the calendar being served through an MCAS reverse proxy
// (Microsoft Defender for Cloud Apps session control — host `*.mcas.ms`). Such a
// session lives in the LIVE browser context, not in the persistent profile's
// cookies, so it does NOT survive a context close+relaunch: dropping to headless
// destroys it and forces a fresh MFA (observed on an MCAS-proxied tenant, which routes
// 100% through `outlook.cloud.microsoft.mcas.ms`). When set, we never drop to
// headless — the window stays visible so the session stays alive. A direct tenant
// (e.g. `outlook.office.com` / `outlook.cloud.microsoft`, no `.mcas.ms`) persists
// in the profile and drops to headless fine, so this stays false for it.
let _sessionProxied = false;
// Monotonic counter bumped every time `_calTemplate` is (re)assigned to a fresh
// authenticated capture — by EITHER capture path (installRouteCapture and the
// passive installCapture listener). The per-month capture primitive
// (captureNextMonth) watches this instead of `page.waitForRequest`: this tenant
// issues GetCalendarView from a service worker, which a page-level
// `waitForRequest` does not observe, but the context-level route capture does.
// So watching the seq reliably detects "the nav click produced a new request",
// which manual paging demonstrably does in ~1s.
let _calCaptureSeq = 0;
// One replay template per captured month (index 0 = current month, 1 = next,
// …). Built once by ensureTemplates and reused for every poll so polls stay
// silent HTTP replays with no page navigation. Each entry is a {url, headers,
// body} snapshot whose MCAS token pins it to that month's window.
let _calTemplates = [];
// How many months ahead the cached _calTemplates were built to cover. A later
// request whose span needs MORE months than this rebuilds the set; a narrower or
// equal one reuses it. -1 = nothing built yet.
let _calTemplatesCoverage = -1;
// Last in-page hook diagnostic log seen while trying to recover a body — helps
// tell a service-worker-issued call (empty) from a page fetch we mis-parsed.
let _lastHookLog = [];
// While an interactive sign-in is in flight, suppress re-navigation until this
// deadline so a background poll can't yank the user out of the login form. The
// passive listener detects completion on its own (a template appears).
let _loginPendingUntil = 0;
// Resolvers waiting for the next authenticated data-plane capture. Fulfilled by
// `installCapture` the instant a template appears — an event-driven signal that
// the sign-in has completed, with no polling.
let _templateWaiters = [];

/// A promise that resolves once an authenticated GetCalendarView template has
/// been captured (immediately if one already has). Used to detect sign-in
/// completion without a timer.
function templateCaptured() {
  if (_calReadable) return Promise.resolve(true);
  return new Promise((resolve) => _templateWaiters.push(resolve));
}

async function launchContext(headless) {
  if (!PROFILE_DIR) {
    throw fail("config", "NYD_O365_PROFILE_DIR is not set");
  }
  const { chromium } = await import("playwright");
  _headed = !headless;
  const ctx = await chromium.launchPersistentContext(PROFILE_DIR, {
    headless,
    viewport: { width: 1280, height: 900 },
    // Surface service-worker network so we can observe the data-plane request.
    serviceWorkers: "allow",
  });
  await installBodyHook(ctx);
  installCapture(ctx);
  await installRouteCapture(ctx);
  installReadableWatch(ctx);
  return ctx;
}

/// Wake anyone waiting for the next authenticated data-plane capture.
function wakeTemplateWaiters() {
  // An authenticated GetCalendarView was just captured ⇒ the calendar surface
  // is available and a fetch would now succeed. Push the fraction-less nudge so
  // the adapter refreshes immediately (event-driven), instead of waiting for the
  // periodic poll (#17). The terminal `calendarLoaded` and the climbing progress
  // ticks come later, from the paging load in getCalendarView/ensureTemplates.
  emitLoadNudge();
  if (_templateWaiters.length) {
    const waiters = _templateWaiters;
    _templateWaiters = [];
    for (const resolve of waiters) resolve(true);
  }
}

// The load cycle emits three kinds of unsolicited push, all id-less (the Rust
// reader routes `event` lines to its event stream):
//
//   1. A fraction-less `calendarProgress {fraction:null}` NUDGE at the first
//      authenticated capture — the calendar surface just became available (a
//      login likely just completed out of band). It is a load boundary: the
//      adapter refetches on it (#17). One-shot per cycle (`_nudgeArmed`), because
//      captures fire many times per cycle (the app's own fetches, month-nav, and
//      our OWN replay POSTs) — emitting on every one fed a refetch→capture→nudge
//      loop.
//   2. Numeric `calendarProgress {fraction}` TICKS as `ensureTemplates` pages the
//      window in, month by month. Banner-only: the adapter moves the percentage
//      but does NOT refetch (that fetch would drive the very load that emits the
//      ticks). Emitted only while genuinely paging, so a silent (cached) poll
//      stays silent.
//   3. A terminal `calendarLoaded` once the paging load finishes (see
//      getCalendarView). It clears the banner and drives a final reconcile.
//      Gated on a genuine rebuild (not a flag): a cached poll never pages, so it
//      never re-emits — which is what stops the terminal→refetch→terminal loop.
//
// `armLoadSignals()` re-arms the one-shot nudge when a fresh (re)navigation or
// interactive sign-in begins, so a genuine reload after the browser bounced
// through sign-in nudges again.
let _nudgeArmed = true;
/// Send a `calendarProgress` push. `fraction` is a number in [0,1] for a
/// quantified paging tick, or `null` for the fraction-less "surface available"
/// boundary.
function emitCalendarProgress(fraction) {
  log(`→ emit calendarProgress fraction=${fraction ?? "null"}`);
  send({ event: "calendarProgress", fraction: fraction ?? null });
}
/// Emit the fraction-less load boundary ONCE per armed cycle.
function emitLoadNudge() {
  if (!_nudgeArmed) return;
  _nudgeArmed = false;
  emitCalendarProgress(null);
}
/// Emit the terminal `calendarLoaded`. Not one-shot on its own — the caller only
/// invokes it after a genuine paging rebuild, which a cached poll never does.
function emitCalendarLoaded() {
  log("→ emit calendarLoaded (terminal)");
  send({ event: "calendarLoaded" });
}
/// Re-arm the one-shot nudge so the next genuine load signals again. Called when
/// a (re)navigation or interactive sign-in starts (i.e. we left the loaded state).
function armLoadSignals() {
  _nudgeArmed = true;
}

// --- mid-operation prompts (adapter-initiated input) ------------------------
// A `promptRequest` push asks the Rust side — and, if a consumer is wired, the
// TUI — to display or collect something the sign-in needs mid-flight (chiefly
// MFA). It is correlated by `reqId`; the answer arrives as an id-less
// `provideInput` line, consumed in `rl.on("line")` before op dispatch. If no
// consumer is wired the Rust translator replies promptly with
// `{ cancelled: true }`, so the returned promise always resolves — a prompt can
// never hang the sign-in.
let _promptSeq = 0;
const _pendingPrompts = new Map(); // reqId -> resolve({ value, cancelled })
/// Emit a `promptRequest` and resolve when its `provideInput` reply arrives.
/// `kind`: "acknowledge" (display-only) | "text" (collect a value). `detail` is
/// read-only text to show (e.g. the number-match code). Resolves to
/// `{ value: string|null, cancelled: bool }`.
function requestPrompt({ kind, secret = false, detail = null }) {
  const reqId = `p${++_promptSeq}`;
  log(`→ emit promptRequest reqId=${reqId} kind=${kind}${detail ? " (+detail)" : ""}`);
  send({ event: "promptRequest", reqId, kind, secret, detail: detail ?? null });
  return new Promise((resolve) => _pendingPrompts.set(reqId, resolve));
}
/// Resolve a pending prompt from an incoming `provideInput` line. An unknown or
/// already-resolved `reqId` is ignored (a late or duplicate reply is harmless).
function resolvePrompt(msg) {
  const resolve = _pendingPrompts.get(msg.reqId);
  if (!resolve) return;
  _pendingPrompts.delete(msg.reqId);
  resolve({ value: msg.value ?? null, cancelled: !!msg.cancelled });
}

/// Intercept the GetCalendarView call via routing (which buffers the request
/// body) and pass it through untouched. Playwright's passive `request` event
/// reports 0 bytes for this service-worker-issued POST, but a routed request
/// exposes `postData()` — so this is our reliable path to the real body (with
/// the date window we need to rewrite). Transparent: we only read, then
/// `continue()`.
async function installRouteCapture(ctx) {
  await ctx.route(CALENDAR_VIEW_RX, async (route) => {
    try {
      const req = route.request();
      const buf = req.postDataBuffer();
      log(
        `route hit: ${req.method()} postData=${(req.postData() || "").length} buf=${buf ? buf.length : 0} url=${req.url().slice(0, 70)}`,
      );
      if (req.method() === "POST") {
        const headers = await req.allHeaders();
        if (headers.authorization) {
          const body = req.postData() ?? (buf ? buf.toString("utf8") : null);
          if (body) {
            _calTemplate = {
              url: req.url(),
              headers,
              body,
              bodyBytes: Buffer.byteLength(body, "utf8"),
              bodySource: "route",
              contentType: headers["content-type"] || "<none>",
            };
            // Cache the replay body + flag "a request fired" (month-nav detection)
            // ONLY. Do NOT signal authed here: a request carrying a bearer proves
            // nothing (the app fires one with a stale token mid-login and gets a
            // 401). Waking waiters happens on the 200 RESPONSE (installReadableWatch).
            _calCaptureSeq++;
          }
        }
      }
    } catch {
      /* never let capture break the request */
    }
    try {
      await route.continue();
    } catch {
      /* request may already be handled/aborted */
    }
  });
}

/// The MCAS session-control proxy submits the GetCalendarView body in a way that
/// Playwright's `request` event cannot read (it reports 0 bytes). Patch the
/// page's own `fetch`/XHR — before any app script runs — to stash the exact
/// request body the app sends into `window.__lastCalBody`, so `installCapture`
/// can recover it. Only the (date-window-only) GetCalendarView body is kept.
async function installBodyHook(ctx) {
  await ctx.addInitScript(() => {
    try {
      const RX = /\/owa\/service\.svc\?.*action=GetCalendarView/i;
      // Diagnostic ring buffer: lets the sidecar tell whether this page-level
      // hook ever observed the call at all (empty ⇒ it fires from a service
      // worker, which this script cannot reach).
      window.__calHookLog = window.__calHookLog || [];
      const note = (m) => {
        try {
          window.__calHookLog.push(m);
          if (window.__calHookLog.length > 40) window.__calHookLog.shift();
        } catch {}
      };
      const stash = (url, body) => {
        try {
          if (!RX.test(String(url || ""))) return;
          if (typeof body === "string") {
            window.__lastCalBody = body;
            note("str:" + body.length);
          } else if (body && typeof body.text === "function") {
            // Request/Blob body — read asynchronously into the stash.
            body
              .text()
              .then((t) => {
                if (t) {
                  window.__lastCalBody = t;
                  note("async:" + t.length);
                }
              })
              .catch(() => {});
            note("defer:" + (body.constructor && body.constructor.name));
          } else {
            note("skip:" + typeof body);
          }
        } catch {}
      };
      const of = window.fetch;
      if (of && !of.__nydWrapped) {
        const wrapped = function (input, init) {
          try {
            let url = "";
            let body;
            if (typeof input === "string") {
              url = input;
              body = init && init.body;
            } else if (input && input.url) {
              url = input.url;
              // Prefer an explicit init body; otherwise clone the Request so we
              // can read its (stream) body without consuming the original.
              body =
                init && init.body != null
                  ? init.body
                  : input.clone
                    ? input.clone()
                    : undefined;
            }
            if (RX.test(String(url))) note("fetch");
            stash(url, body);
          } catch {}
          return of.apply(this, arguments);
        };
        wrapped.__nydWrapped = true;
        window.fetch = wrapped;
      }
      const XO = window.XMLHttpRequest;
      if (XO && XO.prototype && !XO.prototype.__nydWrapped) {
        const open = XO.prototype.open;
        const send = XO.prototype.send;
        XO.prototype.open = function (method, url) {
          this.__nydUrl = url;
          return open.apply(this, arguments);
        };
        XO.prototype.send = function (body) {
          try {
            if (RX.test(String(this.__nydUrl || ""))) note("xhr");
            stash(this.__nydUrl, body);
          } catch {}
          return send.apply(this, arguments);
        };
        XO.prototype.__nydWrapped = true;
      }
    } catch {}
  });
}

/// Passively cache every authenticated GetCalendarView request the app makes,
/// so a replay template is available without forcing a page reload.
function installCapture(ctx) {
  ctx.on("request", async (req) => {
    try {
      if (req.method() !== "POST" || !CALENDAR_VIEW_RX.test(req.url())) return;
      const headers = await req.allHeaders();
      if (!headers.authorization) return; // wait for the authenticated one
      // Prefer the raw buffer: `postData()` returns null for bodies Playwright
      // cannot decode as UTF-8 text, whereas `postDataBuffer()` still carries
      // the bytes. `bodyBytes`/`contentType` are diagnostics so a missing body
      // (e.g. proxied away) is distinguishable from a truly empty one.
      const buf = req.postDataBuffer();
      let body = req.postData() ?? (buf ? buf.toString("utf8") : null);
      let bodyBytes = buf ? buf.length : 0;
      let bodySource = body ? "request" : "none";
      if (!body) {
        // Playwright saw 0 bytes (MCAS proxy). Recover the real body from the
        // in-page fetch/XHR hook (installBodyHook). The `bodySource` values
        // (no-page / hook-empty / page-hook) diagnose where the call lives.
        try {
          const page = req.frame()?.page();
          if (!page) {
            bodySource = "no-page";
          } else {
            const hooked = await page.evaluate(() => ({
              body: window.__lastCalBody || null,
              log: window.__calHookLog || [],
            }));
            if (hooked.body) {
              body = hooked.body;
              bodyBytes = Buffer.byteLength(body, "utf8");
              bodySource = "page-hook";
            } else {
              bodySource = "hook-empty";
              _lastHookLog = hooked.log;
            }
          }
        } catch {
          bodySource = "recover-err";
        }
      }
      // Don't clobber a route-captured template (which carries the real body)
      // with an empty-bodied one — the route handler and this listener race for
      // the same request. Only overwrite when we have a body, or nothing good
      // exists yet (so url/headers are still refreshed while unauthenticated).
      const hasGoodExisting = _calTemplate && _calTemplate.body;
      if (body || !hasGoodExisting) {
        _calTemplate = {
          url: req.url(),
          headers,
          body,
          bodyBytes,
          bodySource,
          contentType: headers["content-type"] || "<none>",
        };
        _calCaptureSeq++;
      }
      // Caching the replay body only — sign-in completion is signalled by the
      // matching 200 RESPONSE (installReadableWatch), not by this request.
    } catch {
      /* ignore malformed */
    }
  });
}

/// Auth truth watcher. A GetCalendarView *response* is the only thing that proves
/// the calendar is readable: HTTP 200 ⇒ readable (wake sign-in waiters + nudge the
/// adapter to refetch); 401/403 ⇒ the session lapsed, so mark it unreadable and
/// the next `ensureAuthed` drives a fresh (headed) login. Our own replay POSTs go
/// through `context.request` (APIRequestContext) and do NOT emit this event, so
/// this reflects only the browser page's own fetches; the replay path updates
/// `_calReadable` itself (see getCalendarView).
function installReadableWatch(ctx) {
  ctx.on("response", async (resp) => {
    try {
      const req = resp.request();
      if (req.method() !== "POST" || !CALENDAR_VIEW_RX.test(req.url())) return;
      const status = resp.status();
      if (status === 200) {
        // Note an MCAS-proxied session (its host is `*.mcas.ms`) BEFORE waking
        // waiters, so restoreRestingMode sees it and never drops to headless.
        if (!_sessionProxied && /\.mcas\.ms/i.test(req.url())) {
          _sessionProxied = true;
          log("calendar served via MCAS proxy (.mcas.ms) → will stay headed");
        }
        if (!_calReadable) log("calendar readable (GetCalendarView → 200)");
        _calReadable = true;
        wakeTemplateWaiters();
      } else if (status === 401 || status === 403) {
        if (_calReadable) log(`calendar no longer readable (→ ${status})`);
        _calReadable = false;
      }
    } catch {
      /* never let the watcher break anything */
    }
  });
}

async function context() {
  if (_context) return _context;
  _context = await launchContext(INITIAL_HEADLESS);
  return _context;
}

/// Is this URL an authenticated Outlook/OWA app surface (not a login page)?
function isAppUrl(url) {
  return (
    /outlook\.(office|office365)\.com|outlook-office|\/owa\/|\/calendar/i.test(
      url,
    ) && !LOGIN_HOST_RX.test(url)
  );
}

async function page() {
  const ctx = await context();
  const pages = ctx.pages();
  return pages.length ? pages[0] : await ctx.newPage();
}

/// How often to emit a `{"id":N,"pending":true}` heartbeat to the Rust side
/// while blocked on an interactive sign-in. Comfortably under the Rust
/// per-request idle timeout so the request never expires mid-login.
const HEARTBEAT_MS = 10000;

/// Wait — with NO time limit — for a handed-off interactive sign-in (typically
/// MFA) to complete, resolving `true` the instant the calendar is actually
/// readable (a GetCalendarView → 200, via `templateCaptured`). Event-driven, not
/// polled, and readability is the ONLY completion signal — a bare app URL is not
/// enough (OWA loads its shell before validating the session). It also
/// auto-advances any step that becomes automatable again after the manual one —
/// chiefly the "stay signed in?" (KMSI) prompt — by re-driving the login state
/// machine whenever the page navigates while still on a sign-in host.
async function waitForInteractiveLogin(p) {
  let driving = false;
  const drive = async () => {
    if (driving) return;
    driving = true;
    try {
      if (!isAppUrl(p.url()) && LOGIN_HOST_RX.test(p.url())) {
        await runLogin(p, _creds, { log });
      }
    } catch {
      // Navigation races during the flow are expected; the URL/template
      // signals below remain the source of truth for completion.
    } finally {
      driving = false;
    }
  };
  // Once automation stops at MFA, surface the second factor through a prompt so
  // the user can answer it in the TUI instead of the browser window. Two shapes:
  //   - number-match: display-only. Show the code; the user approves in their
  //     authenticator app and the browser advances on its own — so this is
  //     fire-and-forget and never gates completion (still `templateCaptured`).
  //   - one-time-code: collect the typed value and drive it into the field +
  //     submit, so the flow continues without touching the window.
  // Additive: with no consumer wired the Rust side cancels the prompt promptly,
  // leaving today's headed-window behaviour unchanged.
  let numberPrompted = false;
  let otcInFlight = false;
  const maybePromptMfa = async () => {
    try {
      const num = await visibleLocator(p, SEL.mfaNumber);
      if (num) {
        if (!numberPrompted) {
          numberPrompted = true;
          const code = (await num.innerText().catch(() => "")).trim();
          // Fire-and-forget: the answer only dismisses the overlay.
          requestPrompt({ kind: "acknowledge", detail: code || null });
        }
        return;
      }
      const otc = await visibleLocator(p, SEL.mfaOtc);
      if (otc && !otcInFlight) {
        otcInFlight = true;
        try {
          const { value, cancelled } = await requestPrompt({
            kind: "text",
            secret: true,
          });
          if (!cancelled && value) {
            await otc.fill(value).catch(() => {});
            await p
              .locator(`${SEL.mfaOtcSubmit}, ${SEL.submit}`)
              .first()
              .click()
              .catch(() => {});
          }
        } finally {
          // Let a fresh OTC field (e.g. a rejected code) prompt again.
          otcInFlight = false;
        }
      }
    } catch {
      // Prompting is best-effort; the headed window remains a valid fallback.
    }
  };
  const advance = async () => {
    await drive();
    await maybePromptMfa();
  };
  const onNav = (frame) => {
    if (frame === p.mainFrame()) advance();
  };
  p.on("framenavigated", onNav);
  try {
    // KMSI / MFA may already be on screen the moment we start waiting.
    advance();
    // Completion = the calendar actually became readable (a real 200), the ONLY
    // proof a sign-in finished. Reaching an app URL is NOT enough: OWA loads its
    // shell before the session is validated. `templateCaptured()` resolves the
    // instant `installReadableWatch` sees the 200. No time limit.
    return await templateCaptured();
  } finally {
    p.off("framenavigated", onNav);
  }
}

/// The first visible locator matching `selector`, or `null`. A light wrapper
/// over Playwright's `isVisible()` — enough for the unambiguous MFA markers
/// (unlike the opacity-toggled email/password inputs the classifier must
/// disambiguate in login.js).
async function visibleLocator(p, selector) {
  const loc = p.locator(selector).first();
  return (await loc.isVisible().catch(() => false)) ? loc : null;
}

/// Settle a fresh navigation as fast as the URL allows. OWA is a SPA that keeps
/// polling in the background, so it NEVER reaches `networkidle`; a plain
/// `waitForLoadState("networkidle")` therefore always burns its whole timeout on
/// an already-signed-in session — the "very slow to notice I'm already logged in"
/// case. Instead we resolve the instant the URL is decisive: an app surface
/// (authed) or the sign-in host (login needed). `waitForURL` returns immediately
/// when the current URL already matches, so both decisive outcomes are fast; a
/// short networkidle probe still covers a genuinely transitional page that is
/// neither yet.
async function settleAfterNav(p) {
  await Promise.race([
    p
      .waitForURL(
        (u) => isAppUrl(String(u)) || LOGIN_HOST_RX.test(String(u)),
        { timeout: 15000 },
      )
      .catch(() => {}),
    p.waitForLoadState("networkidle", { timeout: 15000 }).catch(() => {}),
  ]);
}

/// Authenticated ⟺ the calendar is actually readable — a GetCalendarView that
/// came back HTTP 200. Nothing else counts: NOT a bare app URL (OWA serves its
/// shell before validating the session and only bounces a stale one to the
/// sign-in host a moment later — MCAS / Conditional-Access, AADSTS16000), and NOT
/// a captured `_calTemplate` (the app fires a GetCalendarView with a stale bearer
/// mid-login that 401s, yet still populates the template). This waits up to
/// `graceMs` for a decisive signal — a real 200 (`_calReadable`) or a bounce to
/// the sign-in host — and returns true ONLY once the calendar is confirmed
/// readable. A genuinely signed-in session fetches and returns 200 within a second
/// or two, so the happy path stays fast.
async function appSurfaceIsAuthed(p, graceMs) {
  if (_calReadable) return true;
  await Promise.race([
    // A bounce to the sign-in host means "not signed in" — stop waiting early.
    p
      .waitForURL((u) => LOGIN_HOST_RX.test(String(u)), { timeout: graceMs })
      .catch(() => {}),
    (async () => {
      const deadline = Date.now() + graceMs;
      while (Date.now() < deadline) {
        if (_calReadable) return;
        await sleep(150);
      }
    })(),
  ]);
  return _calReadable;
}

/// Drop back to the resting (headless) display mode — but ONLY once the calendar
/// is confirmed readable (a real 200), and re-confirming a real 200 on the silent
/// relaunch before trusting it. The persistent profile normally carries the
/// session over, so the headless context re-lands on the authenticated calendar
/// and fetches it silently. No-op — returns the page unchanged — unless the
/// resting mode is headless, the auto-switch is on, and we are currently headed
/// (so on every poll after the first drop, and in a fully-headed setup, it does
/// nothing). If the silent relaunch does NOT reach a readable calendar (the
/// session did not carry over), it reverts to a headed window so the caller's
/// replay 401s and re-drives a fresh login (readability-as-auth).
async function restoreRestingMode(p) {
  if (!HEADLESS || !AUTO_HEADED || !_headed) return p;
  // Never drop before the calendar is genuinely readable. Guards against the
  // old bug: dropping to headless on a URL / captured-but-401'd template while
  // the user still had to enter their MFA token.
  if (!_calReadable) return p;
  // An MCAS-proxied session (host `*.mcas.ms`) does not survive a context
  // relaunch — closing this context to go headless destroys it and loops back to
  // MFA. Keep the working (visible) context alive instead; polls replay silently
  // on it via context.request regardless of headed/headless.
  if (_sessionProxied) {
    log("session is MCAS-proxied → staying headed (a relaunch would drop it)");
    return p;
  }
  log("calendar readable → dropping back to headless");
  try {
    await _context.close();
  } catch {}
  // Force a fresh capture + re-confirmation on the silent context: the headed
  // session's template and readable flag do not prove the headless relaunch
  // inherited a working session.
  _calReadable = false;
  _calTemplate = null;
  _calTemplates = [];
  _calTemplatesCoverage = -1;
  _context = await launchContext(true);
  let np = await page();
  await np
    .goto(START_URL, { waitUntil: "domcontentloaded", timeout: 60000 })
    .catch(() => {});
  await settleAfterNav(np);
  if (await appSurfaceIsAuthed(np, 12000)) {
    log("headless relaunch confirmed readable");
    return np;
  }
  // The silent relaunch never reached a readable calendar → the session did not
  // carry over (a proxied/non-persistent session we did not recognise by host).
  // Learn it so we stop trying to drop this connection, and go back to headed so
  // a fresh sign-in is possible; the caller's replay 401s and re-drives login.
  // Leaves `_calReadable` false on purpose.
  _sessionProxied = true;
  log("headless relaunch not readable → staying headed from now on; reverting to headed");
  try {
    await _context.close();
  } catch {}
  _context = await launchContext(false);
  np = await page();
  await np
    .goto(START_URL, { waitUntil: "domcontentloaded", timeout: 60000 })
    .catch(() => {});
  await settleAfterNav(np);
  return np;
}

/// Ensure we have an authenticated calendar page WITHOUT disrupting one that is
/// already there. Only navigates when the current page isn't an app surface —
/// so a background poll never yanks the user out of an in-progress login, and a
/// signed-in session is left untouched (the passive listener keeps the replay
/// template fresh on its own).
///
/// With the auto-switch on, the FIRST launch is already visible (INITIAL_HEADLESS
/// is false), so the whole sign-in runs in one stable window: the login state
/// machine (login.js) auto-fills with the configured credentials (pick the
/// account / fill the email, fill the password, confirm "stay signed in") and
/// stops only at a second factor (MFA). We then WAIT — with no time limit — for
/// the user to complete MFA and return the authenticated page, so the SAME fetch
/// succeeds no matter how long they take. `ctx.heartbeat` (when provided) keeps
/// the Rust request alive during that wait. Once auth is confirmed — whether the
/// session was already valid or the user just finished MFA — restoreRestingMode
/// drops the context back to headless. (A headless-only setup, auto-switch off,
/// starts invisible and reports `loginRequired` instead of popping a window.)
async function ensureAuthed(ctx = {}) {
  let p = await page();

  // The calendar is confirmed readable (a recent 200) → the session is live, so
  // never disturb it. If we are still in the initial visible window, drop to the
  // resting (headless) mode now that readability is confirmed.
  if (_calReadable) {
    _loginPendingUntil = 0;
    return await restoreRestingMode(p);
  }

  // On the app by URL. But a freshly reopened persistent context can restore a
  // STALE app URL from the previous session that MCAS/Conditional-Access then
  // redirects to the account picker for re-validation (AADSTS16000). Trusting the
  // URL immediately hands the picker back as if authed — no login is driven and
  // getCalendarView falls back to a DOM scrape while the browser sits on the
  // picker (observed on restart). So when we land on an app URL with no template
  // yet, give an in-flight bounce to the sign-in host a brief chance to land;
  // only trust the app surface if it stays put.
  if (isAppUrl(p.url())) {
    if (await appSurfaceIsAuthed(p, 4000)) {
      _loginPendingUntil = 0;
      return await restoreRestingMode(p);
    }
    log("reopened app URL not readable; driving login");
  }

  // A sign-in is already in flight: don't navigate (that would reset the login
  // form). Let the user finish; the passive listener will pick it up.
  if (_loginPendingUntil && Date.now() < _loginPendingUntil) {
    throw fail(
      "loginRequired",
      "sign-in in progress — complete it in the open window, then retry",
    );
  }

  // We are about to (re)navigate and sign in — we've left any loaded state, so
  // re-arm the one-shot load nudge for the fresh load that follows.
  armLoadSignals();

  // Navigate once and let redirects settle. Timed so the log shows how much of
  // any pre-click delay is the navigation/redirect chain vs. classification.
  const tNav = Date.now();
  await p
    .goto(START_URL, { waitUntil: "domcontentloaded", timeout: 60000 })
    .catch(() => {});
  log(`ensureAuthed: goto settled in ${Date.now() - tNav}ms → ${p.url().slice(0, 90)}`);
  const tSettle = Date.now();
  await settleAfterNav(p);
  log(`ensureAuthed: settleAfterNav ${Date.now() - tSettle}ms → ${p.url().slice(0, 90)}`);
  if (isAppUrl(p.url())) {
    const tConfirm = Date.now();
    const authed = await appSurfaceIsAuthed(p, 8000);
    log(
      `ensureAuthed: app-surface confirm ${Date.now() - tConfirm}ms → ${authed ? "authed" : "bounced, driving login"} (${p.url().slice(0, 90)})`,
    );
    if (authed) {
      _loginPendingUntil = 0;
      return await restoreRestingMode(p);
    }
  }

  // Not signed in: drive the credentialled sign-in as far as it will go.
  const result = await runLogin(p, _creds, { log });
  if (result.ok && (await appSurfaceIsAuthed(p, 8000))) {
    log("automated sign-in reached a readable calendar");
    _loginPendingUntil = 0;
    return await restoreRestingMode(p);
  }
  log(`automated sign-in stopped at "${result.state}": ${result.reason || ""}`);

  // Automation couldn't finish on its own — typically it stopped at MFA. How we
  // surface the second factor depends on the display mode:
  //
  //   - auto-switch ON (legacy, no event-bus consumer wired): pop a VISIBLE
  //     window and auto-fill again so the user faces only the second factor in
  //     the browser.
  //   - auto-switch OFF: the second factor is surfaced through the EVENT BUS
  //     (requestPrompt below → office365-web:mfa:* topics), so no window is
  //     needed — number-match is approved on the phone, a one-time-code is typed
  //     back into the field via the bus reply. We stay headless and simply fall
  //     through to the event-driven wait. Only a NON-promptable stop (missing /
  //     rejected credentials, unrecognised page) is hopeless headless, so bail
  //     for that rather than wait forever on an invisible page.
  if (!_headed) {
    if (AUTO_HEADED) {
      log("relaunching headed so the user can finish the sign-in");
      try {
        await _context.close();
      } catch {}
      _context = await launchContext(false);
      p = await page();
      await p
        .goto(START_URL, { waitUntil: "domcontentloaded", timeout: 60000 })
        .catch(() => {});
      await settleAfterNav(p);
      // Auto-fill again in the visible window (stops at the MFA prompt).
      const headedResult = await runLogin(p, _creds, { log });
      if (headedResult.ok && (await appSurfaceIsAuthed(p, 8000))) {
        _loginPendingUntil = 0;
        return await restoreRestingMode(p);
      }
    } else if (result.state !== LoginState.MFA) {
      _loginPendingUntil = Date.now() + 180000;
      throw fail(
        "loginRequired",
        "headless sign-in needed and the stop is not a promptable second factor " +
          "(check credentials) — sign in and retry",
      );
    } else {
      log(
        "headless MFA: surfacing the second factor through the event bus (no window)",
      );
    }
  }

  // Now the sign-in is stopped at an inherently interactive step (typically
  // MFA) — either in a visible window (auto-switch) or headless behind the event
  // bus. Rather than bail — which would force the user to re-issue the fetch
  // after finishing — WAIT for them to complete it, with no time limit, then
  // return the authenticated page so THIS fetch yields the calendar. The wait is
  // event-driven (a URL/template signal), and the periodic heartbeat keeps the
  // Rust request alive however long they take.
  log("waiting (no time limit) for the interactive sign-in to complete…");
  const stopHeartbeat = ctx.heartbeat
    ? setInterval(ctx.heartbeat, HEARTBEAT_MS)
    : null;
  let completed;
  try {
    completed = await waitForInteractiveLogin(p);
  } finally {
    if (stopHeartbeat) clearInterval(stopHeartbeat);
  }
  if (completed && _calReadable) {
    log("interactive sign-in completed (calendar readable); continuing");
    _loginPendingUntil = 0;
    return await restoreRestingMode(p);
  }

  // The wait ended without reaching the app — typically the window was closed.
  // Fall back to the grace window so a later retry doesn't yank a fresh login.
  _loginPendingUntil = Date.now() + 180000;
  throw fail(
    "loginRequired",
    "interactive login required — complete sign-in in the open window, then retry",
  );
}

// --- operation handlers -----------------------------------------------------

const OPS = {
  // Receive the resolved credentials from the Rust launcher. Sent once, before
  // any login is attempted. Only the fields present are updated; a null field
  // leaves the current value (username defaults to the login hint).
  async configure(params) {
    if (typeof params.username === "string" && params.username)
      _creds.username = params.username;
    if (typeof params.password === "string") _creds.password = params.password;
    log(
      `configured credentials (username: ${_creds.username ? "set" : "unset"}, password: ${_creds.password ? "set" : "unset"})`,
    );
    return { ok: true };
  },

  // Ensure the persistent context is authenticated. Opens a headed window for
  // MFA if needed. Returns {state:"loggedIn"} or {state:"interactiveLoginOpened"}.
  async ensureLogin(_params, ctx) {
    try {
      await ensureAuthed(ctx);
      return { state: "loggedIn" };
    } catch (e) {
      if (e.kind === "loginRequired")
        return { state: "interactiveLoginOpened" };
      throw e;
    }
  },

  // Return {events:[ MsCalEvent … ]} for the half-open [start,end) range.
  async getCalendarView(params, ctx) {
    const start = params.start;
    const end = params.end;
    if (!start || !end)
      throw fail("protocol", "getCalendarView needs start and end");

    // One fetch attempt: ensure the calendar is readable, then replay the paging
    // set for the window. A replay 401/403 (calendar no longer readable) throws
    // `loginRequired`, which the caller below catches to re-drive login.
    const attempt = async () => {
      // Surface a "loading" hint the MOMENT a genuine load begins — before the
      // (possibly long, interactive) sign-in — so the TUI shows the indeterminate
      // banner throughout the login, not only once the calendar surface appears.
      // A cached poll (already readable) stays silent. The terminal
      // calendarLoaded below always clears it (see `rebuilt || willLoad`).
      const willLoad = !_calReadable;
      if (willLoad) emitCalendarProgress(null);

      const p = await ensureAuthed(ctx);

      const monthsAhead = monthsAheadForRange(start, end);
      log(
        `getCalendarView range ${start} … ${end} → covering current + ${monthsAhead} month(s) ahead`,
      );
      const { templates, rebuilt } = await ensureTemplates(p, monthsAhead);
      let events;
      if (templates.length) {
        try {
          // Replay every month's template and merge, deduping by event id: the
          // month grids overlap (~6 weeks each), so an event near a boundary can
          // appear in two months. Last write wins — identical either way.
          const merged = new Map();
          for (let i = 0; i < templates.length; i++) {
            for (const e of await replayCalendarView(
              templates[i],
              start,
              end,
              `month+${i}`,
            )) {
              merged.set(e.id, e);
            }
          }
          events = [...merged.values()];
          // A successful replay IS a successful calendar read → affirm
          // readability. During silent polls the page never fetches on its own,
          // so this replay is the only fresh readability signal per cycle.
          _calReadable = true;
          log(
            `capture+replay across ${templates.length} month(s) returned ${events.length} events`,
          );
        } catch (e) {
          // A 401/403 on replay means the calendar is genuinely unreadable — the
          // session lapsed. Mark it and bubble up so the outer handler re-drives a
          // (headed) login; this is what makes a lapsed token during POLLING flip
          // back to headed, not just a first-load failure. Any OTHER replay error
          // (parse, transient) still falls back to the DOM scrape below.
          if (e.kind === "loginRequired") {
            _calReadable = false;
            throw e;
          }
          log("replay failed, falling back to DOM scrape:", e.message);
        }
      } else {
        log(
          `no data-plane template captured yet, falling back to DOM scrape (url=${p.url().slice(0, 90)})`,
        );
      }

      if (events === undefined) {
        events = await scrapeDom(p);
        log(`DOM scrape returned ${events.length} events (current view only)`);
      }

      // Clear the banner + do a final reconcile via the terminal `calendarLoaded`.
      // Fire it when the paging set was (re)built OR when we showed the loading
      // hint for this call (`willLoad`) — the latter guarantees a banner we raised
      // is always taken down, even if this load reused a cached template set
      // (rebuilt:false) after a re-auth. A cached poll (no willLoad, no rebuild)
      // stays silent, which is what stops the terminal→refetch loop.
      if (rebuilt || willLoad) emitCalendarLoaded();
      return events;
    };

    try {
      return { events: await attempt() };
    } catch (e) {
      if (e.kind !== "loginRequired") throw e;
      // The calendar was unreadable — drop the stale replay set and re-drive a
      // fresh (headed) login, then retry the fetch ONCE. `ensureAuthed` sees
      // `_calReadable === false` and, if resting headless, relaunches headed and
      // waits for the interactive step. A second failure propagates (Rust maps it
      // to `interactiveLoginOpened`) rather than looping.
      log("calendar unreadable → clearing template + re-driving login");
      _calReadable = false;
      _calTemplate = null;
      _calTemplates = [];
      _calTemplatesCoverage = -1;
      return { events: await attempt() };
    }
  },
};

// --- capture + replay -------------------------------------------------------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/// How long to wait for the data-plane request a month-nav click SHOULD trigger.
/// A genuine month nav fires GetCalendarView within ~1-2 s; if nothing arrives by
/// this bound the tenant is serving the adjacent month straight from the already
/// loaded ~6-week grid and no request will ever come, so we stop waiting. Kept
/// short because this wait sits in the post-login fetch path — a long timeout is
/// felt directly as a delay between "calendar visible" and "events in the TUI".
const NAV_CAPTURE_TIMEOUT_MS = 6000;

/// Return a replay template. The passive listener (installCapture) fills it as
/// soon as the app fetches naturally; if none is cached yet, provoke ONE reload
/// and wait briefly. Once cached, later calls reuse it with no page reload — so
/// polls become silent HTTP replays that never disturb the visible page.
async function ensureTemplate(p) {
  if (_calTemplate) return _calTemplate;
  await p
    .reload({ waitUntil: "domcontentloaded", timeout: 60000 })
    .catch(() => {});
  // Date.now() is fine here — this is Node, not the Rust workflow harness.
  const deadline = Date.now() + 20000;
  while (!_calTemplate && Date.now() < deadline) await sleep(250);
  return _calTemplate;
}

/// Return one replay template per month to cover [current … current+monthsAhead].
/// The current month is captured passively (ensureTemplate). Each following month
/// needs its OWN request (its MCAS token pins the window), so we page the visible
/// calendar forward one month at a time, snapshotting the request that fires.
///
/// CRUCIAL: paging is done via the in-app toolbar arrows (clickCalendarNav), which
/// is client-side SPA navigation — NOT a full page load. A full navigation of the
/// visible, logged-in page (a `goto`/`reload`) re-enters the MCAS/Conditional-Access
/// flow and bounces the page to the account picker (AADSTS16000 SelectUserAccount,
/// #15). So we never `goto` here: we page forward to capture, then page BACK the
/// same number of steps to restore the current month. Built once and cached:
/// subsequent polls reuse the snapshots with no navigation at all.
async function ensureTemplates(p, monthsAhead) {
  const want = Math.max(monthsAhead || 0, MONTHS_AHEAD_MIN);
  // Reuse the cache only when it already covers at least as many months as this
  // request needs. A wider query rebuilds; a narrower/equal one is a silent
  // replay: `rebuilt:false` keeps the poll silent (no progress, no terminal).
  if (_calTemplates.length && _calTemplatesCoverage >= want)
    return { templates: _calTemplates, rebuilt: false };
  const first = await ensureTemplate(p);
  if (!first) return { templates: [], rebuilt: false };
  // Snapshot: the passive listener keeps mutating _calTemplate as the page moves.
  const templates = [{ ...first }];
  // Progress counts ALL months in the window — the current one (captured just
  // above, index 0) PLUS each of the `want` ahead months — so the denominator is
  // want+1, not want. Seed the current month's share now; each ahead month bumps
  // it, the last reaching 1.0 just before the terminal `calendarLoaded` clears
  // the banner. E.g. a July→September window (want 2) climbs 33 % → 66 % → 100 %.
  const totalMonths = want + 1;
  if (want > 0) emitCalendarProgress(1 / totalMonths);
  let stepped = 0; // "next" clicks that ACTUALLY advanced the view (drives restore)
  let clickFailed = false; // a nav CLICK threw (button missing) → likely transient
  const periodStart = await readVisiblePeriod(p); // to restore precisely later
  for (let m = 1; m <= want; m++) {
    let res;
    try {
      res = await captureNextMonth(p);
    } catch (e) {
      log(
        `month+${m} nav click failed (${e.message}); covering ${templates.length} month(s)`,
      );
      clickFailed = true;
      break;
    }
    // The click landed on SOMETHING but the visible period did NOT change → our
    // selectors matched a non-pager element on this OWA build. Paging is how the
    // multi-month (and multi-year, for such tenants) coverage is actually gathered — each
    // month needs its own captured request — so a stuck pager is a real problem,
    // NOT something to paper over. Fail fast: stop rather than keep clicking a
    // wrong element (which would drift the visible calendar) and log loudly so
    // the selector gets fixed for this build. Once the pager is matched this
    // never triggers.
    if (res.moved === false) {
      log(
        `month+${m}: toolbar "next" click did not advance the period — pager selector not matched on this OWA build; stopping at ${templates.length} month(s) (FIX clickCalendarNav for this build)`,
      );
      break;
    }
    stepped++;
    if (res.template) {
      templates.push(res.template);
      log(
        `captured template for month+${m} (${res.template.bodySource}, ${res.template.bodyBytes}B)`,
      );
    } else {
      log(
        `month+${m}: nav fired no NEW data-plane capture within ${NAV_CAPTURE_TIMEOUT_MS}ms (current grid likely already covers it); covering ${templates.length} month(s)`,
      );
    }
    // Bump the climbing fraction (banner-only): current month + m ahead months
    // captured out of totalMonths. Reaches 1.0 on the last ahead month; the
    // terminal `calendarLoaded` then clears the banner.
    emitCalendarProgress((m + 1) / totalMonths);
  }
  // Restore the visible page to the month we started on by paging BACK in-app —
  // never a `goto` (that bounces the logged-in page to the account picker, #15).
  // Page back at most as many steps as we actually advanced (never overshoot),
  // and stop early the instant the period label returns to where it began, so a
  // tenant whose back-arrow we can't match doesn't spin on a doomed click.
  for (let i = 0; i < stepped; i++) {
    if (periodStart && (await readVisiblePeriod(p)) === periodStart) break;
    try {
      await clickCalendarNav(p, "prev");
      await sleep(400);
    } catch (e) {
      log(`month restore step ${i + 1} failed (${e.message})`);
      break;
    }
  }
  // Cache the best-effort set once the forward sweep completed without a hard CLICK
  // failure. A tenant whose in-app nav serves months from cache will NEVER fire the
  // extra requests, so gating the cache on a COMPLETE set would retry every poll —
  // paging the visible calendar further forward each time and stalling 20 s. The
  // current-month template already spans ~6 weeks, so coverage stays close to the
  // requested window regardless. A missing button (clickFailed) is treated as
  // transient and retried next poll instead.
  if (!clickFailed) {
    _calTemplates = templates;
    _calTemplatesCoverage = want;
  }
  return { templates, rebuilt: true };
}

/// Derive how many months AHEAD of the current one a query span needs. The
/// current-month template is captured passively and already spans ~6 weeks; each
/// further calendar month the range reaches into needs its own template. Measured
/// from today's month to the month containing `end` (half-open, so one tick before
/// end), floored at 0. No upper bound — a range into next year pages that far.
/// Generous by design: over-capturing a boundary month costs one silent replay,
/// under-capturing drops events.
function monthsAheadForRange(start, end) {
  // Date.now()/new Date() are fine here — this is Node, not the Rust harness.
  const now = new Date();
  const endInclusive = new Date(new Date(end).getTime() - 1);
  if (isNaN(endInclusive.getTime())) return 0;
  const months =
    (endInclusive.getFullYear() - now.getFullYear()) * 12 +
    (endInclusive.getMonth() - now.getMonth());
  return Math.max(0, months);
}

/// Page the visible calendar forward one month and capture the request the app
/// fires for it. Watches `_calCaptureSeq` (bumped by BOTH capture paths) rather
/// than `page.waitForRequest`: this tenant issues GetCalendarView from a service
/// worker, invisible to a page-level request wait but seen by the context-level
/// route capture — and the route capture carries the real body, so a month
/// captured this way is a full replay template, not a synthesised one.
///
/// Returns `{ template, moved }`:
///   template — a snapshot of `_calTemplate` if the click fired a NEW capture,
///     else `null` (the current ~6-week grid already served the adjacent month).
///   moved — `true` if the visible period label advanced, `false` if it stayed
///     put (the click hit a non-pager element — this build's toolbar is not
///     matched), or `null` if the label was unreadable (assume it moved).
/// Throws only if the nav CLICK itself fails (button missing) — a transient the
/// caller handles.
async function captureNextMonth(p) {
  const seqBefore = _calCaptureSeq;
  const periodBefore = await readVisiblePeriod(p);
  await clickCalendarNav(p, "next"); // throws on a missing/!clickable button
  const deadline = Date.now() + NAV_CAPTURE_TIMEOUT_MS;
  while (_calCaptureSeq === seqBefore && Date.now() < deadline) await sleep(100);
  const periodAfter = await readVisiblePeriod(p);
  const fired = _calCaptureSeq !== seqBefore;
  // Did the view advance? A readable label that changed → yes; a readable label
  // that stayed identical → no (the click did not hit the working month pager);
  // an unreadable label → unknown (treat as moved, preserving old behaviour for
  // tenants whose period label we can't scrape).
  const moved =
    periodBefore && periodAfter ? periodBefore !== periodAfter : null;
  // Diagnostic that disambiguates the two ways a capture can be absent:
  //   period advanced but NO request → the ~6-week grid already served the month
  //   period UNCHANGED               → the nav click did not advance the view
  const movedLabel = moved === null ? "period?" : moved ? "advanced" : "UNCHANGED";
  log(
    `nav next: "${periodBefore}" → "${periodAfter}" (${movedLabel}); capture ${fired ? "fired" : "NONE"}`,
  );
  return { template: fired ? { ..._calTemplate } : null, moved };
}

/// Best-effort read of the calendar's visible period label ("July 2026" etc.),
/// used only for the captureNextMonth diagnostic. OWA localises and reshuffles
/// this label, so we scan likely heading/button nodes for the first short piece
/// of text carrying a 4-digit year. Purely observational — never throws.
async function readVisiblePeriod(p) {
  try {
    return await p.evaluate(() => {
      const nodes = document.querySelectorAll(
        '[role="heading"], h1, h2, button, [aria-label]',
      );
      for (const el of nodes) {
        const t = (el.textContent || el.getAttribute("aria-label") || "").trim();
        if (t.length > 0 && t.length < 40 && /\b(19|20)\d\d\b/.test(t)) return t;
      }
      return null;
    });
  } catch {
    return null;
  }
}

/// Click the calendar toolbar's period-pager arrow. OWA ships at least two
/// toolbar builds and the pager arrow is hooked differently in each, so we try
/// several build-stable hooks in order and click the first that resolves:
///
///  1. Accessible NAME (`getByRole` name) — older OWA labels the arrow
///     "Next"/"Weiter" etc. via aria-label/text.
///  2. A localized phrase in the `tooltip` / `aria-label` / `title` ATTRIBUTE —
///     the Fluent UI v9 build (e.g. one such tenant) gives the button NO
///     accessible name at all: its icon is an aria-hidden icon FONT (no
///     `data-icon-name`), and the only human hook is
///     `tooltip="Zum nächsten Monat … wechseln"`.
///  3. The Fluent chevron ICON name (`data-icon-name="ChevronRight"`) — older
///     Fluent icon builds.
///
/// Throws if none is clickable — the caller treats that as transient.
///
/// NB: no bare "vor" in the next keywords — it also matches "vorher"/"vorherige"
/// (= previous), which would let the next selector grab the PREVIOUS button. Use
/// the unambiguous compound "vorwärts" instead.
async function clickCalendarNav(p, direction) {
  const nameRx =
    direction === "next"
      ? /next|forward|nächst|weiter|vorwärts/i
      : /previous|back|vorher|zurück|frühere/i;
  // Direction keywords likely to appear in a localized tooltip/label. Kept
  // specific (no generic "back"/"prev") so an attribute match can't grab an
  // unrelated button elsewhere on the surface.
  const kws =
    direction === "next"
      ? ["next", "nächst", "weiter", "vorwärts"]
      : ["previous", "vorher", "zurück", "frühere"];
  const attrSel = ["tooltip", "aria-label", "title"]
    .flatMap((a) =>
      kws.map(
        (k) => `button[${a}*="${k}" i], [role="button"][${a}*="${k}" i]`,
      ),
    )
    .join(", ");
  const iconSel =
    direction === "next"
      ? '[data-icon-name*="ChevronRight" i], [data-icon-name*="Forward" i]'
      : '[data-icon-name*="ChevronLeft" i], [data-icon-name*="Back" i]';
  const candidate = p
    .getByRole("button", { name: nameRx })
    .or(p.locator(attrSel))
    .or(p.locator(`button:has(${iconSel}), [role="button"]:has(${iconSel})`))
    .first();
  await candidate.click({ timeout: 8000 });
}

/// Replay a captured request with its date range rewritten to [start, end).
async function replayCalendarView(captured, start, end, tag = "") {
  const ctx = await context();
  const headers = sanitizeHeaders(captured.headers);

  // POST the given body once and parse the response. Returns {events, rc}
  // where `rc` is the server's ResponseClass/ResponseCode (for diagnostics).
  const doPost = async (data, label) => {
    const resp = await ctx.request.post(captured.url, {
      headers,
      data,
      timeout: 60000,
    });
    if (!resp.ok()) {
      if (resp.status() === 401 || resp.status() === 403) {
        throw fail(
          "loginRequired",
          `data plane returned ${resp.status()} on replay`,
        );
      }
      throw fail("opError", `replay HTTP ${resp.status()}`);
    }
    const json = await resp.json();
    const rc =
      json && json.Body
        ? `${json.Body.ResponseClass}/${json.Body.ResponseCode}`
        : "?";
    const events = normalizeEvents(collectEvents(json));
    log(`replay [${label}] → ${rc}, ${events.length} events`);
    if (events.length) {
      const starts = events.map((e) => e.start).sort();
      log(`replay events span: ${starts[0]} … ${starts[starts.length - 1]}`);
    }
    return { events, rc };
  };

  // With a readable body (non-MCAS tenants) rewrite its window; otherwise
  // synthesise a GetCalendarView request for the window.
  let body;
  let label;
  const pfx = tag ? `${tag} ` : "";
  if (captured.body) {
    const rw = rewriteRange(captured.body, start, end);
    body = rw.body;
    label = `${pfx}rewritten:${rw.rewritten.join(",") || "none"}`;
  } else {
    body = buildCalendarViewBody(start, end);
    label = `${pfx}constructed`;
  }

  let { events, rc } = await doPost(body, label);

  // If our synthesised body was rejected (unexpected shape / older build),
  // fall back to the verbatim empty-bodied replay so we never regress below
  // the server's default (current week).
  if (label.endsWith("constructed") && !/Success/i.test(rc)) {
    log("constructed body not accepted — falling back to verbatim replay");
    ({ events } = await doPost(captured.body || "", "verbatim-fallback"));
  }
  return events;
}

/// Drop headers the HTTP client must own itself; keep authorization, content
/// type, and the OWA/action headers verbatim.
function sanitizeHeaders(h) {
  const out = {};
  const drop = new Set([
    "host",
    "content-length",
    "connection",
    "accept-encoding",
    "cookie", // context.request reuses the browser cookie jar itself
  ]);
  for (const [k, v] of Object.entries(h)) {
    // HTTP/2 pseudo-headers (":authority", ":method", ":path", ":scheme") are
    // not valid header names for the HTTP client — drop them.
    if (k.startsWith(":")) continue;
    if (drop.has(k.toLowerCase())) continue;
    out[k] = v;
  }
  return out;
}

/// True for an ISO-8601-ish datetime string (`2026-07-06T…`). Used to tell a
/// query-window field from a plain string, regardless of its key name.
function looksLikeDate(v) {
  return typeof v === "string" && /^\d{4}-\d{2}-\d{2}T/.test(v);
}

/// Rewrite the captured request's query window to `[start, end)`.
///
/// OWA names the window fields differently across builds (`StartDate`/`EndDate`,
/// `StartTime`/`EndTime`, `ViewStart`/`ViewEnd`, …). Rather than enumerate every
/// name, we rewrite ANY datetime-valued field whose key contains `start`/`end`.
/// The GetCalendarView *request* body carries only the window and folder ids —
/// no event objects — so a key-substring match cannot clobber event data.
///
/// Returns `{ body, rewritten }` where `rewritten` lists the keys touched (for
/// diagnostics — an empty list means the window was NOT widened).
function rewriteRange(body, start, end) {
  if (!body) return { body, rewritten: [] };
  let obj;
  try {
    obj = JSON.parse(body);
  } catch {
    return { body, rewritten: [] }; // not JSON — replay verbatim (best effort)
  }
  const rewritten = [];
  const setDates = (node) => {
    if (Array.isArray(node)) {
      node.forEach(setDates);
    } else if (node && typeof node === "object") {
      for (const k of Object.keys(node)) {
        const v = node[k];
        if (looksLikeDate(v) && /start/i.test(k)) {
          node[k] = start;
          rewritten.push(k);
        } else if (looksLikeDate(v) && /end/i.test(k)) {
          node[k] = end;
          rewritten.push(k);
        } else {
          setDates(v);
        }
      }
    }
  };
  setDates(obj);
  return { body: JSON.stringify(obj), rewritten };
}

/// Build an OWA `GetCalendarView` request body for the window `[start, end)`.
///
/// The captured request's real body is unreadable here — this tenant routes the
/// data-plane call through the MCAS session-control proxy, which streams the
/// body so Playwright's request event, a routed request, and even the browser's
/// own DevTools all report 0 bytes. Since there is nothing to rewrite, we
/// synthesise the standard EWS-JSON request instead; the server honours our
/// window and returns the exact same response shape the app receives. Dates are
/// sent as UTC (`…Z`) with a UTC TimeZoneContext so the window is unambiguous —
/// event display offsets in the response are unaffected.
function buildCalendarViewBody(start, end) {
  const iso = (s) => new Date(s).toISOString(); // 2026-07-02T10:53:33.320Z
  return JSON.stringify({
    __type: "GetCalendarViewJsonRequest:#Exchange",
    Header: {
      __type: "JsonRequestHeaders:#Exchange",
      RequestServerVersion: "V2018_01_08",
      TimeZoneContext: {
        __type: "TimeZoneContext:#Exchange",
        TimeZoneDefinition: {
          __type: "TimeZoneDefinitionType:#Exchange",
          Id: "UTC",
        },
      },
    },
    Body: {
      __type: "GetCalendarViewRequest:#Exchange",
      StartDate: iso(start),
      EndDate: iso(end),
      FolderId: {
        __type: "TargetFolderId:#Exchange",
        BaseFolderId: {
          __type: "DistinguishedFolderId:#Exchange",
          Id: "calendar",
        },
      },
    },
  });
}

/// Diagnostic: `path = value` for every datetime-valued field in the captured
/// body, so the real window field names are visible in the log.
function dateFieldPaths(body) {
  let obj;
  try {
    obj = JSON.parse(body);
  } catch {
    return ["<non-JSON body>"];
  }
  const out = [];
  const walk = (node, path) => {
    if (Array.isArray(node)) {
      node.forEach((v, i) => walk(v, `${path}[${i}]`));
    } else if (node && typeof node === "object") {
      for (const [k, v] of Object.entries(node)) {
        if (looksLikeDate(v)) out.push(`${path}.${k} = ${v}`);
        else walk(v, `${path}.${k}`);
      }
    }
  };
  walk(obj, "");
  return out;
}

// --- response parsing -------------------------------------------------------

/// Walk the EWS/OWA JSON and collect every object that looks like a calendar
/// item (has string Start and End). Robust to envelope-shape differences.
function collectEvents(json) {
  const found = [];
  const seen = new Set();
  const walk = (node) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
    } else if (node && typeof node === "object") {
      if (typeof node.Start === "string" && typeof node.End === "string") {
        found.push(node);
      }
      for (const v of Object.values(node)) walk(v);
    }
  };
  walk(json);
  // De-dupe by item id (fallback: start+subject).
  return found.filter((e) => {
    const key = itemId(e) || `${e.Start}|${e.Subject || ""}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function itemId(e) {
  return (
    (e.ItemId && (e.ItemId.Id || e.ItemId.id)) ||
    e.Id ||
    e.id ||
    (e.ItemClass && e.Subject && `${e.Subject}|${e.Start}`) ||
    null
  );
}

const FREEBUSY_MAP = {
  Free: "free",
  Tentative: "tentative",
  Busy: "busy",
  OOF: "oof",
  WorkingElsewhere: "workingElsewhere",
};

/// Map raw OWA calendar items onto the camelCase MsCalEvent JSON the Rust DTO
/// deserialises. Timestamps are normalised to RFC3339 UTC.
function normalizeEvents(items) {
  return items.map((e) => {
    const fb = e.LegacyFreeBusyStatus || e.FreeBusyType || "";
    return {
      id: String(itemId(e) || `${e.Subject || ""}|${e.Start}`),
      subject: e.Subject || null,
      start: toUtc(e.Start),
      end: toUtc(e.End),
      isAllDay: !!(e.IsAllDayEvent || e.AllDayEvent),
      showAs: FREEBUSY_MAP[fb] || "unknown",
      location:
        (e.Location && (e.Location.DisplayName || e.Location.Name)) ||
        (typeof e.Location === "string" ? e.Location : null) ||
        null,
      organizer:
        (e.Organizer &&
          e.Organizer.Mailbox &&
          (e.Organizer.Mailbox.Name || e.Organizer.Mailbox.EmailAddress)) ||
        null,
      bodyPreview: e.Preview || e.TextBody || null,
      webLink: e.WebLink || null,
    };
  });
}

/// Normalise any parseable timestamp to RFC3339 UTC (`…Z`). chrono's
/// DateTime<Utc> deserialiser requires an explicit offset.
function toUtc(raw) {
  const d = new Date(raw);
  if (isNaN(d.getTime())) return raw; // let the Rust side report a bad value
  return d.toISOString();
}

// --- DOM fallback -----------------------------------------------------------

/// Best-effort scrape of the currently rendered calendar. Only covers what the
/// view shows (not the full requested range) — a diagnostic fallback, not a
/// substitute for the data-plane path.
async function scrapeDom(p) {
  await p.waitForLoadState("networkidle", { timeout: 15000 }).catch(() => {});
  const raw = await p
    .$$eval(
      '[role="button"][aria-label], [role="gridcell"] [aria-label]',
      (nodes) =>
        nodes
          .map((n) => n.getAttribute("aria-label"))
          .filter((l) => l && l.length > 4),
    )
    .catch(() => []);
  // aria-labels are locale/format dependent; we can only surface the subject
  // text reliably. Emit minimal events so the caller sees *something* and can
  // tell the data-plane path failed. Times are left at the epoch sentinel so
  // the failure is obvious rather than silently wrong.
  const uniq = [...new Set(raw)];
  return uniq.map((label, i) => ({
    id: `dom-${i}`,
    subject: label,
    start: "1970-01-01T00:00:00Z",
    end: "1970-01-01T00:00:00Z",
    isAllDay: false,
    showAs: "unknown",
    location: null,
    organizer: null,
    bodyPreview: null,
    webLink: null,
  }));
}

// --- transport --------------------------------------------------------------

/// Build an error carrying a protocol `kind` the Rust side understands
/// (`loginRequired` → CalendarError::Auth; others → generic).
function fail(kind, message) {
  const e = new Error(message);
  e.kind = kind;
  return e;
}

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

async function handle(req) {
  const op = OPS[req.op];
  if (!op) {
    return send({
      id: req.id,
      ok: false,
      error: { kind: "unknownOp", message: req.op },
    });
  }
  try {
    // Give the op a heartbeat channel so a long, user-paced step (interactive
    // sign-in) can keep this request alive past the Rust idle timeout without a
    // wall-clock limit. Heartbeats carry the request id and `pending:true`.
    const ctx = {
      id: req.id,
      heartbeat: () => send({ id: req.id, pending: true }),
    };
    const result = await op(req.params || {}, ctx);
    send({ id: req.id, ok: true, result });
  } catch (e) {
    send({
      id: req.id,
      ok: false,
      error: {
        kind: e.kind || "opError",
        message: String(e && e.message ? e.message : e),
      },
    });
  }
}

// --- lifecycle: never let the browser outlive the parent app ----------------

let _shuttingDown = false;
/// Close the browser once and exit. Idempotent: safe to call from any of the
/// teardown triggers (stdin EOF, SIGTERM/SIGINT from the group teardown, or the
/// parent-death pipe) — only the first call does the work.
async function shutdown(code) {
  if (_shuttingDown) return;
  _shuttingDown = true;
  try {
    if (_context) await _context.close();
  } catch {}
  process.exit(code);
}

// The Rust launcher tears the whole process group down with SIGTERM on a clean
// stop; handle it so the persistent context is closed properly (which removes
// the profile's SingletonLock) instead of being hard-killed.
process.on("SIGTERM", () => shutdown(0));
process.on("SIGINT", () => shutdown(0));

// Parent-death pipe: the launcher holds the ONLY write end. If the app dies for
// ANY reason — clean exit, panic, or SIGKILL — the OS closes that end and we
// see EOF here, even when no signal was delivered. This is the hard guarantee
// that the browser can never be left orphaned.
const PARENT_PIPE_FD = process.env.NYD_O365_PARENT_PIPE_FD;
if (PARENT_PIPE_FD) {
  try {
    const watch = fs.createReadStream(null, { fd: Number(PARENT_PIPE_FD) });
    const onGone = () => {
      log("parent gone (pipe EOF); shutting down browser");
      shutdown(0);
    };
    watch.on("data", () => {}); // flowing mode so "end" fires on EOF
    watch.on("end", onGone);
    watch.on("close", onGone);
    watch.on("error", onGone);
  } catch (e) {
    log("parent-death watch unavailable:", e.message);
  }
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let req;
  try {
    req = JSON.parse(trimmed);
  } catch (e) {
    log("dropping non-JSON line:", e.message);
    return;
  }
  // A mid-operation prompt answer is id-less and must be consumed HERE, before
  // op dispatch: the op that raised the prompt is still awaiting its result on
  // the actor, so this reply is out of band relative to the request stream.
  if (req && req.op === "provideInput") {
    resolvePrompt(req);
    return;
  }
  // Requests are serialised by the Rust actor, so sequential handling is fine.
  handle(req);
});
rl.on("close", () => shutdown(0));

log("ready");
