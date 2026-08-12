# fieldsmith

Reflect, template, and interactively build config structs from a single derive.

Rust has no runtime reflection: a `#[derive(Deserialize)]` struct can _read_ a
config but cannot _describe_ itself — you cannot ask it for its field names,
types, or docs. `fieldsmith` fills that gap at compile time.

`#[derive(Buildable)]` projects a type — a struct's fields (types, docs,
defaults, serde renames) or an enum's variants — into a runtime `TypeSchema`.
Every frontend is a consumer of that one schema, so the type definition stays
the **single source of truth** and no view can drift from it:

- **`yaml_template(&schema)`** renders a fillable, commented YAML skeleton.
- a generated **`<Name>Builder`** gives typed setters and a checked `build()`.
- with the **`stdin`** feature, **`build_stdin::<T>()`** drives an interactive,
  recursive builder over the same schema.

`#[derive(Buildable)]` is meant to sit alongside `#[derive(Deserialize)]`: it
reads the same `#[serde(...)]` attributes (`rename`, `rename_all`, `default`,
`tag`) so the emitted keys match what Deserialize expects. The interactive
driver even assembles a `serde_yaml::Value` and hands it to your type's own
`Deserialize`, so deserialization remains the sole authority on how answers
become a value.

## Install

```toml
[dependencies]
fieldsmith = "0.1"

# For the interactive stdin builder:
# fieldsmith = { version = "0.1", features = ["stdin"] }
```

## Templates and the builder

```rust
use fieldsmith::{Buildable, yaml_template};
use serde::Deserialize;

/// Jira adapter configuration.
#[derive(Buildable, Deserialize)]
struct JiraConfig {
    /// Base URL of your Jira instance.
    #[builder(default = "https://your-jira.example.com")]
    url: String,
    /// Optional display name for this instance.
    #[builder(default = "My Jira")]
    name: Option<String>,
    /// Trust self-signed TLS certificates.
    #[serde(default)]
    #[builder(default = false)]
    accept_invalid_certs: bool,
}

// A commented, fillable YAML skeleton (valid YAML as-is):
println!("{}", yaml_template(&JiraConfig::schema()));

// A typed builder with defaults applied and required fields checked:
let cfg = JiraConfigBuilder::new()
    .url("https://jira.acme.test")
    .build()
    .unwrap();
```

## Interactive builder (`stdin` feature)

Nested structs, `Vec<_>` lists, `Option<_>` fields, and both externally- and
internally-tagged enums are walked recursively — one prompt per leaf, a menu per
enum, a confirm-loop per list:

```rust
use fieldsmith::{Buildable, build_stdin};
use serde::Deserialize;

#[derive(Buildable, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Cred {
    Env { var: String },
    Command { script: String, timeout_secs: Option<u64> },
}

#[derive(Buildable, Deserialize, Debug)]
struct AuthConfig {
    cred: Cred,
    #[serde(default)]
    headers: Vec<String>,
}

let cfg: AuthConfig = build_stdin().unwrap();
```

Prompting is abstracted behind the `Prompter` trait: `DialoguerPrompter` drives
a real terminal, while `ScriptedPrompter` replays canned answers so the walk is
unit-testable without a TTY.

## Attributes

- `#[builder(default = X)]` — a real default, parsed via the field's `FromStr`.
  Doubles as the template placeholder and the prompt default.
- `#[builder(doc = "…")]` — override the doc shown for a field.
- `#[builder(leaf)]` — force a non-primitive `FromStr` type to be treated as a
  single-line scalar instead of recursing into it.

Supported serde attributes: `rename`, `rename_all`, `default` (both forms),
`tag` (internally-tagged enums). Adjacently-tagged enums (`content`) and generic
types are not supported yet.

## Examples

```sh
cargo run -p fieldsmith --example template
cargo run -p fieldsmith --example interactive --features stdin
```

## License

Licensed under either of MIT or Apache-2.0 at your option.
