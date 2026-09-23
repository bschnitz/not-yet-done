//! `nyd script run` — a `# mode: form` script driven from the terminal.
//!
//! The rounds are the TUI's: [`FormSession`] runs one process per round, the
//! script answers with a form, a result or an error, and what changes here is
//! only who asks the questions — `fieldsmith` prompts on stderr instead of a
//! popup. A form script therefore behaves the same whether it was started from
//! a menu or from a shell, which is the point: this is how one gets written and
//! tried out without navigating the TUI first, and how one gets automated.
//!
//! Two things the CLI can do that the popup cannot:
//!
//! - **`--set key=value` answers a field from outside.** A field supplied that
//!   way is never asked about, in any round, so a form can be filled entirely
//!   from a script.
//! - **Without a terminal nothing waits.** A field that still needs a value in
//!   a cron job aborts and names the field, rather than blocking on a prompt
//!   nobody will ever see.
//!
//! What is deliberately *not* here is the `commands` dispatch: the answer of a
//! finished conversation is printed as JSON on stdout, because `reload`,
//! `notify` and friends are instructions to a front-end with panes, and this
//! one has none.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use fieldsmith::{Answer, DialoguerPrompter, Prompter};
use not_yet_done_content::script_form::FormState;
use not_yet_done_content::{FormFieldKind, FormFieldSpec, format_duration, parse_duration_minutes};
use not_yet_done_scripts::form_run::{FormSession, FormStep};
use not_yet_done_scripts::{ScriptRepo, ScriptScope, header};
use serde_json::Value;

/// `nyd script …`
pub fn run(args: &[String]) -> Result<()> {
    match args.get(2).map(String::as_str) {
        Some("run") => run_form(args),
        None | Some("help") | Some("-h") | Some("--help") => {
            print_help();
            Ok(())
        }
        Some(other) => Err(anyhow!(
            "unknown script subcommand '{other}' (use `run`, or `help`)"
        )),
    }
}

fn print_help() {
    const PROG: &str = "not-yet-done-cli";
    println!("not-yet-done — form scripts\n");
    println!("Usage:");
    println!("  {PROG} script run <script> [--set key=value …] [--payload FILE]\n");
    println!("<script> is either a path to the file, or the level it lives at:");
    println!("  <adapter>[/<node-type>…]/<name>     e.g. jira/jira:issue/log_work.py");
    println!();
    println!("Flags:");
    println!("  --set key=value   answer a field from outside; never prompted for");
    println!("  --payload FILE    JSON handed to the script as the scope payload");
    println!("                    (the selected node or rows; default: {{}})");
    println!();
    println!("Without a terminal on stdin nothing is prompted for: a field that");
    println!("still needs a value aborts and names itself.");
}

/// What `script run` was asked to do.
struct RunArgs {
    script: String,
    set: BTreeMap<String, String>,
    payload: Option<PathBuf>,
}

/// A run, or the request to be told how to ask for one. `--help` reaches
/// `parse_run` like any other flag, and answering it here rather than in
/// [`run`] means it is recognised wherever a flag may stand — `script run
/// --help` as well as `script run foo.py --help`.
enum RunRequest {
    Help,
    Run(RunArgs),
}

fn parse_run(args: &[String]) -> Result<RunRequest> {
    let mut script = None;
    let mut set = BTreeMap::new();
    let mut payload = None;
    let mut rest = args[3.min(args.len())..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--set" => {
                let pair = rest
                    .next()
                    .ok_or_else(|| anyhow!("--set wants a `key=value`"))?;
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--set wants a `key=value`, got '{pair}'"))?;
                set.insert(key.to_string(), value.to_string());
            }
            "--payload" => {
                payload = Some(PathBuf::from(
                    rest.next()
                        .ok_or_else(|| anyhow!("--payload wants a file"))?,
                ));
            }
            "--help" | "-h" => return Ok(RunRequest::Help),
            flag if flag.starts_with("--") => bail!("unknown flag '{flag}'"),
            positional if script.is_none() => script = Some(positional.to_string()),
            extra => bail!("unexpected argument '{extra}' (one script per run)"),
        }
    }
    Ok(RunRequest::Run(RunArgs {
        script: script.ok_or_else(|| anyhow!("which script? (`script run <script>`)"))?,
        set,
        payload,
    }))
}

/// `nyd script run <script> [--set …]`
fn run_form(args: &[String]) -> Result<()> {
    let args = match parse_run(args)? {
        RunRequest::Help => {
            print_help();
            return Ok(());
        }
        RunRequest::Run(args) => args,
    };
    let path = resolve_script(&args.script)?;
    check_runnable(&path)?;

    let payload = match &args.payload {
        Some(file) => serde_json::from_str(
            &std::fs::read_to_string(file)
                .with_context(|| format!("reading the payload {}", file.display()))?,
        )
        .with_context(|| format!("the payload {} is not JSON", file.display()))?,
        None => Value::Object(Default::default()),
    };

    let interactive = std::io::stdin().is_terminal();
    let mut prompter = DialoguerPrompter::new();
    let mut session = FormSession::new(&path, payload);
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut submitted = BTreeMap::new();
    let mut previous: Option<BTreeMap<String, String>> = None;

    loop {
        let round = session.next(&submitted).map_err(|e| anyhow!("{e}"))?;
        for notice in &round.notices {
            eprintln!("nyd: {notice}");
        }
        match round.step {
            FormStep::Ask(state) => {
                let input =
                    collect_round(&state, &args.set, &mut used, interactive, &mut prompter)?;
                // Every round needs a submit, and a person supplies the brake
                // — unless nothing was asked, in which case the next round
                // would be handed the very same values and say the very same
                // thing. That is a loop, not a conversation.
                if input.asked == 0 && previous.as_ref() == Some(&input.values) {
                    bail!(stuck_message(&state));
                }
                previous = Some(input.values.clone());
                submitted = input.values;
            }
            FormStep::Done(answer) => {
                report_unused(&args.set, &used);
                println!("{}", serde_json::to_string_pretty(&answer)?);
                return Ok(());
            }
            FormStep::Failed(message) => bail!("{message}"),
        }
    }
}

/// Why a conversation that cannot move was stopped, with whatever the script
/// last complained about — otherwise the user is told a form is stuck without
/// being told what it wanted.
fn stuck_message(state: &FormState) -> String {
    let mut complaints: Vec<String> = state.general_errors.clone();
    for (key, messages) in &state.field_errors {
        complaints.push(format!("{key}: {}", messages.join("; ")));
    }
    let tail = if complaints.is_empty() {
        String::new()
    } else {
        format!(": {}", complaints.join("; "))
    };
    format!(
        "the script asked for the same form again and every field is supplied \
         from outside, so a further round would change nothing{tail}"
    )
}

/// A `--set` for a field no round ever showed is a typo far more often than a
/// spare value, and saying so at the end costs nothing — saying it every round
/// would drown the prompts.
fn report_unused(set: &BTreeMap<String, String>, used: &BTreeSet<String>) {
    for key in set.keys() {
        if !used.contains(key) {
            eprintln!("nyd: --set {key}=… was never used — no round asked for that field");
        }
    }
}

// ---------------------------------------------------------------------------
// Finding the script
// ---------------------------------------------------------------------------

/// Resolve what the user named: a file on disk if there is one, else a level's
/// script — `<adapter>[/<node-type>…]/<name>`, the path scheme
/// [`ScriptScope`] owns.
///
/// A file wins over a level because that is the developing case: one edits
/// `./log_work.py` and runs it, long before it is installed anywhere.
fn resolve_script(name: &str) -> Result<PathBuf> {
    let direct = Path::new(name);
    if direct.is_file() {
        return Ok(direct.to_path_buf());
    }
    let repo = ScriptRepo::default();
    let mut parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
    let Some(file) = parts.pop() else {
        bail!("`{name}` names neither a file nor a script");
    };
    if parts.is_empty() {
        bail!(
            "`{name}` is not a file, and a script at a level needs the level too \
             (`<adapter>[/<node-type>…]/{file}`)"
        );
    }
    let scope = ScriptScope::new(parts[0], parts[1..].iter().map(|s| s.to_string()).collect());
    let path = repo.path(&scope, file);
    if path.is_file() {
        return Ok(path);
    }
    let dir = repo.dir(&scope);
    let known = repo
        .list(&scope)
        .map(|e| e.into_iter().map(|s| s.name).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    bail!(
        "no script `{file}` in {} ({})",
        dir.display(),
        if known.is_empty() {
            "the level has none".to_string()
        } else {
            format!("there is: {known}")
        }
    )
}

/// Refuse before the first round what would otherwise fail as a spawn error
/// halfway through: a file the kernel will not exec, or a script that is not a
/// form script at all and would be handed a request it never reads.
fn check_runnable(path: &Path) -> Result<()> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if !header::parse_script_mode(&source).is_form() {
        bail!(
            "{} is not a form script — it needs the header line `# mode: form`",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode();
        if mode & 0o111 == 0 {
            bail!("{} is not executable (chmod +x it)", path.display());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// One round's worth of questions
// ---------------------------------------------------------------------------

/// What one round collected.
#[derive(Debug)]
struct RoundInput {
    /// The values to submit — visible fields only, as the popup submits them.
    values: BTreeMap<String, String>,
    /// How many of them a person was actually asked about. Zero means the
    /// round ran unattended.
    asked: usize,
}

/// Ask for everything this round needs and hand back what to submit.
///
/// The value of a field is, in order: what `--set` supplied, what the round
/// carried in (a default, a prefill, what survived from before), then what the
/// user types over it. A field hidden by its `visible_when` is skipped and
/// left out of the submission — the same omission the popup makes, and the
/// script is meant to read it as one.
fn collect_round(
    state: &FormState,
    preset: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    interactive: bool,
    prompter: &mut dyn Prompter,
) -> Result<RoundInput> {
    if let Some(header) = &state.header {
        eprintln!("\n{header}");
    }
    for message in &state.general_errors {
        prompter.error(message);
    }

    // The live values every condition is evaluated against — hidden fields
    // included, because a field can be hidden and still control another.
    let mut live = state.values.clone();
    let mut presets: BTreeMap<String, String> = BTreeMap::new();
    for field in &state.fields {
        if let Some(raw) = preset.get(&field.key) {
            let value = preset_value(field, raw)?;
            live.insert(field.key.clone(), value.clone());
            presets.insert(field.key.clone(), value);
            used.insert(field.key.clone());
        }
    }

    let mut values = BTreeMap::new();
    let mut asked = 0;
    for field in &state.fields {
        let visible = match &field.visible_when {
            Some(condition) => condition.holds(live.get(&condition.field).map(String::as_str)),
            None => true,
        };
        if !visible {
            continue;
        }
        if let Some(value) = presets.get(&field.key) {
            values.insert(field.key.clone(), value.clone());
            continue;
        }
        if let Some(message) = state.field_error_text(&field.key) {
            prompter.error(&message);
        }
        let current = live.get(&field.key).map(String::as_str);
        let value = if interactive {
            asked += 1;
            ask_field(prompter, field, current)?
        } else {
            unattended_value(field, current)?
        };
        live.insert(field.key.clone(), value.clone());
        values.insert(field.key.clone(), value);
    }
    Ok(RoundInput { values, asked })
}

/// What a `--set` means for this field.
///
/// A value supplied from outside has to mean what a typed one means: a
/// duration is whole minutes whether a person wrote `1h30` at a prompt or a
/// cron line put it in a flag, and an option that is not on the list is a
/// mistake worth naming rather than a string to hand on. So every preset goes
/// through the conversion its kind calls for, and one that cannot ends the run
/// — the shell is the only place left where it could be fixed.
fn preset_value(field: &FormFieldSpec, raw: &str) -> Result<String> {
    let refuse = |complaint: String| anyhow!("`--set {}={raw}` {complaint}", field.key);
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        if field.required {
            return Err(refuse("is empty, and the field needs a value".into()));
        }
        // A toggle is never unanswered; an unset one is off.
        if matches!(field.kind, FormFieldKind::Toggle) {
            return Ok("false".to_string());
        }
        return Ok(String::new());
    }

    let one_of = |allowed: &[String], chosen: &str| -> Result<String> {
        match allowed.iter().find(|o| *o == chosen) {
            Some(option) => Ok(option.clone()),
            None => Err(refuse(format!("is not one of {}", allowed.join(", ")))),
        }
    };
    let existing_path = |chosen: &str| -> Result<String> {
        let path = PathBuf::from(expand_home(chosen.trim()));
        if !path.exists() {
            return Err(refuse(format!(
                "names {}, which does not exist",
                path.display()
            )));
        }
        Ok(path.to_string_lossy().into_owned())
    };

    Ok(match &field.kind {
        // A date stays the phrase it was written as, exactly as a typed one
        // does; resolving it is the backend's job.
        FormFieldKind::Text | FormFieldKind::DateTime { .. } | FormFieldKind::Multiline { .. } => {
            raw.to_string()
        }
        FormFieldKind::Toggle => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "on" | "1" => "true".to_string(),
            "false" | "no" | "n" | "off" | "0" => "false".to_string(),
            _ => return Err(refuse("is neither a yes nor a no".into())),
        },
        FormFieldKind::Select { allowed_values } => one_of(allowed_values, trimmed)?,
        FormFieldKind::MultiSelect { allowed_values } => split_lines(raw)
            .into_iter()
            .map(|chosen| one_of(allowed_values, chosen.trim()))
            .collect::<Result<Vec<_>>>()?
            .join("\n"),
        FormFieldKind::Number { min, max, .. } => {
            whole_number(trimmed, field, *min, *max).map_err(refuse)?
        }
        FormFieldKind::Decimal {
            min,
            max,
            precision,
        } => fractional_number(trimmed, field, *min, *max, *precision).map_err(refuse)?,
        FormFieldKind::Duration => match parse_duration_minutes(trimmed) {
            Some(minutes) => minutes.to_string(),
            None => {
                return Err(refuse(
                    "is not a span of time (try `90`, `90m`, `1h30`, `1:30`)".into(),
                ));
            }
        },
        FormFieldKind::File { multi } => {
            let paths = split_lines(raw)
                .into_iter()
                .map(existing_path)
                .collect::<Result<Vec<_>>>()?;
            if !multi && paths.len() > 1 {
                return Err(refuse("names more than one file".into()));
            }
            paths.join("\n")
        }
    })
}

/// The value of a field nobody can be asked about: whatever it already has.
/// A required field with nothing in it is the end of the run — the one case
/// the plan insists is loud, because a cron job blocking on an invisible
/// prompt is the failure this replaces.
fn unattended_value(field: &FormFieldSpec, current: Option<&str>) -> Result<String> {
    let value = current.unwrap_or_default().to_string();
    if matches!(field.kind, FormFieldKind::Toggle) && value.is_empty() {
        // A toggle is never unanswered; an unset one is off.
        return Ok("false".to_string());
    }
    if field.required && value.trim().is_empty() {
        bail!(
            "the form needs a value for `{}` ({}) and there is no terminal to ask on \
             — supply it with `--set {}=…`",
            field.key,
            field.label,
            field.key
        );
    }
    Ok(value)
}

/// Ask one field, in the gesture its kind calls for, and hand back the value
/// in the form the protocol wants it: a toggle as `true`/`false`, a duration
/// as whole minutes, a decimal with a dot, a multi-value field newline-joined.
fn ask_field(
    prompter: &mut dyn Prompter,
    field: &FormFieldSpec,
    current: Option<&str>,
) -> Result<String> {
    let label = &field.label;
    let optional = !field.required;
    match &field.kind {
        FormFieldKind::Text | FormFieldKind::DateTime { .. } => {
            // A date stays the phrase the user typed; resolving it is the
            // backend's job, exactly as in the popup.
            if field.masked {
                answered(prompter.password(label, None, optional))
            } else {
                answered(prompter.text(label, None, current, optional))
            }
        }
        FormFieldKind::Multiline { .. } => edit_in_editor(label, current.unwrap_or_default()),
        FormFieldKind::Toggle => {
            let value = answered(prompter.confirm(label, None, current == Some("true")))?;
            Ok(value.to_string())
        }
        FormFieldKind::Select { allowed_values } => {
            if allowed_values.is_empty() {
                bail!(
                    "the field `{}` is a choice with nothing to choose",
                    field.key
                );
            }
            let items: Vec<&str> = allowed_values.iter().map(String::as_str).collect();
            let default = current
                .and_then(|v| items.iter().position(|i| *i == v))
                .unwrap_or(0);
            let index = answered(prompter.select(label, None, &items, default))?;
            Ok(allowed_values
                .get(index)
                .cloned()
                .unwrap_or_else(|| allowed_values[0].clone()))
        }
        FormFieldKind::MultiSelect { allowed_values } => {
            if allowed_values.is_empty() {
                bail!(
                    "the field `{}` is a choice with nothing to choose",
                    field.key
                );
            }
            let items: Vec<&str> = allowed_values.iter().map(String::as_str).collect();
            let already: Vec<&str> = current.map(split_lines).unwrap_or_default();
            let selected: Vec<bool> = items.iter().map(|i| already.contains(i)).collect();
            let picks = answered(prompter.multi_select(label, None, &items, &selected))?;
            Ok(picks
                .into_iter()
                .filter_map(|i| allowed_values.get(i).cloned())
                .collect::<Vec<_>>()
                .join("\n"))
        }
        FormFieldKind::Number { min, max, .. } => loop {
            let raw = answered(prompter.text(label, None, current, optional))?;
            match whole_number(raw.trim(), field, *min, *max) {
                Ok(value) => return Ok(value),
                Err(complaint) => prompter.error(&complaint),
            }
        },
        FormFieldKind::Decimal {
            min,
            max,
            precision,
        } => loop {
            let raw = answered(prompter.text(label, None, current, optional))?;
            match fractional_number(raw.trim(), field, *min, *max, *precision) {
                Ok(value) => return Ok(value),
                Err(complaint) => prompter.error(&complaint),
            }
        },
        FormFieldKind::Duration => loop {
            let raw = answered(prompter.text(label, None, current, optional))?;
            let raw = raw.trim();
            if raw.is_empty() && optional {
                return Ok(String::new());
            }
            match parse_duration_minutes(raw) {
                Some(minutes) => {
                    // `1h30` is ninety minutes, and the user should see that
                    // before it is submitted — the popup shows the same line
                    // under the field.
                    eprintln!("  = {}", format_duration(minutes));
                    return Ok(minutes.to_string());
                }
                None => prompter.error("not a span of time (try `90`, `90m`, `1h30`, `1:30`)"),
            }
        },
        FormFieldKind::File { multi } => ask_files(prompter, field, *multi),
    }
}

/// One path, or several until an empty answer ends the list. Each has to exist:
/// a form that hands a script a path to nothing has only moved the failure.
fn ask_files(prompter: &mut dyn Prompter, field: &FormFieldSpec, multi: bool) -> Result<String> {
    let mut picked: Vec<String> = Vec::new();
    loop {
        let label = if multi && !picked.is_empty() {
            format!("{} (empty to finish)", field.label)
        } else if multi {
            format!("{} (one per prompt, empty to finish)", field.label)
        } else {
            field.label.clone()
        };
        let allow_empty = !field.required || !picked.is_empty() || multi;
        let raw = answered(prompter.text(&label, None, None, allow_empty))?;
        let raw = raw.trim();
        if raw.is_empty() {
            if picked.is_empty() && field.required {
                prompter.error("this field needs a file");
                continue;
            }
            return Ok(picked.join("\n"));
        }
        let path = PathBuf::from(expand_home(raw));
        if !path.exists() {
            prompter.error(&format!("{} does not exist", path.display()));
            continue;
        }
        picked.push(path.to_string_lossy().into_owned());
        if !multi {
            return Ok(picked.join("\n"));
        }
    }
}

/// `~` is the shell's, not ours — a path typed at a `dialoguer` prompt never
/// passed one.
fn expand_home(raw: &str) -> String {
    match (raw.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => raw.to_string(),
    }
}

/// `dialoguer` has no multi-line prompt worth the name, so a `multiline` field
/// is edited where multi-line text is edited. Not an `InputSpec::Editor` flow:
/// what comes back is one field's value, not a document.
fn edit_in_editor(label: &str, current: &str) -> Result<String> {
    let file = tempfile::Builder::new()
        .prefix("nyd-form-")
        .suffix(".txt")
        .tempfile()
        .context("could not make a scratch file for the editor")?;
    std::fs::write(file.path(), current)?;
    eprintln!("{label}: opening $EDITOR");
    crate::cli_config::launch_editor(file.path())?;
    Ok(std::fs::read_to_string(file.path())?
        .trim_end_matches('\n')
        .to_string())
}

/// A whole number within the field's bounds, or what to tell the user.
fn whole_number(
    raw: &str,
    field: &FormFieldSpec,
    min: Option<i64>,
    max: Option<i64>,
) -> Result<String, String> {
    if raw.is_empty() {
        return if field.required {
            Err("this field is required".to_string())
        } else {
            Ok(String::new())
        };
    }
    let value: i64 = raw.parse().map_err(|_| "not a whole number".to_string())?;
    if let Some(min) = min
        && value < min
    {
        return Err(format!("must not be below {min}"));
    }
    if let Some(max) = max
        && value > max
    {
        return Err(format!("must not be above {max}"));
    }
    Ok(value.to_string())
}

/// A fractional number within the field's bounds, canonical with a dot and cut
/// to the field's precision.
fn fractional_number(
    raw: &str,
    field: &FormFieldSpec,
    min: Option<f64>,
    max: Option<f64>,
    precision: Option<u8>,
) -> Result<String, String> {
    if raw.is_empty() {
        return if field.required {
            Err("this field is required".to_string())
        } else {
            Ok(String::new())
        };
    }
    // Only the dot, whatever separator a TUI may let its user type: the value
    // that reaches the script must not depend on who ran the form.
    let value: f64 = raw.parse().map_err(|_| "not a number".to_string())?;
    if !value.is_finite() {
        return Err("not a number".to_string());
    }
    if let Some(min) = min
        && value < min
    {
        return Err(format!("must not be below {min}"));
    }
    if let Some(max) = max
        && value > max
    {
        return Err(format!("must not be above {max}"));
    }
    Ok(match precision {
        Some(p) => format!("{value:.*}", p as usize),
        None => value.to_string(),
    })
}

/// The lines of a newline-joined multi-value field.
fn split_lines(value: &str) -> Vec<&str> {
    value.split('\n').filter(|s| !s.is_empty()).collect()
}

/// A prompt answered, or the user walking away — which ends the run without a
/// result, exactly as `Esc` closes the popup without one.
fn answered<T>(answer: Result<Answer<T>, String>) -> Result<T> {
    match answer.map_err(|e| anyhow!("the prompt failed: {e}"))? {
        Answer::Value(value) => Ok(value),
        Answer::Cancelled => Err(anyhow!("cancelled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldsmith::ScriptedPrompter;
    use not_yet_done_content::FormFieldSpec;

    fn state(fields: Vec<FormFieldSpec>, values: &[(&str, &str)]) -> FormState {
        FormState {
            header: None,
            fields,
            values: values
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            field_errors: Default::default(),
            general_errors: Vec::new(),
            script_state: Value::Null,
        }
    }

    fn preset(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn argv(rest: &[&str]) -> Vec<String> {
        let mut args = vec!["nyd".to_string(), "script".to_string(), "run".to_string()];
        args.extend(rest.iter().map(|a| (*a).to_string()));
        args
    }

    /// Asking a command how to use it must never look like misusing it.
    /// `script run --help` used to fall into the unknown-flag arm, so the one
    /// question a newcomer asks was the one answer the command refused.
    #[test]
    fn asking_run_for_help_is_not_a_mistake() {
        assert!(matches!(
            parse_run(&argv(&["--help"])).unwrap(),
            RunRequest::Help
        ));
        assert!(matches!(
            parse_run(&argv(&["-h"])).unwrap(),
            RunRequest::Help
        ));
        // Also behind a script, where a reader reaches for it mid-command.
        assert!(matches!(
            parse_run(&argv(&["log_work.py", "--help"])).unwrap(),
            RunRequest::Help
        ));
        // A genuinely unknown flag still fails, and still names itself.
        let complaint = parse_run(&argv(&["--halp"]))
            .err()
            .expect("an unknown flag must still be refused")
            .to_string();
        assert!(complaint.contains("--halp"), "{complaint}");
    }

    fn duration(key: &str) -> FormFieldSpec {
        let mut field = FormFieldSpec::text(key, "Time spent");
        field.kind = FormFieldKind::Duration;
        field
    }

    /// The whole point of normalising a preset: `1h30` has to mean what it
    /// means at the prompt, or a form filled from a cron line and the same
    /// form filled by hand hand the script two different numbers.
    #[test]
    fn a_supplied_value_is_converted_the_way_a_typed_one_is() {
        let mut used = BTreeSet::new();
        let input = collect_round(
            &state(
                vec![
                    duration("spent"),
                    FormFieldSpec::toggle("billable", "Billable"),
                    FormFieldSpec::select(
                        "activity",
                        "Activity",
                        vec!["dev".into(), "review".into()],
                    ),
                ],
                &[],
            ),
            &preset(&[
                ("spent", "1h30"),
                ("billable", "yes"),
                ("activity", "review"),
            ]),
            &mut used,
            false,
            &mut ScriptedPrompter::new(),
        )
        .expect("every field supplied");
        assert_eq!(input.values["spent"], "90");
        assert_eq!(input.values["billable"], "true");
        assert_eq!(input.values["activity"], "review");
    }

    /// And a preset that cannot mean anything says so with the flag that
    /// wrote it — the only place the user can still fix it.
    #[test]
    fn a_supplied_value_that_does_not_fit_names_the_flag() {
        let mut used = BTreeSet::new();
        let failure = collect_round(
            &state(vec![duration("spent")], &[]),
            &preset(&[("spent", "banana")]),
            &mut used,
            false,
            &mut ScriptedPrompter::new(),
        )
        .expect_err("banana is not a duration");
        assert!(
            failure.to_string().contains("`--set spent=banana`"),
            "{failure}"
        );
    }

    /// A preset also decides what `visible_when` sees — a field switched on
    /// from the shell has to reveal the same fields it reveals in the popup.
    #[test]
    fn a_supplied_value_drives_the_conditions_too() {
        let mut reviewer = FormFieldSpec::text("reviewer", "Reviewer");
        reviewer.visible_when = Some(not_yet_done_content::FieldCondition {
            field: "activity".into(),
            equals_any: vec!["review".into()],
            negate: false,
        });
        let mut used = BTreeSet::new();
        let failure = collect_round(
            &state(
                vec![
                    FormFieldSpec::select(
                        "activity",
                        "Activity",
                        vec!["dev".into(), "review".into()],
                    ),
                    reviewer,
                ],
                &[],
            ),
            &preset(&[("activity", "review")]),
            &mut used,
            false,
            &mut ScriptedPrompter::new(),
        )
        .expect_err("the revealed field has nothing in it");
        assert!(failure.to_string().contains("`reviewer`"), "{failure}");
    }

    /// `--set` answers a field for good: it is not prompted for, and the round
    /// counts as unattended so far as the loop guard is concerned.
    #[test]
    fn a_supplied_field_is_never_asked_about() {
        let mut used = BTreeSet::new();
        let input = collect_round(
            &state(vec![FormFieldSpec::text("title", "Title")], &[]),
            &preset(&[("title", "from the shell")]),
            &mut used,
            true,
            &mut ScriptedPrompter::new(),
        )
        .expect("nothing to ask");
        assert_eq!(input.values["title"], "from the shell");
        assert_eq!(input.asked, 0, "no prompt was needed");
        assert!(used.contains("title"));
    }

    /// The one thing the plan insists on: no terminal, a field with nothing in
    /// it, and the abort says which field.
    #[test]
    fn without_a_terminal_a_missing_field_names_itself() {
        let err = collect_round(
            &state(vec![FormFieldSpec::text("title", "Title")], &[]),
            &preset(&[]),
            &mut BTreeSet::new(),
            false,
            &mut ScriptedPrompter::new(),
        )
        .expect_err("nobody can answer");
        let message = err.to_string();
        assert!(message.contains("`title`"), "{message}");
        assert!(message.contains("--set title="), "{message}");
    }

    /// A field that already has a value needs no terminal — that is what a
    /// default is for.
    #[test]
    fn without_a_terminal_a_field_that_has_a_value_is_fine() {
        let input = collect_round(
            &state(
                vec![FormFieldSpec::text("title", "Title")],
                &[("title", "from the default")],
            ),
            &preset(&[]),
            &mut BTreeSet::new(),
            false,
            &mut ScriptedPrompter::new(),
        )
        .expect("the default answers it");
        assert_eq!(input.values["title"], "from the default");
    }

    /// A hidden field is not asked about and does not reach the script — the
    /// absence is what tells it the field did not apply.
    #[test]
    fn a_hidden_field_is_neither_asked_about_nor_submitted() {
        let fields = vec![
            FormFieldSpec::toggle("billable", "Billable"),
            FormFieldSpec::text("rate", "Rate").visible_when("billable", "true"),
        ];
        let input = collect_round(
            &state(fields, &[("billable", "false"), ("rate", "120")]),
            &preset(&[]),
            &mut BTreeSet::new(),
            true,
            &mut ScriptedPrompter::new().push_confirm(false),
        )
        .expect("only the toggle is asked about");
        assert_eq!(input.values["billable"], "false");
        assert!(
            !input.values.contains_key("rate"),
            "the hidden field stays out of the submission: {:?}",
            input.values
        );
        assert_eq!(input.asked, 1);
    }

    /// Every kind reaches the gesture it calls for, and comes back in the form
    /// the protocol wants — the CLI's counterpart to the popup's widget map.
    #[test]
    fn each_kind_is_asked_for_in_its_own_way() {
        let fields = vec![
            FormFieldSpec::toggle("done", "Done"),
            FormFieldSpec::select("sprint", "Sprint", vec!["S-41".into(), "S-42".into()]),
            FormFieldSpec::multiselect("tags", "Tags", vec!["red".into(), "blue".into()]),
            FormFieldSpec::number("size", "Size"),
            FormFieldSpec::duration("spent", "Spent"),
            FormFieldSpec::text("token", "Token").masked(),
        ];
        let prompter = ScriptedPrompter::new()
            .push_confirm(true)
            .push_select(1)
            .push_multi_select(vec![1])
            .push_text("7")
            .push_text("1h30")
            .push_password("s3cret");
        let input = collect_round(
            &state(fields, &[]),
            &preset(&[]),
            &mut BTreeSet::new(),
            true,
            &mut { prompter },
        )
        .expect("one answer per field");
        assert_eq!(input.values["done"], "true");
        assert_eq!(input.values["sprint"], "S-42");
        assert_eq!(input.values["tags"], "blue");
        assert_eq!(input.values["size"], "7");
        assert_eq!(input.values["spent"], "90", "`1h30` submits as minutes");
        assert_eq!(input.values["token"], "s3cret");
    }

    /// A number outside its bounds is refused and asked for again, rather than
    /// being handed to the script for it to complain about a round later.
    #[test]
    fn a_number_out_of_range_is_asked_for_again() {
        let mut prompter = ScriptedPrompter::new().push_text("99").push_text("5");
        let mut field = FormFieldSpec::number("size", "Size");
        field.kind = FormFieldKind::Number {
            min: Some(1),
            max: Some(10),
            step: None,
        };
        let value = ask_field(&mut prompter, &field, None).expect("the second answer fits");
        assert_eq!(value, "5");
    }

    /// The separator is not a matter of taste on the wire: what reaches the
    /// script has a dot and the field's precision.
    #[test]
    fn a_decimal_is_cut_to_its_precision_and_keeps_the_dot() {
        let field = FormFieldSpec::decimal("hours", "Hours");
        assert_eq!(
            fractional_number("1.567", &field, None, None, Some(2)),
            Ok("1.57".to_string())
        );
        assert_eq!(
            fractional_number("2", &field, None, None, None),
            Ok("2".to_string())
        );
    }

    /// Nothing to ask and nothing changed: the round after would be the round
    /// before, so the run stops and says what the script wanted.
    #[test]
    fn a_form_that_cannot_move_says_what_it_was_complaining_about() {
        let mut state = state(vec![FormFieldSpec::text("title", "Title")], &[]);
        state
            .field_errors
            .insert("title".into(), vec!["already taken".into()]);
        let message = stuck_message(&state);
        assert!(message.contains("title: already taken"), "{message}");
    }
}
