//! Writing the two YAML files.
//!
//! Emitted as text rather than through a serializer, for two reasons that both
//! matter more here than the safety of a round trip: the files carry the
//! comments explaining every decision the importer made, and the view file
//! leans on YAML anchors so six subtabs share one definition of the levels
//! below them. A serializer would throw both away and hand back a correct file
//! nobody wants to edit.

use super::accounts::Account;

/// Where the credential script lives, as the generated file spells it.
const CRED_SCRIPT: &str = "~/.config/not_yet_done/scripts/pass_credentials.py";

/// The adapter config: one instance, every account.
pub(super) fn adapter_yaml(accounts: &[Account], pass_prefix: &str, profile: &str) -> String {
    let mut s = String::new();
    s.push_str("# Mail (IMAP) — one instance, every account.\n#\n");
    s.push_str(&format!(
        "# Imported from the Thunderbird profile at\n#   {profile}\n# by `nyd config import-thunderbird`.\n#\n"
    ));
    s.push_str(
        "# Thunderbird's own `mail.server.*` prefs are the ground truth for host, port\n\
         # and login name: those are the values already known to work for these\n\
         # mailboxes. `security:` is stated rather than derived from the port — a\n\
         # bridge on loopback speaks plain text on purpose, and guessing would break\n\
         # it rather than protect anyone.\n#\n",
    );
    s.push_str(
        "# NO SECRET IS IN THIS FILE, and none was read out of Thunderbird: its own\n\
         # password store is NSS-encrypted and is deliberately left alone. Each\n\
         # account below fetches its password from `pass` at login time, through\n\
         # scripts/pass_credentials.py. The store paths are GUESSED from the account\n\
         # id — check each one; a wrong path shows up as a failed login.\n",
    );
    s.push_str("\nname: Mail\n\npage_size: 50\n\nretry:\n  attempts: 2\n  backoff_ms: 250\n");
    s.push_str("\naccounts:\n");

    for account in accounts {
        s.push_str(&format!("  # ── {} ", account.name));
        s.push_str(&"─".repeat(60usize.saturating_sub(account.name.chars().count())));
        s.push('\n');
        for note in &account.notes {
            s.push_str(&wrap_comment(note, "  # "));
        }
        s.push_str(&format!("  - id: {}\n", scalar(&account.id)));
        s.push_str(&format!("    name: {}\n", scalar(&account.name)));
        if let Some(address) = &account.address {
            s.push_str(&format!("    address: {}\n", scalar(address)));
        }
        s.push_str(&format!("    host: {}\n", scalar(&account.host)));
        if let Some(port) = account.port {
            s.push_str(&format!("    port: {port}\n"));
        }
        s.push_str(&format!("    security: {}\n", account.security.as_yaml()));
        s.push_str("    auth:\n      mechanism: password\n");
        s.push_str("      script: >-\n");
        s.push_str(&format!("        {CRED_SCRIPT}\n"));
        s.push_str(&format!(
            "        password={pass_prefix}/{}/pass\n",
            account.id
        ));
        s.push_str("      bindings:\n");
        // The login name is not a secret and the profile states it, so it goes
        // in literally: the store is then asked for one thing, and a failed
        // login has one possible cause instead of two.
        s.push_str("        - field: username\n");
        s.push_str(&format!(
            "          provider: {{ type: literal, value: {} }}\n",
            scalar(&account.username)
        ));
        s.push_str("        - field: password\n");
        s.push_str("          provider: { type: script-result }\n\n");
    }
    s.truncate(s.trim_end().len());
    s.push('\n');
    s
}

/// The view: one tab, one subtab per account.
pub(super) fn view_yaml(accounts: &[Account]) -> String {
    let keys = subtab_keys(accounts);
    let mut s = String::new();
    s.push_str(
        "# Mail — one tab, one subtab per account (see mail-adapter.yaml).\n#\n\
         # What binds a subtab to a mailbox is its level query, `account:<id>`, where\n\
         # the id is an `id:` from the adapter config. Renaming an id means renaming\n\
         # it in both files — an unknown id is refused by name rather than guessed.\n#\n\
         # Scope today: the FOLDER TREE, the MESSAGES under a folder, READING a\n\
         # message and its ATTACHMENTS. Writing — marking read, flagging, moving,\n\
         # sending — is the adapter's next phase.\n\n",
    );
    s.push_str("tab:\n  name: Mail\n  icon: \"\u{f01e0}\"\n");
    s.push_str("  # While any folder in this tab holds unread mail, the tab bar\n");
    s.push_str("  # prefixes the label with this marker — the only cue a background\n");
    s.push_str("  # mail tab can give.\n");
    s.push_str("  unread_marker: \"\u{2709}\"\n\n");
    s.push_str("adapter:\n  type: mail\n  id: mail\n  config: mail-adapter.yaml\n");
    s.push_str(&format!(
        "  # Connect the subtab actually opened, and only that one: {} mailboxes\n",
        accounts.len()
    ));
    s.push_str("  # asking for that many passwords at startup would be intolerable.\n");
    s.push_str("  auto_connect: on_open\n\nviews:\n");

    for (i, account) in accounts.iter().enumerate() {
        let first = i == 0;
        s.push_str(&format!("  - name: {}\n", scalar(&account.name)));
        s.push_str(&format!("    key: \"t {}\"\n", keys[i]));
        if first {
            s.push_str("    default: true\n");
        }
        s.push_str("    node_type: \"mail:folder\"\n");
        s.push_str(&format!(
            "    query: {{ default: \"account:{}\" }}\n",
            account.id
        ));
        s.push_str("    tree_label: name\n    unread_style: unread\n    unread_marker: \"●\"\n");
        // The messages list opens as a pane beside the tree, so the `w`
        // leader has panes to operate on. Without the opt-in it never
        // engages and `w q` / `w h` / `w l` do nothing here.
        s.push_str("    window_ops: true\n");
        if first {
            s.push_str(FIRST_SUBTAB_BODY);
        } else {
            s.push_str(
                "    columns: *folder_columns\n    actions: *folder_actions\n    children: *folder_children\n",
            );
        }
        s.push('\n');
    }
    s.truncate(s.trim_end().len());
    s.push('\n');
    s
}

/// The levels below a folder, written out once. Every further subtab aliases
/// the three anchors defined here, so a change to a column or a keybinding is
/// made in one place and not once per mailbox.
const FIRST_SUBTAB_BODY: &str = r#"    columns: &folder_columns
      - { key: name, label: Folder, source: label, sizing: "flex(1)" }
      # `unread_count`, not `unread`: rows carry a separate `unread` field
      # holding "true", and that is the key the unread highlight and the tab
      # marker are painted from. A count on the same name takes it over.
      - { key: unread_count, label: Unread, kind: number, sizing: "fixed(8)" }
      - { key: total, label: Total, kind: number, sizing: "fixed(8)" }
      # The server's own spelling of the path — what SELECT uses and what
      # exclude_folders matches. Off by default, on via `c c`.
      - { key: path, label: Path, sizing: "flex(1)", hidden: true }
    actions: &folder_actions
      - { name: refresh, key: r, type: reload }
      # Enter on a folder that has subfolders expands it, so its messages
      # need a key of their own. On a leaf folder Enter opens them directly
      # and `m` does the same — one key that always works.
      - { name: messages, key: m, type: navigate, navigate_to: "mail:message" }
      - name: fuzzy filter
        key: "f f"
        type: fuzzy_filter
        fuzzy_filter:
          fields: [name]
    children: &folder_children
      # One page = one UID SEARCH + one UID FETCH of exactly this window, so a
      # mailbox holding 40 000 mails opens as fast as one holding 40. A row is
      # the envelope; the body is a separate fetch, made only for the mail the
      # cursor is on.
      - &messages
        name: messages
        node_type: "mail:message"
        # The tree stays on the left and the list opens beside it. `coupled`
        # makes that pane the tree's own: opening another folder replaces the
        # list instead of stacking a third pane.
        split: { direction: right, ratio: 0.65, coupled: true }
        # Backing out of the list closes its pane rather than walking the
        # pane up the folder tree: this pane is the tree's own list, so one
        # level up would leave a second folder list beside the tree.
        # `back: null` first: two handlers on one key would let the chain
        # win quietly, and the validator flags exactly that.
        keybindings:
          back: null
        action_chains:
          backspace: [window.close]
          h: [window.close]
        pagination: { mode: server, page_size: 50 }
        cursor_on_open: first_unread
        columns:
          # ● unread, ↩ answered, ★ flagged, ✎ draft, 📎 has an attachment —
          # in a fixed order, so the column does not jitter from row to row.
          - { key: flags, label: "", sizing: "fixed(6)" }
          - { key: from, label: From, sizing: "flex(1)" }
          # `source: label`, so a mail with an empty Subject header still has
          # a row to aim at: the label carries the `(no subject)` stand-in.
          - { key: subject, label: Subject, source: label, sizing: "flex(2)" }
          - { key: date, label: Date, kind: datetime, sizing: "fixed(16)" }
          - {
              key: size,
              label: Size,
              kind: number,
              sizing: "fixed(9)",
              hidden: true,
            }
          - {
              key: attachments,
              label: Att,
              kind: number,
              sizing: "fixed(5)",
              hidden: true,
            }
          - { key: to, label: To, sizing: "flex(1)", hidden: true }
          - { key: account, label: Account, sizing: "fixed(10)", hidden: true }
        actions:
          - { name: refresh, key: r, type: reload }
        # A header block (From / To / Subject / Date, plus `Attachments:` when
        # there are any) over the text. `markdown: false` is a decision, not an
        # omission: a mail is not markdown, and turning it on reflows the
        # header block and eats the `>` quoting of every reply.
        #
        # Fetched with BODY.PEEK, so moving the cursor does NOT mark a mail
        # read — that stays a thing you do on purpose.
        preview:
          enabled: true
          source: content
          markdown: false
          keybinding: p
        children:
          # Free: the part list arrives with the envelope, so drilling into a
          # mail costs no round trip. Only opening a file fetches, and then
          # only that part.
          - name: attachments
            node_type: "mail:attachment"
            columns:
              - {
                  key: filename,
                  label: File,
                  source: label,
                  style: text_high,
                  sizing: "flex(1)",
                }
              - { key: content_type, label: Type, sizing: max }
              - { key: size, label: Size, kind: number, sizing: "fixed(10)" }
              - { key: part, label: Part, sizing: "fixed(6)", hidden: true }
            actions:
              # On the `o` leader: a bare `o` would swallow every global
              # chord starting with it (shortcut overview/menu).
              - { name: open, key: "o o", type: custom, id: open }
              - { name: download all, key: D, type: custom, id: download_all }
      - name: subfolders
        node_type: "mail:folder"
        tree_label: name
        recursive: true
        leaf_glyph: "📁"
        # A nested folder reaches its messages the same way.
        children:
          - *messages
"#;

/// One letter per subtab, taken from the account's own name where that letter
/// is still free. The chord is `t <letter>`, so the letters only have to be
/// unique among themselves — not against the view's own `r`/`m`/`p`.
fn subtab_keys(accounts: &[Account]) -> Vec<char> {
    let mut used: Vec<char> = Vec::new();
    accounts
        .iter()
        .map(|account| {
            let preferred = account
                .name
                .chars()
                .chain(account.id.chars())
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase());
            let key = preferred
                .chain('a'..='z')
                .find(|c| !used.contains(c))
                .unwrap_or('z');
            used.push(key);
            key
        })
        .collect()
}

/// A YAML scalar, quoted only when it has to be. Plain is the readable form
/// and most of these values are plain, but a display name is free text and an
/// unquoted `Work: inbox` or `#1` would change the file's meaning.
fn scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | '+' | '-'))
        && !value.starts_with('-');
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// A note as a comment block, wrapped so the file stays readable at 80 columns.
fn wrap_comment(text: &str, prefix: &str) -> String {
    let width = 79usize.saturating_sub(prefix.len());
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push_str(prefix);
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(prefix);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thunderbird::accounts::Security;

    fn account(id: &str, name: &str, host: &str) -> Account {
        Account {
            id: id.into(),
            name: name.into(),
            address: Some(format!("ada@{id}.example")),
            host: host.into(),
            port: None,
            security: Security::Tls,
            username: format!("ada@{id}.example"),
            notes: Vec::new(),
        }
    }

    #[test]
    fn no_password_reaches_the_generated_file() {
        // The importer never reads one, and the shape it writes has nowhere to
        // put one: everything comes from the store at login time.
        let yaml = adapter_yaml(&[account("one", "One", "imap.example.org")], "mail", "/tmp/p");
        assert!(yaml.contains("password=mail/one/pass"));
        assert!(yaml.contains("provider: { type: script-result }"));
        assert!(!yaml.to_lowercase().contains("password: "));
    }

    #[test]
    fn each_subtab_gets_a_key_of_its_own() {
        // Two accounts whose names start with the same letter: the second has
        // to move rather than shadow the first.
        let accounts = [
            account("one", "Mail", "a.example.org"),
            account("two", "Music", "b.example.org"),
        ];
        let keys = subtab_keys(&accounts);
        assert_eq!(keys, ['m', 'u']);
        let yaml = view_yaml(&accounts);
        assert!(yaml.contains("key: \"t m\""));
        assert!(yaml.contains("key: \"t u\""));
    }

    #[test]
    fn only_the_first_subtab_spells_the_levels_out() {
        let accounts = [
            account("one", "One", "a.example.org"),
            account("two", "Two", "b.example.org"),
        ];
        let yaml = view_yaml(&accounts);
        assert_eq!(yaml.matches("&folder_columns").count(), 1);
        assert_eq!(yaml.matches("*folder_columns").count(), 1);
        assert!(yaml.contains("    default: true\n"));
        assert_eq!(yaml.matches("default: true").count(), 1);
    }

    #[test]
    fn a_name_that_would_change_the_yaml_gets_quoted() {
        assert_eq!(scalar("post.example"), "post.example");
        assert_eq!(scalar("Work: inbox"), "\"Work: inbox\"");
        assert_eq!(scalar("#1"), "\"#1\"");
        assert_eq!(scalar(""), "\"\"");
    }
}
