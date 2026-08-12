# Office 365 web session sidecar

A small Node process that drives a persistent (Playwright) browser and answers
the `not-yet-done-office365-web` crate over newline-delimited JSON on
stdin/stdout. It exists because some Office 365 tenants block the Graph API
(device-compliance Conditional Access) and serve the calendar only through
Outlook on the web, whose data plane needs a short-lived MSAL bearer minted
_inside_ the browser — cookies alone return 401.

## Setup

From this directory:

```sh
npm install
npx playwright install chromium
```

Then point the crate at this script via the `NYD_OFFICE365_SIDECAR` environment
variable (absolute path), or set `sidecar_script:` in the connection config:

```sh
export NYD_OFFICE365_SIDECAR="$PWD/index.js"
```

## First run (interactive login)

The first fetch against a fresh profile is not signed in, so the sidecar opens a
**headed** browser window for you to complete MFA. Finish the sign-in there; the
session (and its SSO cookies) persists into `profile_dir`, and subsequent runs
are headless and unattended. If a fetch reports `interactive login required`,
complete the login in the window and retry.

## Protocol

One JSON object per line each way. stdout is **pure JSONL** — all logging goes
to stderr.

| Request                                                              | Response                                           |
| -------------------------------------------------------------------- | -------------------------------------------------- |
| `{"id":1,"op":"configure","params":{"username":"…","password":"…"}}` | `{"id":1,"ok":true,"result":{"ok":true}}`          |
| `{"id":2,"op":"ensureLogin","params":{"loginHint":"…"}}`             | `{"id":2,"ok":true,"result":{"state":"loggedIn"}}` |
| `{"id":3,"op":"getCalendarView","params":{"start":"…Z","end":"…Z"}}` | `{"id":3,"ok":true,"result":{"events":[…]}}`       |

`configure` is sent once, up front, by the Rust launcher to hand over the
resolved sign-in credentials. They travel over this protocol — **never** via the
environment, because the browser child inherits the process env and that would
leak the password. Fields are optional; the username defaults to the login hint.

Errors: `{"id":N,"ok":false,"error":{"kind":"loginRequired","message":"…"}}`.
The `kind` `loginRequired` maps to an auth error on the Rust side; anything else
is surfaced as a generic sidecar/protocol error.

## Configuration (env, set by the Rust launcher)

| Variable               | Meaning                                                 |
| ---------------------- | ------------------------------------------------------- |
| `NYD_O365_PROFILE_DIR` | Persistent browser profile (login/SSO survives here).   |
| `NYD_O365_HEADLESS`    | `"1"` headless (default) / `"0"` headed.                |
| `NYD_O365_LOGIN_HINT`  | UPN to prefill on the interactive login (optional).     |
| `NYD_O365_START_URL`   | Entry URL (optional; defaults to the Outlook calendar). |

## Login automation (`login.js`)

The Azure AD / Entra sign-in is not a single form: with or without session
cookies the same URL lands on an email field, an **account picker** (tile list —
the default once any account is remembered), a password field, a "stay signed
in?" prompt, or a second-factor (MFA) challenge. `login.js` models this as an
explicit state machine:

- `classifyLoginPage(page)` recognises the current state from stable element
  ids / names / roles **and their visibility** (never visible text, so it is
  locale-independent; the email and password inputs coexist in the DOM and only
  visibility distinguishes them).
- `decideLoginStep(state, creds)` is a pure function mapping a state to the next
  action.
- `runLogin(page, creds)` loops classify → decide → act until the app is reached
  or it hands off. **MFA is never automated** — it hands off to the headed
  window for the user to finish.

The credentials come from the `configure` op (see the protocol above); with a
password set, `ensureLogin`/`getCalendarView` drive the sign-in unattended up to
the MFA prompt, then relaunch headed so you complete only the second factor.
Without a password the login stays fully manual, as before.

## Tests

```sh
npm test
```

`test/login.test.js` runs the classifier against static HTML fixtures in
`test/fixtures/` that mirror the exact markup of the live sign-in pages
(verified against the real login), plus pure-function tests for the step
decider and `isAppUrl`. Fully offline — no network, tenant, or real account;
all fixture data is invented.

## How a fetch works

1. Drive the browser to the calendar; relaunch headed for MFA if not signed in.
2. Observe the app's own `POST /owa/service.svc?action=GetCalendarView` to
   capture its exact (MCAS-proxied) URL, headers — including the bearer — and
   body. Nothing tenant-specific is hardcoded; it is all captured at runtime.
3. Replay that request with the body's date range rewritten to the requested
   window and parse the EWS JSON into events.
4. Fallback: if the request can't be captured, scrape the rendered DOM (current
   view only) so a failure is visible rather than silent.
