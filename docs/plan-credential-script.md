# Script-Driven Credentials — Plan

Status: **implemented** (Ph1–Ph4); only the live smoke tests in
`smoke-tests.md` ("Skriptgetriebene Credentials") are still open. Replaced the
`form-command` provider shipped in `b2c2e1b` (see `plan-auth-mechanisms.md`
Ph3), which was removed again.

## Goal

One script supplies **several** credential fields at once, and asks the
user something **only when it actually has to**.

The motivating case: credentials live in `pass`. Normally the GPG agent is
unlocked, the script prints username and token, and nothing is asked. When
the store is locked, today `pass` reaches for `pinentry` — a second window,
outside the TUI, with no idea it is inside a login flow. Instead the script
should hand that question back to the frontend and get the answer as data.

`form-command` cannot do either: it produces exactly one value for exactly
one binding (so `pass` would run twice), and it always asks first, even
when nothing needs asking.

## Config shape

```yaml
auth:
  mechanism: user-api-token
  script: ~/.config/not_yet_done/scripts/pass_credentials.py username=… token=…
  bindings:
    - field: username
      provider: { type: script-result }
    - field: token
      provider: { type: script-result }
```

- `script` sits on `auth:`, not on the provider: **exactly one script per
  auth block, invoked once**. Several script blocks would read as several
  invocations, which is the opposite of the point.
- `script-result` takes no parameters. The binding's `field` name is the
  key looked up in the script's result object.
- Provider ids are kebab-case throughout (`user-api-token`, `until-rejected`),
  so it is `script-result`, not `script_result`.
- The string is run through `sh -c`, so `~` and arguments work.

Validation (in `AuthSpec::validate_against`, i.e. at config-read time):

- a binding uses `script-result` but `auth.script` is unset → error
- `auth.script` is set but no binding uses `script-result` → error

## Protocol

One process per round. Request on stdin, answer on stdout, both JSON.

**stdin** — always both keys; `input` is `{}` on the first round and
accumulates every answer collected so far (the script is a fresh process
each round and cannot remember anything itself):

```json
{ "request": ["username", "token"], "input": { "password": "…" } }
```

`request` lists the field names the config wants, so one generic script can
serve several adapters without guessing.

**stdout** — exactly one of three keys:

```json
{ "result": { "username": "…", "token": "…" } }
```

```json
{
  "form": {
    "header": "Unlock the password store",
    "error": "that passphrase was rejected",
    "fields": [
      {
        "name": "password",
        "label": "Password-store passphrase",
        "masked": true,
        "optional": false,
        "prefill": null
      }
    ]
  }
}
```

```json
{ "error": "no password store at ~/.password-store" }
```

- `result` ends the loop. Every name in `request` must be present.
- `form` is rendered by the frontend; the answers go back in `input` on the
  next round. `field.name` is mandatory — it is the key in `input`, and
  `type: password` alone cannot key two password fields.
- `form.error` is how a script says "wrong passphrase, try again": the
  message is shown with the re-asked form. Top-level `error` aborts.
- A non-zero exit also aborts, with stderr as the message.
- `masked` only for now; a richer `type` taxonomy can come later.
- Round cap: **5**, then abort — a script that keeps asking must not loop
  forever.

## Frontend

- `AdapterStatus::NeedsCreds` gains `header: Option<String>` and
  `error: Option<String>`; `AuthField` already carries name/label/masked/
  optional/prefill.
- Enter accepts, **Esc cancels**. Cancelling today only closes the TUI popup
  while the orchestrator keeps waiting on its oneshot — the login hangs with
  the auth mutex held. So: `ContentAdapter::cancel_credentials()` (default
  `NotSupported`) → `AuthOrchestrator::cancel_prompt()` drops the pending
  sender → the awaiting login fails with `PromptCancelled`.
- Plain `prompt` bindings are collected first, in one dialog; the script's
  forms follow as their own dialogs. Mixing both is an edge case, and the
  lazy decision makes a single merged dialog impossible anyway.
- No TTY (pipe, cron) → the existing "no terminal to ask on" error.

## Phases

1. **Core** — `content/src/auth/credential_script.rs` (protocol + round
   parsing), `AuthSpec.script` / `script_timeout_secs`,
   `CredentialProvider::ScriptResult`, orchestrator round loop, remove
   `form_command.rs` and the `FormCommand` variant, `NeedsCreds` header /
   error, cancel plumbing.
2. **Frontends** — TUI popup header + error + cancel wiring; CLI prints
   header and error before prompting.
3. **Wizard** — `config build` offers `script-result` and asks for
   `auth.script` once; `config auth <type>` lists it (schema-sourced, so it
   follows automatically).
4. **Docs + script** — README "Authentication" section rewritten off
   `form-command`, `content-adapter-spec.md`, example YAMLs, smoke tests,
   and a `pass_credentials.py` under `~/.config/not_yet_done/scripts/`
   (outside the repo) that decrypts with `--pinentry-mode error` first and
   only asks for the passphrase when that fails.
