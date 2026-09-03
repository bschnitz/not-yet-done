//! `nyd config import-thunderbird` — every Thunderbird account, as one mail
//! instance plus one view.
//!
//! The mail adapter holds many accounts in a single instance, so an import is
//! two files and not two files per mailbox: `mail-adapter.yaml` with an entry
//! per account, and `mail.yaml` with a subtab per account pinned to it by an
//! `account:<id>` query.
//!
//! What is imported is what Thunderbird states — host, port, socket type,
//! login name, display name. What is not imported is anything secret: the
//! passwords stay where they are, and each account gets a `pass` path that is
//! a GUESS, printed as such, because a wrong guess should read as a line to
//! fix rather than as a mysterious login failure.

mod accounts;
mod emit;
mod prefs;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use accounts::{Account, Skipped};

/// `nyd config import-thunderbird [--profile <dir>] [--pass-prefix <p>]
/// [--out-dir <dir>] [--stdout] [--force]`
pub fn run(args: &[String]) -> Result<()> {
    let opts = Options::parse(&args[3.min(args.len())..])?;

    let prefs_path = prefs::find_prefs(opts.profile.as_deref())?;
    let prefs = prefs::Prefs::read(&prefs_path)?;
    let (accounts, skipped) = accounts::collect(&prefs);

    if accounts.is_empty() {
        bail!(
            "no IMAP account in {} — nothing to import{}",
            prefs_path.display(),
            skipped_hint(&skipped)
        );
    }

    let adapter = emit::adapter_yaml(&accounts, &opts.pass_prefix, &prefs_path.display().to_string());
    let view = emit::view_yaml(&accounts);

    if opts.stdout {
        println!("# ===== views/mail-adapter.yaml =====\n{adapter}");
        println!("# ===== views/mail.yaml =====\n{view}");
    } else {
        let dir = match &opts.out_dir {
            Some(dir) => dir.clone(),
            None => not_yet_done_host::views_dir(),
        };
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        write_file(&dir.join("mail-adapter.yaml"), &adapter, opts.force)?;
        write_file(&dir.join("mail.yaml"), &view, opts.force)?;
        eprintln!("wrote {}/mail-adapter.yaml and {}/mail.yaml", dir.display(), dir.display());
    }

    report(&accounts, &skipped, &opts.pass_prefix, &prefs_path);
    Ok(())
}

/// Refuse to overwrite by default. The two file names are the ones a working
/// mail setup already uses, and a hand-tuned config is exactly what somebody
/// would run this command near — losing it to a convenience would be a poor
/// trade.
fn write_file(path: &Path, content: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists — pass --force to overwrite it, --out-dir <dir> to write elsewhere, or --stdout to just look",
            path.display()
        );
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// What the import decided, printed where the user reads it rather than only
/// buried in the file's comments.
fn report(accounts: &[Account], skipped: &[Skipped], pass_prefix: &str, profile: &Path) {
    eprintln!("\nimported {} account(s) from {}", accounts.len(), profile.display());
    for account in accounts {
        let port = account
            .port
            .map(|p| format!(":{p}"))
            .unwrap_or_else(|| " (default port)".into());
        eprintln!(
            "\n  {} — {}\n    {}{} over {}, login {}",
            account.id,
            account.name,
            account.host,
            port,
            account.security.as_yaml(),
            account.username,
        );
        eprintln!("    password from: {pass_prefix}/{}/pass   (a GUESS — check it)", account.id);
        for note in &account.notes {
            eprintln!("    note: {note}");
        }
    }
    if !skipped.is_empty() {
        eprintln!("\nnot imported:");
        for entry in skipped {
            eprintln!("  {} — {}", entry.label, entry.reason);
        }
    }
    eprintln!(
        "\nNext: check the `{pass_prefix}/…/pass` paths against `pass ls`, then open the Mail tab.\n\
         An id appears in BOTH files (as `id:` and as `account:<id>`); rename it in both or in neither."
    );
}

fn skipped_hint(skipped: &[Skipped]) -> String {
    if skipped.is_empty() {
        String::new()
    } else {
        format!(
            " ({} other server(s) were passed over: {})",
            skipped.len(),
            skipped
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[derive(Debug)]
struct Options {
    profile: Option<String>,
    pass_prefix: String,
    out_dir: Option<PathBuf>,
    stdout: bool,
    force: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut opts = Options {
            profile: None,
            pass_prefix: "mail".into(),
            out_dir: None,
            stdout: false,
            force: false,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |flag: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| anyhow!("{flag} needs a value"))
            };
            match arg.as_str() {
                "--profile" => opts.profile = Some(value("--profile")?),
                "--pass-prefix" => opts.pass_prefix = value("--pass-prefix")?.trim_matches('/').to_string(),
                "--out-dir" => opts.out_dir = Some(prefs::expand_tilde(&value("--out-dir")?)),
                "--stdout" => opts.stdout = true,
                "--force" => opts.force = true,
                other => bail!(
                    "unknown option '{other}' (use --profile, --pass-prefix, --out-dir, --stdout, --force)"
                ),
            }
        }
        Ok(opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented profile in Thunderbird's own shape: two mailboxes, one of
    /// them behind a plain-text local bridge.
    const PROFILE: &str = r#"
user_pref("mail.accountmanager.accounts", "account1,account2");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account1.identities", "id1");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.socketType", 3);
user_pref("mail.server.server1.userName", "ada@example.org");
user_pref("mail.server.server1.name", "Private");
user_pref("mail.identity.id1.useremail", "ada@example.org");
user_pref("mail.account.account2.server", "server2");
user_pref("mail.account.account2.identities", "id2");
user_pref("mail.server.server2.type", "imap");
user_pref("mail.server.server2.hostname", "localhost");
user_pref("mail.server.server2.port", 1143);
user_pref("mail.server.server2.socketType", 0);
user_pref("mail.server.server2.userName", "ada@work.example");
user_pref("mail.identity.id2.useremail", "ada@work.example");
"#;

    fn generated() -> (String, String) {
        let prefs = prefs::Prefs::parse(PROFILE);
        let (accounts, _) = accounts::collect(&prefs);
        (
            emit::adapter_yaml(&accounts, "mail", "/profile/prefs.js"),
            emit::view_yaml(&accounts),
        )
    }

    #[test]
    fn both_generated_files_are_yaml_that_parses() {
        // The view file leans on anchors and aliases; if one were emitted
        // before its anchor, this is where it shows.
        let (adapter, view) = generated();
        serde_yaml::from_str::<serde_yaml::Value>(&adapter).expect("adapter yaml");
        serde_yaml::from_str::<serde_yaml::Value>(&view).expect("view yaml");
    }

    #[test]
    fn every_account_reaches_both_files_under_the_same_id() {
        // The id is the only thing tying a subtab to a mailbox, and it lives
        // in two files: a drift between them is a tab that resolves to
        // nothing.
        let (adapter, view) = generated();
        let doc: serde_yaml::Value = serde_yaml::from_str(&adapter).unwrap();
        let ids: Vec<String> = doc["accounts"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["example-org", "work-example"]);

        let view_doc: serde_yaml::Value = serde_yaml::from_str(&view).unwrap();
        let queries: Vec<String> = view_doc["views"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| v["query"]["default"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(queries, ["account:example-org", "account:work-example"]);
    }

    #[test]
    fn the_plain_text_bridge_survives_the_trip() {
        // The one value that must not be "improved": localhost on 1143 speaks
        // plain text on purpose, and deriving security from the port would
        // silently break it.
        let (adapter, _) = generated();
        let doc: serde_yaml::Value = serde_yaml::from_str(&adapter).unwrap();
        let bridged = &doc["accounts"][1];
        assert_eq!(bridged["host"].as_str(), Some("localhost"));
        assert_eq!(bridged["port"].as_u64(), Some(1143));
        assert_eq!(bridged["security"].as_str(), Some("none"));
    }

    #[test]
    fn an_existing_config_is_not_overwritten_by_accident() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.yaml");
        std::fs::write(&path, "# hand-written\n").unwrap();

        let err = write_file(&path, "# generated\n", false).unwrap_err().to_string();
        assert!(err.contains("--force"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# hand-written\n");

        write_file(&path, "# generated\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# generated\n");
    }

    #[test]
    fn an_unknown_flag_is_refused_rather_than_ignored() {
        let args = ["--pass-prefix".to_string(), "secrets/mail".to_string()];
        assert_eq!(Options::parse(&args).unwrap().pass_prefix, "secrets/mail");

        let args = ["--dry-run".to_string()];
        let err = Options::parse(&args).unwrap_err().to_string();
        assert!(err.contains("--stdout"), "{err}");
    }
}
