---
title: Release cutting
mode: manual
log_runs: true
---

Any prose before the first `##` heading is the **workflow description** — a short
overview of what this flow does and when to run it. This one cuts a release:
build, test, tag, and (optionally) announce.

## Build

```yaml meta
id: build
mode: auto
```

Compile the release binary. With `mode: auto` and a `command`, this step runs
unattended; otherwise a human runs it.

```command
cargo build --release
```

```yaml routing
exit == 0: tests-green
else: fail
```

## Tests green?

```yaml meta
id: tests-green
mode: auto
```

Run the suite. `on_success` / `on_failure` are convenience guards (equivalent to
`exit == 0` / `exit > 0`) that send a green run onward and a red run to the
recovery step instead of aborting.

```command
cargo test --release
```

```yaml routing
on_success: update-changelog
on_failure: recover
```

## Recover

```yaml meta
id: recover
mode: manual
```

A human inspects the failure and decides whether to retry the build or abandon
the release. No routing block, so on completion the flow falls through to the
next step in document order.

## Update changelog

```yaml meta
id: update-changelog
mode: ai
```

No command here — this is an **AI** step. The configured `ai_command` gets this
description as its instruction and drives the app's CLI itself:

Summarise the commits since the last tag into `CHANGELOG.md` under a new version
heading. Keep entries user-facing; drop pure refactors and chores.

## Tag the release

```yaml meta
id: tag
mode: manual
```

A deliberately manual gate — a human confirms the version and cuts the tag.
Reference material can live in a normal fenced block; non-`command` fences (and
plain `yaml`) are preserved verbatim in the description and never run:

```sh
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

```yaml routing
else: [announce, update-website]
```

## Announce on Slack

```yaml meta
id: announce
mode: auto
optional: true
```

`optional` steps may be skipped without failing the run. The previous step routes
to a `[announce, update-website]` list, so both notifications fan out and run
concurrently.

```command
notify-send "Released vX.Y.Z"
```

```yaml routing
else: end
```

## Update website

```yaml meta
id: update-website
mode: auto
optional: true
```

The second member of the fan-out. Both notifications run together, then the run
ends.

```command
echo "site rebuild triggered"
```

```yaml routing
else: end
```
