// Tests for the login state machine (login.js).
//
// Two layers:
//   * classifyLoginPage — driven against real Playwright pages loaded from the
//     static HTML fixtures in ./fixtures. These mirror the exact ids/names/
//     roles of the live AAD pages (verified against the real login), so a
//     change in how a state is recognised is caught here — without any network,
//     tenant, or real account. All fixture data is invented.
//   * decideLoginStep / isAppUrl — pure functions, tested directly.
//
// Run: `npm test` (from the sidecar dir), or `node --test test/`.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import { chromium } from "playwright";

import {
  LoginState,
  classifyLoginPage,
  clickUntilGone,
  decideLoginStep,
  isAppUrl,
  tileSelector,
} from "../login.js";

const FIXTURES = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "fixtures",
);
const fixtureUrl = (name) => pathToFileURL(path.join(FIXTURES, name)).href;

let browser;
let page;

before(async () => {
  browser = await chromium.launch();
  page = await browser.newPage();
});

after(async () => {
  await browser?.close();
});

async function classifyFixture(name) {
  await page.goto(fixtureUrl(name));
  return classifyLoginPage(page);
}

// --- classification against real fixture pages ------------------------------

test("email screen classifies as EMAIL (hidden password ignored)", async () => {
  const c = await classifyFixture("email.html");
  assert.equal(c.state, LoginState.EMAIL);
  assert.equal(c.errorText, "");
});

test("account picker classifies as ACCOUNT_PICKER", async () => {
  const c = await classifyFixture("account-picker.html");
  assert.equal(c.state, LoginState.ACCOUNT_PICKER);
});

test("password screen classifies as PASSWORD (hidden email ignored)", async () => {
  const c = await classifyFixture("password.html");
  assert.equal(c.state, LoginState.PASSWORD);
});

test("password screen with an inline error surfaces the error text", async () => {
  const c = await classifyFixture("password-error.html");
  assert.equal(c.state, LoginState.PASSWORD);
  assert.match(c.errorText, /incorrect/i);
});

test("stay-signed-in screen classifies as STAY_SIGNED_IN", async () => {
  const c = await classifyFixture("kmsi.html");
  assert.equal(c.state, LoginState.STAY_SIGNED_IN);
});

test("MFA challenge classifies as MFA", async () => {
  const c = await classifyFixture("mfa.html");
  assert.equal(c.state, LoginState.MFA);
});

// --- the account tile is matched by UPN via data-test-id --------------------

test("the configured account's tile is present and selectable", async () => {
  await page.goto(fixtureUrl("account-picker.html"));
  const tile = page.locator(tileSelector("user@example.com"));
  assert.equal(await tile.count(), 1);
  // A different, non-remembered account has no tile → the flow falls back to
  // "use another account".
  const missing = page.locator(tileSelector("nobody@example.com"));
  assert.equal(await missing.count(), 0);
});

// --- clickUntilGone: the hydration-race retry -------------------------------

test("clickUntilGone retries past a not-yet-hydrated tile", async () => {
  await page.goto(fixtureUrl("account-picker-hydrating.html"));
  // First click lands before the handler attaches (a no-op); the retry, after
  // the short settle window, hits the hydrated tile and the picker gives way.
  const advanced = await clickUntilGone(
    page,
    () => page.locator(tileSelector("user@example.com")).first(),
    "#tilesHolder",
    () => {},
    { attempts: 4, settleMs: 400, pollMs: 50 },
  );
  assert.equal(advanced, true);
});

test("clickUntilGone gives up when the click never advances the flow", async () => {
  await page.goto(fixtureUrl("account-picker-dead.html"));
  const advanced = await clickUntilGone(
    page,
    () => page.locator(tileSelector("user@example.com")).first(),
    "#tilesHolder",
    () => {},
    { attempts: 3, settleMs: 150, pollMs: 50 },
  );
  assert.equal(advanced, false);
});

// --- isAppUrl (pure) --------------------------------------------------------

test("isAppUrl distinguishes app surfaces from login pages", () => {
  assert.equal(isAppUrl("https://outlook.office.com/calendar/view/week"), true);
  assert.equal(isAppUrl("https://outlook.office365.com/owa/"), true);
  assert.equal(
    isAppUrl("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
    false,
  );
  assert.equal(isAppUrl("https://login.live.com/"), false);
  assert.equal(isAppUrl("https://fs.example.com/adfs/ls/"), false);
});

// --- decideLoginStep (pure) -------------------------------------------------

const CREDS = { username: "user@example.com", password: "s3cr3t" };

test("EMAIL → fill the username", () => {
  const step = decideLoginStep({ state: LoginState.EMAIL }, CREDS);
  assert.deepEqual(step, { action: "fillEmail", value: "user@example.com" });
});

test("ACCOUNT_PICKER → pick the account by username", () => {
  const step = decideLoginStep({ state: LoginState.ACCOUNT_PICKER }, CREDS);
  assert.deepEqual(step, { action: "pickAccount", value: "user@example.com" });
});

test("PASSWORD → fill the password", () => {
  const step = decideLoginStep({ state: LoginState.PASSWORD }, CREDS);
  assert.deepEqual(step, { action: "fillPassword", value: "s3cr3t" });
});

test("STAY_SIGNED_IN → confirm to persist SSO", () => {
  const step = decideLoginStep({ state: LoginState.STAY_SIGNED_IN }, CREDS);
  assert.deepEqual(step, { action: "confirmStaySignedIn" });
});

test("MFA → hand off (never automated)", () => {
  const step = decideLoginStep({ state: LoginState.MFA }, CREDS);
  assert.equal(step.action, "handoff");
});

test("APP → done", () => {
  const step = decideLoginStep({ state: LoginState.APP }, CREDS);
  assert.deepEqual(step, { action: "done" });
});

test("missing password on PASSWORD → hand off, not a crash", () => {
  const step = decideLoginStep(
    { state: LoginState.PASSWORD },
    { username: "user@example.com" },
  );
  assert.equal(step.action, "handoff");
});

test("missing username on EMAIL → hand off", () => {
  const step = decideLoginStep({ state: LoginState.EMAIL }, {});
  assert.equal(step.action, "handoff");
});
