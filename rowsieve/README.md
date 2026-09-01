# rowsieve

A small YAML filter language over rows: an AST, a parser, and an in-memory
evaluator over anything that can answer _"what is the value of field X"_.

`rowsieve` is the **language** half of a filter system. It knows nothing about
where rows come from — no entity, no schema, no database — so the same filter
can be written once and applied to a database table, an API result and a list
held in memory without three implementations disagreeing about what it means.

A host teaches it about its own data by implementing one method:

```rust
use rowsieve::{Field, FilterExpr, RowFields, matches};

struct Task { title: String, priority: i64 }

impl RowFields for Task {
    fn field(&self, name: &str) -> Field<'_> {
        match name {
            "title" => Field::Text(self.title.as_str().into()),
            "priority" => Field::Number(self.priority as f64),
            _ => Field::Null,
        }
    }
}

let expr: FilterExpr = serde_yaml::from_str(
    "and:\n  - [title, has, deploy]\n  - [priority, '>=', 3]\n",
)?;

assert!(matches(&expr, &Task { title: "Deploy the bar".into(), priority: 5 }));
```

## Install

```toml
[dependencies]
rowsieve = "0.1"

# To write "end of next week" on the right-hand side of a comparison:
# rowsieve = { version = "0.1", features = ["natural-dates"] }
```

## The language

An expression is a combinator or a leaf:

```yaml
and: # every branch matches; `or` and `not` likewise
  - [status, "=", open] # [field, operator, value]
  - [assignee, is_not_null] # two elements for an operator that takes no value
  - not: [title, matches, "^WIP"] # a regex, case-sensitive
```

| Operator                | Compares                                              |
| ----------------------- | ----------------------------------------------------- |
| `=` `!=`                | Equality. Text compares case-insensitively.           |
| `>` `>=` `<` `<=`       | Order: numbers, instants, text.                       |
| `like` `not_like`       | SQL wildcards: `%` for any run, `_` for one.          |
| `has`                   | Substring — `like '%value%'` without the punctuation. |
| `in` `not_in`           | Membership in a list.                                 |
| `is_null` `is_not_null` | Whether the row has a value at all.                   |
| `matches`               | A regular expression, against the raw value.          |

Three rules are worth knowing before writing a filter:

- **Text compares case-insensitively.** These filters are a search feature, not
  an exact-match store. `matches` is the exception — a regex is a
  classification rather than a search, and silent case-folding in a
  classification is a surprise. Write `(?i)` for the other behaviour.
- **Any comparison against a null is false**, `!=` included. A missing value is
  not "different from X", it is unknown — the three-valued logic SQL applies.
  `is_null` / `is_not_null` are the way to ask.
- **A field name is never guessed.** An unknown one reads as null and matches
  nothing; `eval::validate_fields` turns that into a load-time error, with the
  known names in the message.
- **A dotted name stays whole.** `tags.sender` reaches `RowFields::field` as
  `"tags.sender"`. A qualifier is a join alias only to a host that has joins;
  a row in memory has none, so the dot is part of the path into it.

## Extending it

A predicate this crate cannot evaluate — "is a descendant of X", "is assigned
to me" — is written `[name, argument]` and parses to `FilterExpr::Custom`.
`rowsieve` carries it through the tree untouched and evaluates it to `false`;
the host recognises it by name and resolves it however it must, usually by
rewriting that branch before evaluation.

```rust
expr.validate_custom(&["in_tree", "assigned_to_me"])?;
```

Call that once where the filter is loaded. Parsing cannot do it — `[foo, bar]`
is well-formed for a host that has a predicate called `foo` — and without it a
misspelt name is a branch that quietly matches nothing.

## What is deliberately not here

**Translating an expression into SQL.** That needs a schema, a dialect and a
query builder, and every host has different ones. The AST is public precisely
so a host can walk it and build its own `WHERE` clause.

**A date grammar.** The language has no date type: a resolved date is a string
literal that the evaluator compares as an instant. With the `natural-dates`
feature, `dates::resolve_dates` pre-processes the YAML and replaces
right-hand-side phrases like `end of next week` with RFC 3339 timestamps —
before parsing, so every document that embeds a filter gets the same treatment
without re-deriving the walk.

## Fetching before filtering

A host that pulls rows from somewhere expensive usually has to ask for a
_window_ first. `extract_date_bounds(&expr, &["starts_at", "ends_at"])` derives
the tightest `(min, max)` a filter can possibly match. The answer is always a
superset — a hint for narrowing a fetch, never a substitute for evaluating the
filter afterwards.

## License

MIT OR Apache-2.0
