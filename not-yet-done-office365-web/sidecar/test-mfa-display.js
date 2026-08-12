#!/usr/bin/env node
// Standalone MFA-display spike — NOT part of the sidecar protocol.
//
// Launches a HEADED Chromium with a persistent profile and drives the AAD
// sign-in via the same state machine the sidecar uses (login.js). When the flow
// reaches the number-match MFA challenge, it scrapes the match number
// (#idRichContext_DisplaySign) plus the challenge title and prints them
// prominently to the TERMINAL — so we can prove the number is available to us
// out-of-band, ahead of the eventual "headless-from-start + show MFA in a
// window" design.
//
// AUTH CRITERION — readability, and NOTHING else. "Logged in" is judged ONLY by
// a successful (HTTP 200) GetCalendarView RESPONSE. The URL being an app URL
// (outlook.office.com/calendar/...) means nothing: the app shell loads at that
// URL even when signed out and only its data fetch reveals the truth. So this
// script watches responses, not URLs.
//
// It NEVER closes the browser on success — it leaves the window open and hangs
// so you can inspect it and close it yourself (Ctrl-C here).
//
// Usage:
//   NYD_O365_PROFILE_DIR=/path/to/profile \
//   NYD_O365_LOGIN_HINT=user@example.com \
//   NYD_O365_PASSWORD_CMD='pass show work/.../pass' \
//     node test-mfa-display.js
//
// Env:
//   NYD_O365_PROFILE_DIR   REQUIRED. Persistent browser profile directory.
//   NYD_O365_LOGIN_HINT    UPN to type / pick (optional if profile remembers it).
//   NYD_O365_PASSWORD      Password literal (optional; prefer *_CMD).
//   NYD_O365_PASSWORD_CMD  Shell command whose stdout is the password
//                          (e.g. `pass show ...`); trimmed. Used only if
//                          NYD_O365_PASSWORD is unset.
//   NYD_O365_START_URL     Entry URL (default: Outlook web calendar month view).
//
// WARNING — profile lock: Chromium takes an exclusive lock on the profile
// directory. Do NOT point this at a profile that the running TUI/sidecar is
// already using (e.g. a live connection profile) — stop that sidecar first, or copy
// the profile to a throwaway directory and point NYD_O365_PROFILE_DIR there.

import { execSync } from "node:child_process";
import { chromium } from "playwright";
import {
  runLogin,
  classifyLoginPage,
  LoginState,
} from "./login.js";

const PROFILE_DIR = process.env.NYD_O365_PROFILE_DIR || "";
const LOGIN_HINT = process.env.NYD_O365_LOGIN_HINT || "";
// Headless-from-start when NYD_O365_HEADLESS is truthy. This is the target mode:
// no browser window at all, the MFA number-match code surfaced only in the
// terminal. Default is headed (visible) for eyeballing the flow.
const HEADLESS = /^(1|true|yes|on)$/i.test(process.env.NYD_O365_HEADLESS || "");
const START_URL =
  process.env.NYD_O365_START_URL ||
  "https://outlook.office.com/calendar/view/month";

// The data-plane request whose 200 response proves the calendar is readable.
// Same matcher the sidecar uses; matches on path so it works behind the MCAS
// reverse proxy (*.mcas.ms) too.
const CALENDAR_VIEW_RX = /\/owa\/service\.svc\?.*action=GetCalendarView/i;

// Overall budget to reach a readable calendar (covers manual MFA).
const OVERALL_MS = 10 * 60 * 1000;
// Poll cadence while waiting/observing.
const POLL_MS = 1500;

// --- readability truth (set ONLY by a 200 GetCalendarView response) ---------
let _calReadable = false;
let _sessionProxied = false;

function ts() {
  return new Date().toISOString().slice(11, 19);
}
function log(msg) {
  process.stderr.write(`[${ts()}] ${msg}\n`);
}

function installReadableWatch(ctx) {
  ctx.on("response", (resp) => {
    try {
      const req = resp.request();
      if (req.method() !== "POST" || !CALENDAR_VIEW_RX.test(req.url())) return;
      const status = resp.status();
      if (status === 200) {
        if (!_sessionProxied && /\.mcas\.ms/i.test(req.url())) {
          _sessionProxied = true;
          log("calendar served via MCAS proxy (.mcas.ms)");
        }
        if (!_calReadable) log("calendar READABLE (GetCalendarView → 200)");
        _calReadable = true;
      } else if (status === 401 || status === 403) {
        if (_calReadable) log(`calendar no longer readable (→ ${status})`);
        _calReadable = false;
      }
    } catch {
      /* never let the watcher break anything */
    }
  });
}

/** Resolve the password from env or a command; empty string if neither set. */
function resolvePassword() {
  if (process.env.NYD_O365_PASSWORD) return process.env.NYD_O365_PASSWORD;
  const cmd = process.env.NYD_O365_PASSWORD_CMD;
  if (!cmd) return "";
  try {
    return execSync(cmd, { encoding: "utf8" }).replace(/\r?\n$/, "").trim();
  } catch (e) {
    log(`password command failed: ${e.message}`);
    return "";
  }
}

/** Best-effort visible text of the first matching, visible element. */
async function visibleTextOf(page, selector) {
  try {
    const el = page.locator(selector).first();
    if (await el.isVisible({ timeout: 500 }).catch(() => false)) {
      return ((await el.textContent()) || "").trim();
    }
  } catch {
    /* ignore */
  }
  return "";
}

/** Scrape the number-match code + surrounding instruction text. */
async function scrapeMfa(page) {
  const number = await visibleTextOf(page, "#idRichContext_DisplaySign");
  const title =
    (await visibleTextOf(page, "#idDiv_SAOTCAS_Title")) ||
    (await visibleTextOf(page, "#idDiv_SAASTO_Title"));
  const desc = await visibleTextOf(page, "#idDiv_SAOTCAS_Description");
  return { number, title, desc };
}

/** Print the MFA challenge prominently to the terminal (stdout). */
function printMfaBanner({ number, title, desc }) {
  const bar = "=".repeat(60);
  const lines = ["", bar, "  MICROSOFT AUTHENTICATOR — NUMBER MATCH", bar];
  if (title) lines.push(`  ${title}`);
  if (desc) lines.push(`  ${desc}`);
  lines.push("");
  if (number) {
    lines.push(`      >>>  ENTER THIS NUMBER:   ${number}  <<<`);
  } else {
    lines.push(
      "      (no number-match code found — this MFA method may be a",
      "       code entry or approve-only prompt; complete it in the",
      "       browser window)",
    );
  }
  lines.push("", bar, "");
  process.stdout.write(lines.join("\n") + "\n");
}

function printSuccessBanner() {
  const bar = "=".repeat(60);
  process.stdout.write(
    ["", bar, "  ✔  CALENDAR READABLE — login complete.", bar, ""].join("\n") +
      "\n",
  );
}

/** Let redirects/data-fetches settle after a navigation or an action. */
async function settle(page) {
  await page
    .waitForLoadState("networkidle", { timeout: 8000 })
    .catch(() => {});
}

/** True if an error is "the page/context/browser is gone" (e.g. user closed it). */
function isClosedError(e) {
  const m = (e && e.message) || String(e);
  return /has been closed|Target (page|closed)|browser has disconnected/i.test(m);
}

/**
 * Structural inventory of the page's VISIBLE interactive elements — tag, id,
 * name, type, role, plus the PRESENCE (not value) of aria-label / placeholder.
 * No text, no values → no UPN / password / code can leak. Used to learn what a
 * federated IdP login page actually looks like so we can drive it correctly.
 */
async function dumpStructure(page) {
  return page
    .evaluate(() => {
      const sel = "input, button, [role='button'], a[href], form, select";
      const out = [];
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (!(r.width > 0 && r.height > 0)) continue; // visible only
        const p = [el.tagName.toLowerCase()];
        if (el.id) p.push(`#${el.id}`);
        if (el.name) p.push(`[name=${el.name}]`);
        if (el.type) p.push(`[type=${el.type}]`);
        const role = el.getAttribute("role");
        if (role) p.push(`[role=${role}]`);
        if (el.getAttribute("aria-label") != null) p.push("[aria-label]");
        if (el.getAttribute("placeholder") != null) p.push("[placeholder]");
        if (el.getAttribute("autocomplete")) p.push(`[ac=${el.getAttribute("autocomplete")}]`);
        out.push(p.join(""));
      }
      return out.slice(0, 40).join("  ");
    })
    .catch((e) => `(dumpStructure failed: ${e.message})`);
}

/** Redact everything but the host+path of a URL, for safe logging. */
function safeUrl(u) {
  try {
    const url = new URL(u);
    return url.origin + url.pathname;
  } catch {
    return "(unparseable url)";
  }
}

async function main() {
  if (!PROFILE_DIR) {
    log("NYD_O365_PROFILE_DIR is required (a persistent profile directory).");
    process.exit(2);
  }
  log(`profile: ${PROFILE_DIR}`);
  log(
    "reminder: do NOT reuse a profile the running sidecar/TUI holds " +
      "(exclusive lock) — stop it or copy the profile first.",
  );

  const creds = {
    username: LOGIN_HINT || undefined,
    password: resolvePassword() || undefined,
  };
  log(
    `credentials: username=${creds.username ? "set" : "none"}, ` +
      `password=${creds.password ? "set" : "none"}`,
  );

  log(`launching ${HEADLESS ? "HEADLESS" : "HEADED"} Chromium (persistent context)…`);
  const ctx = await chromium.launchPersistentContext(PROFILE_DIR, {
    headless: HEADLESS,
    viewport: null,
    args: ["--no-first-run", "--no-default-browser-check"],
  });
  installReadableWatch(ctx);
  const page = ctx.pages()[0] || (await ctx.newPage());

  // The browser may vanish from under us — the user closes the window, or a
  // navigation detaches the page. Track it so the loop stops cleanly instead of
  // throwing a scary "Target page has been closed" stack.
  let closed = false;
  const markClosed = (what) => {
    if (!closed) log(`${what} closed.`);
    closed = true;
  };
  ctx.on("close", () => markClosed("browser context"));
  page.on("close", () => markClosed("page"));

  log(`navigating to ${START_URL}`);
  await page
    .goto(START_URL, { waitUntil: "domcontentloaded", timeout: 60000 })
    .catch((e) => log(`initial goto: ${e.message}`));
  await settle(page);

  const deadline = Date.now() + OVERALL_MS;
  let lastNumber = "";
  let mfaAnnounced = false;
  let lastState = null;
  const dumpedStates = new Set();

  try {
    while (Date.now() < deadline) {
      if (closed || _calReadable) break;

      const c = await classifyLoginPage(page).catch(() => null);
      const state = c ? c.state : LoginState.UNKNOWN;

      if (state !== lastState) {
        log(`state: ${state} @ ${safeUrl(page.url())}`);
        lastState = state;
      }

      // First time we see each state, dump its structure (structure only — no
      // values leak). On PASSWORD, also probe whether the EMAIL field is ALSO
      // visible — if so, the "password" classification is wrong: it is really
      // the email view (AAD ships both inputs and only toggles which shows).
      if (!dumpedStates.has(state)) {
        dumpedStates.add(state);
        log(`page structure [${state}]: ${await dumpStructure(page)}`);
        if (state === LoginState.PASSWORD) {
          const vis = await page
            .evaluate(() => {
              const seen = (id) => {
                const el = document.getElementById(id);
                if (!el) return "absent";
                const r = el.getBoundingClientRect();
                const cs = getComputedStyle(el);
                return `box=${r.width}x${r.height} display=${cs.display} vis=${cs.visibility} opacity=${cs.opacity}`;
              };
              return { i0116: seen("i0116"), i0118: seen("i0118") };
            })
            .catch((e) => ({ error: e.message }));
          log(`  email(#i0116): ${vis.i0116}`);
          log(`  passwd(#i0118): ${vis.i0118}`);
        }
      }

      if (state === LoginState.MFA) {
        const shown = await scrapeMfa(page);
        if (!mfaAnnounced || (shown.number && shown.number !== lastNumber)) {
          if (mfaAnnounced) log("number-match code changed — reprinting.");
          else log("reached MFA — showing the number-match code.");
          printMfaBanner(shown);
          lastNumber = shown.number;
          mfaAnnounced = true;
        }
        await page.waitForTimeout(POLL_MS);
        continue;
      }

      if (
        state === LoginState.EMAIL ||
        state === LoginState.PASSWORD ||
        state === LoginState.ACCOUNT_PICKER ||
        state === LoginState.STAY_SIGNED_IN
      ) {
        log(`login step: ${state} — driving with runLogin.`);
        // runLogin drives classify→act until it reaches an app URL, MFA, or a
        // handoff. We ignore its ok/APP verdict entirely: only _calReadable is
        // trusted. This just advances the form.
        await runLogin(page, creds, { log }).catch((e) =>
          log(`runLogin: ${e.message}`),
        );
        await settle(page);
        continue;
      }

      // APP url (shell showing, not yet proven readable) or UNKNOWN: just wait —
      // the app's own GetCalendarView will resolve to 200 (readable) or 401/403
      // (bounce back to login), and the watcher reflects it.
      await page.waitForTimeout(POLL_MS);
    }
  } catch (e) {
    if (isClosedError(e)) markClosed("browser");
    else throw e;
  }

  if (_calReadable) {
    printSuccessBanner();
    if (_sessionProxied) log("note: this session is MCAS-proxied (*.mcas.ms).");
  } else if (closed) {
    log("browser was closed before the calendar became readable — stopping.");
    process.exit(1);
  } else {
    log("timed out before the calendar became readable.");
  }

  if (closed) {
    log("browser is gone — nothing to keep open. bye.");
    process.exit(_calReadable ? 0 : 1);
  }

  if (HEADLESS) {
    // No window to inspect — close cleanly and report the outcome.
    log("headless: closing and exiting.");
    await ctx.close().catch(() => {});
    process.exit(_calReadable ? 0 : 1);
  }

  // Headed: NEVER auto-close on success — leave the window open; user closes it.
  log("leaving the browser open — inspect it, then press Ctrl-C here to quit.");
  await new Promise(() => {});
}

main().catch((e) => {
  log(`fatal: ${e && e.stack ? e.stack : e}`);
  process.exit(1);
});
