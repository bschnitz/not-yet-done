//! `nyd config generate` — scaffold a TUI view config from an adapter's
//! protocol description.
//!
//! This is the config-generation twin of the `adapter … help` documentation:
//! both walk the content tree level-by-level over the public protocol (child
//! types, type-level actions, sortable/described columns) with no live
//! connection. `help` renders that as Markdown docs; `generate` renders it as a
//! `views/*.yaml` skeleton — the heavy lifting lives in
//! [`not_yet_done_content::scaffold`]; this module only sources the adapter,
//! drives the interactive level-by-level view/child selection, and prints.
//!
//! Two ways to name the adapter:
//!
//! * **Existing instance** — `nyd config generate <instance>`: resolve a
//!   configured instance (its `adapter:` block is reproduced in the output).
//! * **Bootstrap** — `nyd config generate --type <t> [--config <f> | --inline
//!   <yaml> | --inline -]`: construct a brand-new adapter of a given type, for
//!   which no view file exists yet.
//!
//! Selection is interactive by default (prompts on **stderr**, choices read
//! from stdin); `--all` takes every view + child non-interactively. The
//! generated YAML always goes to **stdout** — the TUI never opens — so
//! `nyd config generate tasks --all > views/tasks.yaml` just works.

use std::collections::HashSet;
use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, Result, anyhow};
use not_yet_done_content::{
    ContentAdapter, NodeType, ScaffoldFileMeta, ScaffoldSelection, child_types_of_type,
    generate_scaffold,
};

/// Parsed `config generate` options.
struct Opts {
    /// Mode 1: an existing configured instance to regenerate.
    instance: Option<String>,
    /// Mode 2 (bootstrap): construct an adapter of this type.
    adapter_type: Option<String>,
    /// `--config <path>`: adapter config file (bootstrap; also echoed into the
    /// generated `adapter.config:`).
    config: Option<String>,
    /// `--inline <yaml>` (or `--inline -` to read stdin): inline adapter config.
    inline: Option<String>,
    /// `--all`: include every view + child, no prompts.
    all: bool,
    /// `--depth N`: cap descent (0 = top-level views only).
    depth: Option<usize>,
    /// `--name <s>`: override the generated `tab.name`.
    name: Option<String>,
    /// `--order N`: the generated `tab.order` (default 1).
    order: i32,
}

/// Entry point for `nyd config generate …` (`args[2]` is `generate`/`gen`).
pub fn run(args: &[String]) -> Result<()> {
    let opts = parse(&args[3..])?;
    let rt = tokio::runtime::Runtime::new().context("creating tokio runtime")?;
    rt.block_on(run_async(opts))
}

fn parse(rest: &[String]) -> Result<Opts> {
    let mut opts = Opts {
        instance: None,
        adapter_type: None,
        config: None,
        inline: None,
        all: false,
        depth: None,
        name: None,
        order: 1,
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i].as_str();
        let mut take_val = |name: &str| -> Result<String> {
            i += 1;
            rest.get(i)
                .cloned()
                .ok_or_else(|| anyhow!("{name} needs a value"))
        };
        match a {
            "--type" | "-t" => opts.adapter_type = Some(take_val("--type")?),
            "--config" | "-c" => opts.config = Some(take_val("--config")?),
            "--inline" => opts.inline = Some(take_val("--inline")?),
            "--name" => opts.name = Some(take_val("--name")?),
            "--order" => {
                opts.order = take_val("--order")?
                    .parse()
                    .context("--order must be an integer")?
            }
            "--depth" | "-d" => {
                opts.depth = Some(
                    take_val("--depth")?
                        .parse()
                        .context("--depth must be a non-negative integer")?,
                )
            }
            "--all" | "-a" => opts.all = true,
            other if other.starts_with('-') => return Err(anyhow!("unknown flag '{other}'")),
            _ => {
                if opts.instance.is_some() {
                    return Err(anyhow!("unexpected extra argument '{a}'"));
                }
                // Generation always starts at the adapter root; a type path is
                // reduced to its instance segment.
                opts.instance = Some(a.split(':').next().unwrap_or(a).to_string());
            }
        }
        i += 1;
    }
    if opts.instance.is_none() && opts.adapter_type.is_none() {
        return Err(anyhow!(
            "name an instance (`nyd config generate <instance>`) or bootstrap one (`--type <t> [--config <f>]`)"
        ));
    }
    if opts.instance.is_some() && opts.adapter_type.is_some() {
        return Err(anyhow!("give either an <instance> or `--type`, not both"));
    }
    Ok(opts)
}

async fn run_async(opts: Opts) -> Result<()> {
    let ctx = not_yet_done_host::host_context();
    let (adapter, meta) = build_adapter_and_meta(&opts, &ctx)?;

    // Resolve the selection: full (`--all`) or an interactive level-by-level
    // pick. Interactive needs a TTY to read from.
    let selection = if opts.all {
        let mut s = ScaffoldSelection::all();
        if let Some(d) = opts.depth {
            s = s.with_max_depth(d);
        }
        s
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(anyhow!(
                "interactive selection needs a terminal; pass `--all` for a full non-interactive scaffold"
            ));
        }
        let included = prompt_selection(adapter.as_ref(), opts.depth).await?;
        let mut s = ScaffoldSelection::with_types(included);
        if let Some(d) = opts.depth {
            s = s.with_max_depth(d);
        }
        s
    };

    let yaml = generate_scaffold(adapter.as_ref(), &meta, &selection).await?;
    print!("{yaml}");
    // Guidance to stderr so stdout stays a clean, pipeable config.
    eprintln!();
    eprintln!(
        "# scaffold written to stdout — review, then save under {}/",
        not_yet_done_host::views_dir().display()
    );
    Ok(())
}

/// Build the adapter to introspect plus the `tab:`/`adapter:` header facts,
/// from either an existing instance or a bootstrap `--type`.
fn build_adapter_and_meta(
    opts: &Opts,
    ctx: &not_yet_done_content::HostContext,
) -> Result<(Box<dyn ContentAdapter>, ScaffoldFileMeta)> {
    if let Some(instance) = &opts.instance {
        // Mode 1: reproduce the discovered instance's adapter block verbatim.
        let discovered = not_yet_done_host::discover_instances()
            .into_iter()
            .find(|d| d.instance_id() == instance)
            .ok_or_else(|| anyhow!("no configured instance '{instance}'"))?;
        let adapter = not_yet_done_host::resolve_adapter(instance, ctx)?;
        let inst = &discovered.adapter;
        let meta = ScaffoldFileMeta {
            tab_name: opts.name.clone().unwrap_or_else(|| capitalize(instance)),
            order: opts.order,
            adapter_type: inst.adapter_type.clone(),
            adapter_id: inst.id.clone(),
            config: inst.config.clone(),
            config_inline: inst.config_inline.clone(),
            manual_connect: inst.manual_connect,
        };
        Ok((adapter, meta))
    } else {
        // Mode 2: bootstrap a fresh adapter from `--type` + a config source.
        let atype = opts.adapter_type.as_ref().expect("checked in parse");
        let factories = not_yet_done_host::factories();
        let factory = factories.get(atype).ok_or_else(|| {
            let known: Vec<&str> = factories.keys().map(String::as_str).collect();
            anyhow!("no adapter type '{atype}' (known: {})", known.join(", "))
        })?;
        let (config_string, meta_config, meta_inline) = resolve_bootstrap_config(opts)?;
        let adapter = factory
            .create(atype, &config_string, ctx)
            .map_err(|e| anyhow!("creating adapter '{atype}': {e} (supply --config <file>?)"))?;
        let meta = ScaffoldFileMeta {
            tab_name: opts.name.clone().unwrap_or_else(|| capitalize(atype)),
            order: opts.order,
            adapter_type: atype.clone(),
            adapter_id: None,
            config: meta_config,
            config_inline: meta_inline,
            // A brand-new instance follows the field's default: wait for an
            // explicit reload rather than connect (and possibly ask for
            // credentials) the first time the TUI starts.
            manual_connect: true,
        };
        Ok((adapter, meta))
    }
}

/// For bootstrap mode, produce `(config_string_for_construction, meta.config,
/// meta.config_inline)`. `--inline -` reads stdin; `--config <path>` is read for
/// construction but echoed as a path; with neither, an empty `{}` is used (works
/// for config-less adapters like tasks).
fn resolve_bootstrap_config(opts: &Opts) -> Result<(String, Option<String>, Option<String>)> {
    if let Some(inline) = &opts.inline {
        let s = if inline == "-" {
            read_stdin().context("reading inline config from stdin")?
        } else {
            inline.clone()
        };
        return Ok((s.clone(), None, Some(s)));
    }
    if let Some(path) = &opts.config {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading adapter config '{path}'"))?;
        return Ok((s, Some(path.clone()), None));
    }
    eprintln!("# note: no --config/--inline given; constructing with empty config \"{{}}\"");
    Ok(("{}".to_string(), None, None))
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
    Ok(s)
}

/// Interactively walk the type tree from the root, asking at each level which
/// child types to include, and return the chosen type ids. Prompts go to
/// stderr; choices are read from stdin.
async fn prompt_selection(
    adapter: &dyn ContentAdapter,
    max_depth: Option<usize>,
) -> Result<HashSet<String>> {
    let root = adapter.root().await?;
    let root_nt = root.node_type().clone();
    let mut included = HashSet::new();
    let mut ancestors = vec![root_nt.type_id.clone()];
    eprintln!("Selecting views/children for '{}'.", root_nt.type_id);
    eprintln!("At each level: Enter=all, `n`=none, or a list like `1,3`.");
    walk_prompt(
        adapter,
        &root_nt,
        &mut ancestors,
        &mut included,
        0,
        max_depth,
        true,
    )?;
    Ok(included)
}

#[allow(clippy::too_many_arguments)]
fn walk_prompt(
    adapter: &dyn ContentAdapter,
    parent_nt: &NodeType,
    ancestors: &mut Vec<String>,
    included: &mut HashSet<String>,
    depth: usize,
    max_depth: Option<usize>,
    is_view_level: bool,
) -> Result<()> {
    if max_depth.map(|m| depth > m).unwrap_or(false) {
        return Ok(());
    }
    let kids = child_types_of_type(adapter, parent_nt);
    if kids.is_empty() {
        return Ok(());
    }

    let noun = if is_view_level { "views" } else { "children" };
    eprintln!();
    eprintln!("{} under {}:", capitalize(noun), parent_nt.type_id);
    for (idx, k) in kids.iter().enumerate() {
        let recursive = ancestors.iter().any(|a| a == &k.type_id);
        eprintln!(
            "  {}) {} ({}){}",
            idx + 1,
            k.display_name,
            k.type_id,
            if recursive { "  [recursive]" } else { "" }
        );
    }
    eprint!("select [{noun}]> ");
    std::io::stderr().flush().ok();
    let chosen = read_choice(kids.len())?;

    for idx in chosen {
        let kid = &kids[idx];
        included.insert(kid.type_id.clone());
        // A type already on the ancestor path is recursive — included, but not
        // descended into (the scaffold emits `recursive: true`).
        if ancestors.iter().any(|a| a == &kid.type_id) {
            continue;
        }
        ancestors.push(kid.type_id.clone());
        walk_prompt(
            adapter,
            kid,
            ancestors,
            included,
            depth + 1,
            max_depth,
            false,
        )?;
        ancestors.pop();
    }
    Ok(())
}

/// Read one selection line from stdin and resolve it to zero-based indices.
/// Empty = all, `n`/`none` = none, otherwise a comma/space list of 1-based
/// indices (out-of-range entries are rejected).
fn read_choice(count: usize) -> Result<Vec<usize>> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    parse_choice(&line, count)
}

/// Pure resolution of a selection line to zero-based indices (see [`read_choice`]).
fn parse_choice(line: &str, count: usize) -> Result<Vec<usize>> {
    let t = line.trim();
    match t.to_ascii_lowercase().as_str() {
        "" | "a" | "all" => return Ok((0..count).collect()),
        "n" | "none" => return Ok(Vec::new()),
        _ => {}
    }
    let mut out = Vec::new();
    for tok in t.split([',', ' ']).filter(|s| !s.is_empty()) {
        let n: usize = tok.parse().map_err(|_| anyhow!("not a number: '{tok}'"))?;
        if n == 0 || n > count {
            return Err(anyhow!("out of range: {n} (have 1..={count})"));
        }
        if !out.contains(&(n - 1)) {
            out.push(n - 1);
        }
    }
    Ok(out)
}

/// Capitalize the first character (ASCII), leaving the rest untouched.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_requires_a_target() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn parse_instance_positional() {
        let o = parse(&v(&["jira"])).unwrap();
        assert_eq!(o.instance.as_deref(), Some("jira"));
        assert!(!o.all);
    }

    #[test]
    fn parse_reduces_type_path_to_instance() {
        let o = parse(&v(&["jira:issue:comment"])).unwrap();
        assert_eq!(o.instance.as_deref(), Some("jira"));
    }

    #[test]
    fn parse_bootstrap_flags() {
        let o = parse(&v(&["--type", "tasks", "--all", "--depth", "1"])).unwrap();
        assert_eq!(o.adapter_type.as_deref(), Some("tasks"));
        assert!(o.all);
        assert_eq!(o.depth, Some(1));
    }

    #[test]
    fn parse_rejects_instance_and_type_together() {
        assert!(parse(&v(&["jira", "--type", "tasks"])).is_err());
    }

    #[test]
    fn choice_empty_and_all_select_everything() {
        assert_eq!(parse_choice("", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_choice("all", 3).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn choice_none_selects_nothing() {
        assert!(parse_choice("n", 3).unwrap().is_empty());
        assert!(parse_choice("none", 3).unwrap().is_empty());
    }

    #[test]
    fn choice_list_is_one_based_deduped() {
        assert_eq!(parse_choice("1, 3, 3", 3).unwrap(), vec![0, 2]);
    }

    #[test]
    fn choice_rejects_out_of_range_and_nonnumeric() {
        assert!(parse_choice("0", 3).is_err());
        assert!(parse_choice("4", 3).is_err());
        assert!(parse_choice("x", 3).is_err());
    }
}
