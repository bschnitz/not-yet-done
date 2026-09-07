# 0009 — Decorator traits for the content layer, and action aliases

- **Status:** accepted, implemented
- **Date:** 2026-09-07
- **Affects:** `not-yet-done-content` — `decorate.rs`, `aliasing.rs`,
  `anonymize.rs`; `not-yet-done-host` — `AdapterInstance`,
  `decorate_instance`; the TUI's tab construction

## Context

A tasks binding was to start a tracking that stops only the trackings in the
same _group_ of the task tree (`^/Work` vs. `^/Other/Group`). The mechanism
already existed in half: a binding can hand an action named arguments
(`args:`), so `toggle-tracking` could take `group_paths`. What did not exist
was a place to say it **once**. The same set of paths would have been repeated
on every binding of the action in `tasks.yaml` and `trackings.yaml`, and a
global binding on the adapter block would have suggested that the action is
global when it is a per-node action like any other.

Independently, the content layer had three hand-written decorators
(anonymization, scripts, custom columns), each a full copy of the
`ContentAdapter` forwarders. `ContentAdapter` has grown to forty-nine methods,
and every method added after a decorator was written silently fell through to
the trait's default behind it: the anonymizing wrapper had lost the credential
prompts, the reminders, the asset download and the extended-query store; the
scripts and custom-columns wrappers had lost `subscribe_status_for`, which is
why a per-subtab status degrades to the global one behind them. Nothing
reported it, because a default impl is a valid impl.

## Options

### Where the group paths live

1. **On every binding (`args:`).** Works today, repeats the policy per
   binding; a change means editing every file that binds the action.
2. **A config key on the adapter (`group_paths:` next to
   `tracking_marker`).** One place, but a second channel into the action
   beside its arguments, and the action has to know about instance config
   that other callers (CLI, hooks) never see.
3. **An alias on the adapter block (chosen).** The instance declares
   `my-toggle-tracking: { action: toggle-tracking, args: {…} }`. The alias is
   a real action to every frontend and is bound like one; the target keeps a
   single input channel (its arguments), and the per-instance policy is
   written once. The alias is not global: it exists wherever its target
   exists.

### How the alias reaches the adapter

1. **Expand in each frontend.** The TUI, the CLI and the hook runner would
   each map alias names before invoking — three copies of the lookup, and an
   adapter listing that does not know the alias exists.
2. **A decorator in the content layer (chosen).** `AliasingAdapter` wraps the
   built adapter; it expands the listings (`actions_for_type`,
   `collection_actions`), merges the alias defaults under the invocation's
   arguments on every path that carries a context (`invoke_action`, `prepare`,
   `execute`), rewrites the name on the paths that only take a name
   (`picker_options`, `form_prep`) and refuses an alias that carries
   arguments on the paths that have nowhere to put them (`execute_addressed`,
   `collection_prepare`, `execute_collection`). Frontends see one action
   list and change nothing.

### Where the decorator is applied

The type-level decorators live in `host::factories()` and wrap every instance
of a type alike. An alias is per instance, so it needs the `AdapterInstance`,
which the factory never sees. Rather than threading the instance through the
factory trait, one host function `decorate_instance(adapter, &instance)` runs
after `create` at each of the two build sites (host `resolve_adapter_with`,
TUI tab construction). It is the second chokepoint next to `factories()`; a
future per-instance decorator goes into the same function.

### Stopping the forwarder drift

1. **Keep copying, review harder.** The seven dead forwards in the
   anonymizer had survived several reviews.
2. **Decorator traits with blanket impls (chosen).** `AdapterDecorator`
   requires only `inner()` and provides a default forwarder per method;
   `impl<T: AdapterDecorator> ContentAdapter for T` turns any decorator into
   an adapter. A decorator overrides the handful of methods it changes and
   forwards the rest by construction. Coherence allows the blanket impl
   because every concrete adapter implements `ContentAdapter` directly and
   nothing implements both traits. A test parses the trait source and the
   forwarder source and fails on a method without a forwarder, so the traits
   themselves cannot drift from `ContentAdapter`. `NodeDecorator` does the
   same for `Node`.

## Decision

`content::decorate` defines the two traits; the anonymizer is the first
decorator migrated to them. `content::aliasing` defines `AliasSpec`,
`AliasTable` (validated at load: non-empty name and target, no
self-reference, no duplicate, no alias-to-alias chain) and the
`AliasingAdapter`/`AliasingNode` pair. `AdapterInstance` gains
`aliases: BTreeMap<String, AliasSpec>`; `host::decorate_instance` builds the
table and wraps the adapter, or leaves it untouched when the table is empty.

## Consequences

- A per-instance action policy is written once, on the adapter block, and is
  visible in every listing a frontend or the CLI produces.
- Alias arguments are defaults: an invocation's own arguments win key by key,
  and the merged set is validated against the target's declared parameters.
  A real action of the alias's name wins, so an adapter update never breaks
  a view file.
- The scripts and custom-columns decorators are migrated in the same step.
  Each had five dead forwards (`refresh`, `subscribe_status_for`, the
  extended-query store, the custom query context, the query language); since
  those two wrap every adapter, a per-subtab status had been degrading to the
  global one behind them.
- One hop only: an alias of an alias is a load error. Chains would make the
  listing order and the argument precedence a matter of declaration order.
