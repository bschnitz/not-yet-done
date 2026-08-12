# Adapter-Defined Auth Mechanisms — Plan

Status: **Ph1–Ph4 implemented**; live smoke tests outstanding.
Tracking memory: `project_auth_mechanism_descriptors.md`.

## Goal

Move the knowledge "which auth mechanisms exist and which fields they
need" out of `not-yet-done-content` and into the adapters, so that:

1. an adapter can implement **any** mechanism without touching the core
   crate,
2. an adapter declares only the **input fields** it needs from the
   outside, and
3. selecting a mechanism the adapter does not support fails at **config
   parse time**, not at login time.

`content` keeps the machinery — credential providers, the orchestrator's
resolve/prompt/cache state machine, session-cache policies, and the
`NeedsCreds` prompt contract.

## Current state (what is being replaced)

`not-yet-done-content/src/auth.rs` holds a closed `enum AuthMechanism`
(`password-login`, `bearer-token`, `cookie`, `basic-auth`,
`user-api-token`) and a hard-wired required-field table inside
`AuthSpec::validate()`. Adapters can only reject a mechanism, and they do
so late: every auth bridge has an `other =>` arm in `run_login` that
returns "X adapter does not support mechanism Y" — reached only when a
login is actually attempted.

What already works and is kept: every factory calls `cfg.auth.validate()`
in `build()`, so a parse-time check exists. It just does not know the
per-adapter mechanism set.

Supported sets as of today, expressed only as `match` arms:

| Adapter    | Mechanisms                       |
| ---------- | -------------------------------- |
| Jira       | `cookie`, `basic-auth`           |
| Kimai      | `user-api-token`, `bearer-token` |
| Confluence | `cookie`                         |
| Stoat      | `password-login`                 |
| Taiga      | `password-login`                 |

## Scope

- `content`: descriptor types, `AuthSpec.mechanism` as a string id,
  `validate_against`, factory hook
- 5 adapter crates: static descriptor tables, `run_login` matching on
  `&str`
- CLI: `config build` wizard sources its choices from the descriptor;
  `config auth <type>` lists them (see Ph2)
- New credential provider `form-command` (script-driven form)
- Docs: `content-adapter-spec.md`, README auth section, example YAMLs,
  smoke tests

## Out of scope (deliberate)

- Automatic re-login on a 401 **during** a session. A real gap, but
  independent of this rebuild — tracked separately.
- Adapters fetching cookies from a browser themselves. Cookie
  acquisition stays a script's job; the adapter stays browser-agnostic.
- Config migration. Existing YAML keeps parsing unchanged (see D3).

## Design decisions

### D1 — Descriptor shape

```rust
pub struct MechanismSpec {
    /// Wire id used in YAML (`mechanism: cookie`). Kebab-case.
    pub id: &'static str,
    /// Display name for the config wizard.
    pub label: &'static str,
    /// One line explaining when to pick this mechanism.
    pub doc: &'static str,
    pub fields: &'static [AuthFieldSpec],
}

pub struct AuthFieldSpec {
    pub name: &'static str,
    /// Prompt label. Replaces the title-casing guess in `CredentialBinding`.
    pub label: &'static str,
    /// Whether input is masked. Replaces the field-name heuristic
    /// (`password|token|secret|cookie|api_key`).
    pub masked: bool,
    /// `false` lets a binding be omitted entirely — see D5.
    pub required: bool,
}
```

The binding-level `label:` / `masked:` stay as per-instance overrides;
the descriptor supplies the default the adapter considers correct rather
than one derived from the field's spelling.

### D2 — Static tables, for now

`&'static [MechanismSpec]`. Every mechanism known today is a compile-time
constant of its adapter. A future plugin adapter talking IPC would need
its mechanisms at runtime, which turns the type into
`Vec<MechanismSpec>` — a mechanical change, deliberately deferred rather
than paid for up front.

### D3 — `mechanism` becomes a string id

`AuthSpec.mechanism: String` (or a `MechanismId` newtype) instead of the
enum. Because the enum already serialises kebab-case, every existing
config file parses unchanged — **no migration step, no config rewrite**.

The cost: `fieldsmith` can no longer reflect the valid choices out of the
type, which is precisely why the wizard has to ask the factory (Phase 2).

### D4 — The factory publishes the set

```rust
trait TypedAdapterFactory {
    fn auth_mechanisms(&self) -> &'static [MechanismSpec] { &[] }
}
```

Defaulting to empty means "this adapter has no auth" — true for the local
adapters (tasks, trackings, projects, sqlite) and keeps them untouched.
`TypedFactory` forwards it to the object-safe `AdapterFactory`, next to
the existing `config_schema()` forwarding, so the wizard and
`nyd adapter … help` can read it **without a live instance**. The
anonymisation decorator forwards it like it already forwards
`config_schema`.

### D5 — Validation moves to the descriptor

`AuthSpec::validate()` → `AuthSpec::validate_against(&[MechanismSpec])`:

- unknown mechanism id → error naming the ids **this** adapter supports
- missing binding for a `required: true` field → error
- binding for a field the mechanism does not declare → error
- duplicate binding → error
- missing binding for a `required: false` field → accepted, field absent
  from the resolved credential map

Optional fields are new capability: today the check demands exact
coverage, which makes an optional `domain`, `otp` or `realm` impossible
to express. Phase 1 introduces the flag; Phase 3 is the first real user.

`AuthOrchestrator::from_spec` stops validating — it has no descriptor.
This is the one check that disappears from `content`; it is replaced by
the factory-level call that provably runs before any orchestrator is
built.

### D6 — `run_login` keeps a fallback arm

Bridges match on `spec.mechanism.as_str()`. The `other =>` arm survives
as a defensive assertion rather than a user-facing path: after D5 the
config can no longer reach it.

### D7 — Script-driven forms are a _provider_, not a mechanism

The original request — a custom script that builds and validates a form
in the TUI — is deliberately **not** modelled as a mechanism. A new
provider `form-command` sits beside `command`:

1. the script is invoked with a discovery flag and prints a form spec
   (fields, labels, masked, validation rules) to stdout,
2. the frontend renders it — the TUI via the existing `FormFieldSpec`
   form driver, the CLI via the terminal prompt in
   `not-yet-done-cli/src/adapter_connect.rs`,
3. the answers are handed back to the script on stdin, and it prints the
   final credential value.

This keeps the mechanism's field set fixed (goal 2 stays intact) while
the script gets its form. For the cookie case it is exactly the right
shape: the script runs whatever browser flow it likes and the adapter
only ever sees the finished `Cookie:` header line.

## Phases

### Ph1 — Descriptors and validation

No user-visible behaviour change; existing configs keep working.

- `content/src/auth.rs`: add `MechanismSpec` / `AuthFieldSpec`, remove
  `AuthMechanism`, `mechanism: String`, `validate_against`.
- `content/src/lib.rs`: `auth_mechanisms()` on both factory traits,
  forwarding in `TypedFactory` and in the anonymisation decorator.
- Per adapter: static table, `build()` calls
  `validate_against(self.auth_mechanisms())`, `run_login` matches on
  `&str`.
- Tests: the `assert_eq!(cfg.auth.mechanism, AuthMechanism::Cookie)`
  assertions in each adapter's `config.rs` become string comparisons; add
  one test per adapter that an unsupported mechanism is rejected **at
  build time** with the supported ids in the message.

### Ph2 — Config wizard

- `config build` asks the factory for `auth_mechanisms()`, offers
  `label` + `doc`,
- generates the full `bindings:` block from `fields` instead of an empty
  placeholder, asking per field which provider to use,
- omits `required: false` fields unless the user asks for them,
- a listing of mechanisms and fields from the same source, so it cannot
  drift.

The listing landed as **`nyd config auth <type>`**, not as a section of
`nyd adapter <type> help` as first sketched: `adapter` addresses a
configured _instance_ and connects before it prints anything, which is
exactly what a user reading up on authentication cannot do yet. The
`config` commands are factory-by-type and connect to nothing, so the
auth listing belongs beside `config template` / `config build` — which
also means `config template` can point at it (`# tip: nyd config auth
<type>`) where the static skeleton can only show `mechanism: <string>`.

### Ph3 — `form-command` provider (superseded)

**Historical.** `form-command` was removed again; `auth.script` plus the
`script-result` provider took its place — see `plan-credential-script.md`.
The paragraph below records what Ph3 shipped, not what the code does today.

Implements D7. The script is called twice: with `form_flag` appended
(default `--nyd-auth-form`) it prints its form as JSON on stdout, without
it the answers arrive as a JSON object on stdin and the credential value
comes back on stdout. `content/src/auth/form_command.rs` owns that
protocol; the orchestrator treats the provider like a prompt
(`CredentialProvider::needs_frontend`), publishes the script's fields
namespaced as `<field>.<script-field>` in the one `NeedsCreds` form, and
turns the reply back into the single value the mechanism declared.

Two deviations from the sketch, both narrowing:

- **No separate TUI rendering.** The plan named the `FormFieldSpec` form
  driver, but a script's form is a list of labelled, optionally masked
  inputs — exactly `AdapterCredsPopup`'s `AuthField` list. Routing it
  through the existing popup means the CLI (`adapter_connect.rs`), the
  TUI and the no-TTY error path all work unchanged, including the "no
  terminal to ask on" message. `AuthField` gained one field, `optional`,
  so a script can declare an input it can do without; a field the config
  binds is never optional.
- **No validation rules in the form spec.** A regex in the spec would put
  validation semantics into the core crate and both frontends for a rule
  the script has to enforce anyway (its answers may come from a
  non-interactive provider tomorrow). The script rejects bad input by
  exiting non-zero, and its stderr becomes the credential error.

Also deliberately without a retry count: the second call carries the
user's answers, and repeating it unasked replays a failed login.

### Ph4 — Docs

- `content-adapter-spec.md` — new section "Authentication — the adapter
  publishes its mechanisms" beside the other contract sections: the
  factory hook, the descriptor types, and the three obligations that
  follow (validate in `build()`, match on `&str` with a defensive
  fallback arm, declare labels and masking instead of guessing them).
- README — a user-facing `Authentication` section under _Configuration_:
  the `auth:` block, the provider table including `form-command`, the
  two-call script protocol with a runnable example, and the
  session-cache policies. The Stoat and Confluence sections now point at
  it instead of re-explaining bindings.
- Example YAMLs — every one of the five points at `nyd config auth
<type>` rather than listing mechanisms in prose. `jira-adapter.yaml`
  was rewritten: it still documented the pre-orchestrator fields
  (`email`, `token`, `session_id`) that `JiraConfig` has not had for a
  long time, so it could not have parsed.
- Drift removal in the code doc-comments: kimai's `auth` field
  hand-repeated both mechanism ids, stoat's module doc named fields
  (`email` + `password`) the mechanism does not declare, and
  confluence's still said auth was "parsed but not yet wired". All four
  adapter configs now name `MECHANISMS` and `nyd config auth <type>` as
  the source instead of restating it.

Smoke tests for the listing and for `form-command` landed with Ph2/Ph3;
running them against live instances is what remains.

## Risks

- **Loss of exhaustiveness.** Matching on `&str` cannot be checked by the
  compiler. Mitigated by D5 (config can't reach the fallback) and by the
  per-adapter rejection test in Ph1.
- **Wizard regression window.** Between Ph1 and Ph2 the wizard has no
  choice list for `mechanism` — it degrades to a free-text field. Ph2
  should follow Ph1 closely, or Ph1 ships a stop-gap that reads the
  descriptor without the full wizard rework. _Closed by Ph2: the wizard
  splits the `auth` field out of the reflected schema and fills it from
  the descriptors, so `mechanism` is a menu again._
