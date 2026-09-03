//! The query a *level* is scoped by.
//!
//! A subtab is a `node_type` plus a `query:` — there is no "start here" field
//! in a view. So the way one instance shows six accounts in six subtabs is
//! that each subtab's folder level carries `query: "account:work"`, and this
//! is the parser that reads it.
//!
//! An unknown term is an error rather than a silent no-op: a query that is
//! quietly ignored looks exactly like a query that matched everything, and
//! the user would be left wondering why their subtab shows the wrong account.

/// What a folder-level query narrowed the listing to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FolderScope {
    /// Which account's mailboxes to list.
    pub(super) account: Option<String>,
    /// List the folders *below* this mailbox path instead of the top level —
    /// how a subtab pins itself to one subtree.
    pub(super) under: Option<String>,
}

/// Read `account:<id>` / `folder:<path>` out of a level query.
///
/// A mailbox path may contain spaces (`Sent Items` is on half the servers in
/// existence), so a `folder:` term keeps swallowing words until one of them
/// opens a new term. That is why the known prefixes are a closed list: it is
/// what tells a folder name apart from the next instruction.
pub(super) fn parse_folder_scope(query: Option<&str>) -> Result<FolderScope, String> {
    const PREFIXES: [&str; 2] = ["account:", "folder:"];

    let mut scope = FolderScope::default();
    let query = query.unwrap_or("").trim();
    if query.is_empty() {
        return Ok(scope);
    }
    let mut current: Option<(&str, String)> = None;
    for word in query.split_whitespace() {
        match PREFIXES.iter().find(|p| word.starts_with(**p)) {
            Some(prefix) => {
                store(&mut scope, current.take())?;
                let key = prefix.trim_end_matches(':');
                current = Some((key, word[prefix.len()..].to_string()));
            }
            // Not a new term: the continuation of the folder name being read.
            None => match current.as_mut() {
                Some((key, value)) if *key == "folder" => {
                    value.push(' ');
                    value.push_str(word);
                }
                _ => {
                    return Err(format!(
                        "cannot read `{word}` — a folder query understands `account:<id>` and `folder:<path>`"
                    ));
                }
            },
        }
    }
    store(&mut scope, current)?;
    Ok(scope)
}

fn store(scope: &mut FolderScope, term: Option<(&str, String)>) -> Result<(), String> {
    let Some((key, value)) = term else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Err(format!("`{key}:` needs a value"));
    }
    match key {
        "account" => scope.account = Some(value),
        _ => scope.under = Some(value),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subtab_pins_itself_to_one_account() {
        let scope = parse_folder_scope(Some("account:work")).expect("parses");
        assert_eq!(scope.account.as_deref(), Some("work"));
        assert_eq!(scope.under, None);
    }

    #[test]
    fn an_empty_query_scopes_nothing() {
        assert_eq!(
            parse_folder_scope(None).expect("parses"),
            FolderScope::default()
        );
        assert_eq!(
            parse_folder_scope(Some("   ")).expect("parses"),
            FolderScope::default()
        );
    }

    /// Mailbox names have spaces in them on every second server; a query that
    /// silently truncated at the first one would open the wrong folder.
    #[test]
    fn a_folder_term_may_carry_spaces() {
        let scope = parse_folder_scope(Some("account:work folder:Sent Items")).expect("parses");
        assert_eq!(scope.account.as_deref(), Some("work"));
        assert_eq!(scope.under.as_deref(), Some("Sent Items"));
    }

    /// Silently ignoring a term the parser does not know would look exactly
    /// like a term that matched everything.
    #[test]
    fn an_unknown_term_is_refused_and_says_what_is_understood() {
        let err = parse_folder_scope(Some("is:unread account:work")).expect_err("refused");
        assert!(err.contains("is:unread"), "{err}");
        assert!(err.contains("account:"), "{err}");
    }

    /// A folder name swallows the words after it — but not the next term, or
    /// the account would end up as part of the mailbox path.
    #[test]
    fn a_following_term_ends_the_folder_name() {
        let scope = parse_folder_scope(Some("folder:Sent Items account:work")).expect("parses");
        assert_eq!(scope.under.as_deref(), Some("Sent Items"));
        assert_eq!(scope.account.as_deref(), Some("work"));
    }

    #[test]
    fn a_term_without_a_value_is_refused() {
        assert!(parse_folder_scope(Some("account:")).is_err());
    }
}
