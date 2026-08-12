# fieldsmith-derive

Derive macro for [`fieldsmith`](https://crates.io/crates/fieldsmith). This crate
provides `#[derive(Buildable)]` and is re-exported by `fieldsmith` — depend on
`fieldsmith` directly rather than on this crate.

`#[derive(Buildable)]` reads a type's fields/variants, doc comments,
`#[serde(...)]` renames/defaults/tag and its own `#[builder(...)]` attributes,
then emits an `impl fieldsmith::Buildable` returning a runtime schema (and, for a
struct, a typed `<Name>Builder`).

See the `fieldsmith` crate for documentation and examples.

## License

Licensed under either of MIT or Apache-2.0 at your option.
