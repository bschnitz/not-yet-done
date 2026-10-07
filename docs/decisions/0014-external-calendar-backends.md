# 0014 — Calendar backends that nyd does not compile in run as separate programs

- **Status:** accepted, implemented
- **Date:** 2026-10-07
- **Affects:** `not-yet-done-calendar-external` (new: `protocol.rs`,
  `process.rs`, `config.rs`, `lib.rs`); `not-yet-done-calendar-core`
  (`subscribe_attention()` on `CalendarBackend`, serde on the event DTOs);
  `not-yet-done-calendar-adapter` (cargo feature `external`, the registry,
  `LoadState`'s attention); the workspace members. Doc surfaces:
  `docs/examples/views/calendar-adapter.yaml`, `docs/architecture.md`,
  [ADR 0010](0010-out-of-process-auth-plugins.md) (amendment).

## Context

A calendar backend is a crate behind a cargo feature: `microsoft` (Graph) and
`caldav` speak HTTP and serve anybody with an account. A calendar that neither
reaches — a web UI behind a browser sign-in, a proprietary API — needs code
that serves one particular setup, and until now that code also lived in this
workspace, behind a feature of its own.

That does not hold for a public repository. The workspace must build, test and
run without any of it, and a backend written for one setup should not have to
be published to be usable. The same question had already been answered for
logins: an auth plugin is a program nyd starts and talks to over JSON lines
([ADR 0010](0010-out-of-process-auth-plugins.md)). A calendar backend is the
same shape one level up — a long-lived process that may sign in, may need a
person for a second factor, and answers calls for as long as it runs.

Three options were weighed:

1. **Dynamic libraries** (`dlopen` of a `cdylib`). Rust has no stable ABI, so a
   plugin would have to be built with the exact compiler and crate versions of
   the host, and a crash in it takes the host down. Rejected.
2. **A private fork of the workspace** carrying the extra crates. Every change
   here becomes a merge there. Rejected.
3. **A separate program behind a line protocol (chosen).** Built, released
   and broken on its own, reached over stdin/stdout, and written in whatever
   language suits it. A Rust backend links this workspace's protocol crate.

## Decision

A connection with `backend: external` names a program; nyd starts it and
talks to it over JSON lines.

```yaml
connections:
  - id: web
    backend: external
    config:
      command: my-calendar-backend # run under `sh -c`
      name: Web calendar # "Account" column (optional)
      share: web # same command + share → one process
      secrets: # resolved by nyd, handed over in `start`
        password: { type: command, script: pass show example/calendar }
      config: # handed to the backend verbatim
        account: user@example.com
```

### The protocol

One JSON object per line, both ways, each an externally tagged variant
(`{"delete_event":{"id":3,"uid":"e1"}}`). **stdout is the protocol, stderr is
the log**; stderr goes to `<log dir>/calendar-<name>.log` through `child-log`,
as an auth plugin's does.

nyd writes:

| Line                                                                 | Meaning                                                                    |
| -------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `{"start":{"protocol":1,"connection":"web","config":…,"secrets":…}}` | Once, first. `config` is the opaque block, `secrets` the resolved values.  |
| `{"list_events":{"id":1,"range":…}}`                                 | Every event overlapping the range.                                         |
| `{"list_calendars":{"id":2}}`                                        | The calendars a new event may go into.                                     |
| `{"create_event":{"id":3,"calendar_id":null,"draft":…}}`             | Create an event; replies with it as created.                               |
| `{"update_event":{"id":4,"uid":"…","patch":…}}`                      | Change only what the patch names.                                          |
| `{"delete_event":{"id":5,"uid":"…"}}`                                | Delete one event (one occurrence, where the source addresses occurrences). |
| `{"toggle_window":{"id":6}}`                                         | Show or hide the backend's window; replies with whether it is shown now.   |
| `{"input":{"form":7,"values":{"code":"424242"}}}`                    | The answers to a form.                                                     |
| `{"dismissed":{"form":7}}`                                           | Nobody answered that form.                                                 |
| `{"cancel":{}}`                                                      | Clean up and exit.                                                         |

The backend writes:

| Line                                                      | Meaning                                                                                            |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `{"hello":{"protocol":1,"empty_is_authoritative":false}}` | The answer to `start`.                                                                             |
| `{"reply":{"id":1,"ok":[…]}}`                             | The answer to a call; `"error":{"kind":"not_found","message":"…"}` instead of `ok` when it failed. |
| `{"progress":{"fraction":0.5}}`                           | How far a load has got.                                                                            |
| `{"attention":"Approve the sign-in on your phone"}`       | A wait on the person somewhere nyd cannot reach; `null` withdraws it.                              |
| `{"form":{"id":7,"header":"…","fields":[…]}}`             | Ask the person; answered with `input` or `dismissed`.                                              |
| `{"fatal":"…"}`                                           | The backend cannot go on. The process is restarted on the next call.                               |

Calls carry their own `id` and may overlap; replies are matched by `id`, not
by order. The error kinds are those of `CalendarError` plus `unsupported`, for
a call the backend does not implement: `list_calendars` then reads as no
calendars, `toggle_window` as "this connection has no window".

The types live in `not_yet_done_calendar_external::protocol` and both
serialise and deserialise, so a Rust backend reads what nyd wrote instead of
transcribing it.

### Where the pieces go

- **The host resolves secrets.** `secrets:` takes the credential providers
  every other adapter uses, so a backend never runs `pass` itself and a
  locked store asks its passphrase through the front-end. Values travel in
  `start` and nowhere else — never on a command line or in an environment.
- **The `config:` block is opaque.** nyd does not read it; its shape is the
  backend's business, as a connection's `config:` already is for an
  in-process backend.
- **One process per `(command, share)`.** Connections that name the same
  program and share key talk to one process, so one sign-in serves them all.
  A process that exits is started again on the next call.
- **Attention is a status, not a bus event.** What a backend asks of the
  person shows as the connection's `AdapterStatus::Connecting` with
  `attention: true` — the tab's sign-in banner — and outranks a running load.
  No topic has to be bound in the view config for it to be seen.

## Consequences

- **The public workspace builds and runs without any setup-specific
  backend.** `microsoft` and `caldav` stay in process; everything else is a
  program on `PATH`.
- **`CalendarBackend` gains `subscribe_attention()`**, defaulting to `None`,
  so the in-process backends are
  unchanged.
- **The event DTOs (`EventDraft`, `EventPatch`, `TimeRange`, `LoadProgress`,
  …) are serde types.** They are the protocol's payload; a second set of
  wire types would drift from them.
- **A crashing backend costs one call, not the TUI.** The call fails with its
  message, and the next one starts a fresh process.
- **A backend without forms or windows is a short program.** Answering
  `hello`, `list_events` and `unsupported` for the rest is a working
  read-only calendar.
