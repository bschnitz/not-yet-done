// Login state machine for the Microsoft Azure AD / Entra web sign-in.
//
// The AAD login is NOT a single deterministic form: depending on whether the
// browser profile already has session cookies, the same URL lands you on any
// of several pages — an email field, an "account picker" tile list, a password
// field, a "stay signed in?" prompt, or a second-factor (MFA) challenge. The
// exact page is decided server-side and can differ per visit.
//
// This module turns that into an explicit state machine:
//   1. `classifyLoginPage(page)` inspects the LIVE page and returns which state
//      it is in. It keys on stable element ids / names / roles and their
//      VISIBILITY — never on visible text — so it is locale-independent (the
//      real tenant renders in German). Visibility matters because AAD ships the
//      email (#i0116) and password (#i0118) inputs in the DOM together and only
//      toggles which is shown; presence alone would misclassify.
//   2. `decideLoginStep(state, creds)` is a PURE function: given the state and
//      the resolved credentials, it returns the action to take. Kept pure so it
//      is unit-testable without a browser.
//   3. `runLogin(page, creds)` drives the loop: classify → decide → act → settle,
//      until the app surface is reached or a step hands off to the user (MFA is
//      inherently interactive and is never automated).
//
// The credentials are passed in per call and never stored here.

// --- states -----------------------------------------------------------------

export const LoginState = {
  /** Authenticated Outlook/OWA app surface reached — login is complete. */
  APP: "app",
  /** Email / username input (the classic first screen; also "use another account"). */
  EMAIL: "email",
  /** "Pick an account" tile list (shown when one or more accounts are remembered). */
  ACCOUNT_PICKER: "account-picker",
  /** Password input. */
  PASSWORD: "password",
  /** "Stay signed in?" prompt — answering Yes persists SSO into the profile. */
  STAY_SIGNED_IN: "stay-signed-in",
  /** Second-factor challenge (authenticator, code, number match). Not automatable. */
  MFA: "mfa",
  /** No known marker matched — hand off to the user. */
  UNKNOWN: "unknown",
};

// --- selectors (verified against the live login pages) ----------------------

export const SEL = {
  email: '#i0116, input[name="loginfmt"]',
  password: '#i0118, input[name="passwd"]',
  submit: "#idSIButton9",
  back: "#idBtn_Back",
  tilesHolder: "#tilesHolder",
  otherTile: "#otherTile",
  // A remembered-account tile carries the account's UPN as its data-test-id,
  // which is exactly what we match on — locale-independent and unambiguous.
  anyTile: '#tilesHolder [role="button"][data-test-id]',
  kmsiCheckbox: "#KmsiCheckboxField",
  usernameError: "#usernameError",
  passwordError: "#passwordError",
  // MFA comes in several shapes; any of these markers means "second factor".
  mfa: [
    "#idDiv_SAOTCAS_Title", // approve-a-notification
    "#idDiv_SAASTO_Title", // "having trouble" / methods
    "#idRichContext_DisplaySign", // number-match display
    "#idTxtBx_SAOTCC_OTC", // one-time-code input
    '[data-testid="passkeyContainer"]',
  ].join(", "),
  // Two MFA shapes we can surface through a prompt instead of the browser
  // window: the number-match code to *display* (the user approves on their
  // phone) and the one-time-code to *type* (then submit).
  mfaNumber: "#idRichContext_DisplaySign",
  mfaOtc: "#idTxtBx_SAOTCC_OTC",
  mfaOtcSubmit: "#idSubmit_SAOTCC_Continue",
};

/** CSS selector for the tile of a specific account. */
export function tileSelector(username) {
  // Escape the double quotes; UPNs never contain quotes in practice.
  return `#tilesHolder [role="button"][data-test-id="${username}"]`;
}

// Hosts that mean "still on the sign-in flow", not the app.
const LOGIN_HOST_RX =
  /(login\.microsoftonline|login\.live|login\.windows|\/common\/oauth2|\/oauth2\/|sso|adfs|fs\.)/i;

/** Is this URL an authenticated Outlook/OWA app surface (not a login page)? */
export function isAppUrl(url) {
  return (
    /outlook\.(office|office365)\.com|outlook-office|\/owa\/|\/calendar/i.test(
      url,
    ) && !LOGIN_HOST_RX.test(url)
  );
}

// --- classification ---------------------------------------------------------

/**
 * First *actually shown* element matching `selector`, or null.
 *
 * Not just Playwright's `isVisible()`: AAD ships the email (#i0116) and password
 * (#i0118) inputs in the DOM together and toggles between them by animating
 * OPACITY — both stay `display:block`, so `isVisible()` reports the hidden one
 * (opacity:0) as visible too. Since the classifier keys on which input is shown
 * to tell the email page from the password page, we must also reject near-zero
 * opacity, or the email page misreads as the password page.
 */
async function firstVisible(page, selector) {
  const loc = page.locator(selector);
  const n = await loc.count();
  for (let i = 0; i < n; i++) {
    const el = loc.nth(i);
    if (!(await el.isVisible().catch(() => false))) continue;
    const opaque = await el
      .evaluate((node) => parseFloat(getComputedStyle(node).opacity) >= 0.1)
      .catch(() => true); // on error, don't over-reject
    if (opaque) return el;
  }
  return null;
}

/** Trimmed text of the first visible element matching `selector`, or "". */
async function visibleText(page, selector) {
  const el = await firstVisible(page, selector);
  if (!el) return "";
  return (await el.textContent().catch(() => ""))?.trim() || "";
}

/**
 * Classify the current login page. Returns `{ state, url, errorText }`.
 * `errorText` is any inline error shown alongside an input (e.g. a wrong
 * password), used by the loop to detect a stuck login rather than retrying.
 *
 * Order is deliberate: the app check wins first; the account picker has no
 * inputs so it is checked before the input states; MFA is checked before the
 * input states because some MFA pages also carry a hidden password field.
 */
export async function classifyLoginPage(page) {
  const url = page.url();
  if (isAppUrl(url)) return { state: LoginState.APP, url, errorText: "" };

  const errorText = await visibleText(
    page,
    `${SEL.usernameError}, ${SEL.passwordError}`,
  );

  if (
    (await firstVisible(page, SEL.tilesHolder)) ||
    (await firstVisible(page, SEL.otherTile))
  ) {
    return { state: LoginState.ACCOUNT_PICKER, url, errorText };
  }
  if (await firstVisible(page, SEL.mfa)) {
    return { state: LoginState.MFA, url, errorText };
  }
  if (await firstVisible(page, SEL.password)) {
    return { state: LoginState.PASSWORD, url, errorText };
  }
  if (await firstVisible(page, SEL.email)) {
    return { state: LoginState.EMAIL, url, errorText };
  }
  // KMSI: no inputs, a Yes (submit) and No (back), usually a "keep signed in"
  // checkbox. Require the checkbox OR (submit AND back) — but only here, after
  // the input states, so the email page (which also has submit+back) can't
  // be mistaken for it.
  if (
    (await firstVisible(page, SEL.kmsiCheckbox)) ||
    ((await firstVisible(page, SEL.submit)) &&
      (await firstVisible(page, SEL.back)))
  ) {
    return { state: LoginState.STAY_SIGNED_IN, url, errorText };
  }
  return { state: LoginState.UNKNOWN, url, errorText };
}

// --- step decision (pure) ---------------------------------------------------

/**
 * Decide the next action from a classification and the resolved credentials.
 * Pure and browser-free. Returns one of:
 *   { action: "done" }
 *   { action: "fillEmail", value }
 *   { action: "pickAccount", value }
 *   { action: "fillPassword", value }
 *   { action: "confirmStaySignedIn" }
 *   { action: "handoff", reason }   // stop automation; let the user finish
 */
export function decideLoginStep(classification, creds) {
  const c = creds || {};
  switch (classification.state) {
    case LoginState.APP:
      return { action: "done" };

    case LoginState.EMAIL:
      return c.username
        ? { action: "fillEmail", value: c.username }
        : { action: "handoff", reason: "no username configured" };

    case LoginState.ACCOUNT_PICKER:
      // Prefer the matching remembered tile; without a username we cannot know
      // which account to pick, so hand off.
      return c.username
        ? { action: "pickAccount", value: c.username }
        : { action: "handoff", reason: "no username to pick an account" };

    case LoginState.PASSWORD:
      return c.password
        ? { action: "fillPassword", value: c.password }
        : { action: "handoff", reason: "no password configured" };

    case LoginState.STAY_SIGNED_IN:
      // Answering Yes persists SSO into the profile → fewer future logins.
      return { action: "confirmStaySignedIn" };

    case LoginState.MFA:
      return { action: "handoff", reason: "second factor required" };

    default:
      return {
        action: "handoff",
        reason: classification.errorText || "unrecognized login page",
      };
  }
}

// --- driving the browser ----------------------------------------------------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Wait until the page settles on a *stable, actionable* login state, absorbing
 * the transitional frames AAD shows while it swaps views client-side (a brief
 * blank / redirect page classifies as UNKNOWN, and the previous screen's
 * markers linger for a moment). Without this, the loop acts on a page that is
 * still mid-transition — e.g. handing off the instant the account picker gives
 * way to the password field, before the field is visible.
 *
 * Polls until either the app is reached, or the same non-UNKNOWN state is seen
 * on two consecutive reads (so a genuine transition, where the state flips,
 * keeps us waiting until it settles). On timeout, returns the last known state
 * seen — or UNKNOWN if nothing else ever showed.
 */
async function waitForStableClassification(page, opts = {}) {
  const { timeoutMs = 20000, pollMs = 300 } = opts;
  const deadline = Date.now() + timeoutMs;
  let prevState = null;
  let lastKnown = null;
  for (;;) {
    const c = await classifyLoginPage(page);
    if (c.state === LoginState.APP) return c;
    if (c.state !== LoginState.UNKNOWN) {
      if (c.state === prevState) return c; // same known state twice → stable
      prevState = c.state;
      lastKnown = c;
    }
    if (Date.now() >= deadline) {
      return lastKnown || c; // give up; act on the best we saw
    }
    await sleep(pollMs);
  }
}

/** Wait until `selector` is no longer visible (the view we just left is gone). */
async function waitForGone(page, selector, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!(await firstVisible(page, selector))) return;
    await sleep(200);
  }
}

/**
 * Click a target and confirm the click actually advanced the flow, retrying if
 * it did not. On the account picker the tiles render — and pass Playwright's
 * actionability checks (visible, stable, enabled, hit-testable) — BEFORE the
 * page's JS hydrates their click handlers, so an immediate click can land on a
 * not-yet-live button and do nothing; the view never changes. We cannot tell a
 * dead click from a merely slow one, so after each click we wait a SHORT window
 * for `leaveSelector` to disappear; if it is still there, hydration has almost
 * certainly finished by now, so we click again. Only after `attempts` fruitless
 * clicks do we give up (returning false).
 *
 * This replaces the old single-click-then-`waitForGone` on this step, which,
 * when the click was a no-op, sat out the whole 15 s dead-wait for a view change
 * that would never come. `getEl` is re-invoked each attempt so we always act on
 * the element that is currently visible. Returns true as soon as the picker is
 * gone.
 */
export async function clickUntilGone(
  page,
  getEl,
  leaveSelector,
  log = () => {},
  opts = {},
) {
  const { attempts = 3, settleMs = 1200, pollMs = 150 } = opts;
  const gone = async () => !(await firstVisible(page, leaveSelector));
  for (let a = 1; a <= attempts; a++) {
    if (await gone()) {
      // Smoking gun: if we bail here on attempt 1 we never clicked at all —
      // the leave-selector was absent, so we thought there was nothing to
      // advance. Log it so we can tell a real success from a silent no-op.
      if (a === 1) log(`clickUntilGone: "${leaveSelector}" absent at entry — NOT clicking`);
      return true;
    }
    const el = await getEl();
    if (!el) {
      log(`clickUntilGone: no visible target to click (attempt ${a})`);
    } else {
      await el.click();
      log(`clickUntilGone: clicked (attempt ${a})`);
    }
    const deadline = Date.now() + settleMs;
    while (Date.now() < deadline) {
      if (await gone()) return true;
      await sleep(pollMs);
    }
    log(`account-picker click #${a} did not advance the flow; retrying`);
  }
  return gone();
}

/**
 * Run `fn` with a temporary listener that logs sign-in network requests (POSTs
 * and login-host hits). Purely observational: it tells us live whether a click
 * produced the request it should, WITHOUT gating any behaviour on a specific
 * Microsoft endpoint — so the flow logic stays decoupled from tenant internals.
 */
async function withRequestDiagnostics(page, log, fn) {
  const onReq = (req) => {
    const u = req.url();
    if (req.method() === "POST" || LOGIN_HOST_RX.test(u)) {
      log(`  ↳ ${req.method()} ${u.slice(0, 120)}`);
    }
  };
  page.on("request", onReq);
  try {
    return await fn();
  } finally {
    page.off("request", onReq);
  }
}

/**
 * A structural inventory of the page's visible interactive elements, for
 * diagnosing an UNKNOWN login page so we can extend the classifier. Emits only
 * structure — tag, id, name, type, role, and the PRESENCE of aria-label /
 * data-test-id — never their values or any text, so no UPN / email / code
 * leaks into the log. `id`s on AAD pages are stable, non-secret markers.
 */
async function describeInteractive(page) {
  return page
    .evaluate(() => {
      const sel = "button, input, [role='button'], a[href], form";
      const seen = [];
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (!(r.width > 0 && r.height > 0)) continue; // visible only
        const a = el.attributes;
        const parts = [el.tagName.toLowerCase()];
        if (el.id) parts.push(`#${el.id}`);
        if (a.name) parts.push(`[name=${a.name.value}]`);
        if (a.type) parts.push(`[type=${a.type.value}]`);
        if (a.role) parts.push(`[role=${a.role.value}]`);
        if (el.getAttribute("aria-label") != null) parts.push("[aria-label]");
        if (el.getAttribute("data-test-id") != null) parts.push("[data-test-id]");
        seen.push(parts.join(""));
      }
      return seen.slice(0, 40).join("  ");
    })
    .catch(() => "(describeInteractive failed)");
}

/**
 * Type a value into a login input, VERIFY the field actually holds it, then
 * click submit. AAD validates against its client-side (React) state, which only
 * updates from the input events a keystroke fires — a `fill()` whose events race
 * the submit click can leave AAD seeing an EMPTY field ("enter a valid email").
 * So we fill, read the value back, and if it did not take, clear and type it
 * key-by-key before submitting. Never logs the value itself (only lengths).
 */
async function fillAndSubmit(page, selector, value, log = () => {}) {
  const el = await firstVisible(page, selector);
  if (!el) throw new Error(`input ${selector} vanished`);
  await el.click().catch(() => {});
  await el.fill(value).catch(() => {});
  let got = await el.inputValue().catch(() => "");
  if (got !== value) {
    log(`fill did not take (field has ${got.length} chars) — retyping key-by-key`);
    await el.fill("").catch(() => {});
    await el.pressSequentially(value, { delay: 25 }).catch(() => {});
    got = await el.inputValue().catch(() => "");
    if (got !== value) {
      log(`field STILL not matching after retype (${got.length} chars)`);
    }
  }
  await page.locator(SEL.submit).first().click();
}

/** Execute one decided step against the live page. */
async function applyStep(page, step, log) {
  switch (step.action) {
    case "fillEmail": {
      await fillAndSubmit(page, SEL.email, step.value, log);
      await waitForGone(page, SEL.email);
      break;
    }
    case "pickAccount": {
      // The account picker is the one step whose click is the SOLE progress
      // trigger, so a too-early click that misses the not-yet-hydrated handler
      // stalls the whole login. Two guards, both mirroring what a human does:
      //   1. click the VISIBLE tile — AAD keeps hidden duplicate/menu nodes in
      //      the DOM (e.g. a "<upn>-menu-dots" tile), so `.first()` could grab
      //      one of those; `firstVisible` picks the rendered tile.
      //   2. verify the picker actually gives way and re-click if not, instead
      //      of a single click followed by one long dead wait.
      const tileSel = tileSelector(step.value);
      const useTile = !!(await firstVisible(page, tileSel));
      // Diagnostic: counts/booleans only — no UPN, no tile text. Tells us
      // whether we even found a clickable tile, or fell back / found nothing.
      const tileCount = await page.locator(tileSel).count();
      const holderVisible = !!(await firstVisible(page, SEL.tilesHolder));
      log(
        `pickAccount: visible-tile=${useTile} matching-tiles=${tileCount} ` +
          `tilesHolder-visible=${holderVisible}`,
      );
      if (!useTile) {
        // Remembered accounts exist but none matches → use another account,
        // which lands on the email field for the configured user.
        log("no matching account tile; choosing 'use another account'");
      }
      const targetSel = useTile ? tileSel : SEL.otherTile;
      const advanced = await withRequestDiagnostics(page, log, () =>
        clickUntilGone(
          page,
          () => firstVisible(page, targetSel),
          SEL.tilesHolder,
          log,
        ),
      );
      if (!advanced) {
        // Don't throw: let the outer loop re-classify. If it still reads
        // ACCOUNT_PICKER, the loop guard hands off cleanly; a slow tenant may
        // yet have advanced between our last check and the next classification.
        log("account picker did not give way after retries");
      }
      break;
    }
    case "fillPassword": {
      await fillAndSubmit(page, SEL.password, step.value, log);
      await waitForGone(page, SEL.password);
      break;
    }
    case "confirmStaySignedIn": {
      await page.locator(SEL.submit).first().click();
      break;
    }
    default:
      throw new Error(`applyStep: nothing to do for "${step.action}"`);
  }
}

/**
 * Drive the login loop until the app surface is reached or a step hands off.
 * Returns `{ ok, state, reason? }`. Never types the password more than the
 * flow requires; tolerates the transient UNKNOWN frames AAD shows between
 * views, and stops on a repeated inline error or a genuinely stuck state
 * instead of looping.
 */
export async function runLogin(page, creds, opts = {}) {
  const { maxSteps = 15, log = () => {} } = opts;
  let lastError = "";
  let lastActionState = null;
  let repeats = 0;
  for (let i = 0; i < maxSteps; i++) {
    const tClassify = Date.now();
    const c = await waitForStableClassification(page);
    log(
      `login state: ${c.state}${c.errorText ? ` (error: ${c.errorText})` : ""}` +
        ` (classified in ${Date.now() - tClassify}ms)`,
    );

    // A persistent inline error means our credentials won't get us further.
    if (c.errorText && c.errorText === lastError) {
      return { ok: false, state: c.state, reason: c.errorText };
    }
    lastError = c.errorText;

    const step = decideLoginStep(c, creds);
    if (step.action === "done") return { ok: true, state: c.state };
    if (step.action === "handoff") {
      // When we hand off because the page is UNRECOGNISED, dump its structure
      // so we can teach the classifier what it was (privacy-safe: structure
      // only). MFA hand-offs are expected and need no dump.
      if (c.state === LoginState.UNKNOWN) {
        log(`unknown page inventory: ${await describeInteractive(page)}`);
      }
      return { ok: false, state: c.state, reason: step.reason };
    }

    // Loop guard: if the same actionable state recurs after we acted on it, our
    // action isn't advancing the flow — hand off rather than click forever.
    if (c.state === lastActionState) {
      if (++repeats >= 2) {
        return { ok: false, state: c.state, reason: `stuck on ${c.state}` };
      }
    } else {
      repeats = 0;
      lastActionState = c.state;
    }

    await applyStep(page, step, log);
  }
  return { ok: false, state: LoginState.UNKNOWN, reason: "login did not converge" };
}
