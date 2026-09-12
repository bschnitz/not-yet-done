//! Turning Thunderbird's prefs into accounts this adapter can connect with.
//!
//! Thunderbird's own `mail.server.*` block is the ground truth here, and that
//! is the point of importing at all: host, port, socket type and login name
//! are the values already known to work for that mailbox. Nothing is guessed
//! that the profile states, and everything guessed says so.

use super::prefs::Prefs;

/// How the connection is secured, in this adapter's spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Security {
    Tls,
    Starttls,
    None,
}

impl Security {
    pub(super) fn as_yaml(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::Starttls => "starttls",
            Security::None => "none",
        }
    }

    /// The port this adapter uses when the account names none. Kept in step
    /// with `Security::default_port` in the mail adapter, so the importer can
    /// leave `port:` out whenever Thunderbird left it out too.
    fn default_port(self) -> u16 {
        match self {
            Security::Tls => 993,
            Security::Starttls | Security::None => 143,
        }
    }

    /// The same for SUBMISSION, which has an entirely different port scheme —
    /// 465/587/25 against reading's 993/143. One table per direction, because
    /// a submission port derived from the reading defaults would land on 993
    /// and fail in a way that reads like a firewall.
    fn default_submission_port(self) -> u16 {
        match self {
            Security::Tls => 465,
            Security::Starttls => 587,
            Security::None => 25,
        }
    }
}

/// Where an account submits outgoing mail, as Thunderbird states it.
///
/// Separate from the account because it is a separate server with a separate
/// port scheme and, often enough, a separate login. An account without one is
/// not broken: it reads mail and refuses to send, which is exactly what
/// Thunderbird does with it too.
#[derive(Debug)]
pub(super) struct Smtp {
    pub(super) host: String,
    /// Emitted only when it differs from this adapter's default for the
    /// security, the same rule the reading side follows.
    pub(super) port: Option<u16>,
    pub(super) security: Security,
    /// Only when submission logs in under a different name than reading.
    /// Absent means "the account's own credentials", which is the normal case
    /// and keeps the emitted block to three lines.
    pub(super) username: Option<String>,
    /// The identity's display name — what the recipient sees in `From`.
    pub(super) from_name: Option<String>,
}

/// One importable mailbox.
#[derive(Debug)]
pub(super) struct Account {
    /// Derived id — the first segment of every node id below the account, and
    /// what the view file's `account:<id>` query names.
    pub(super) id: String,
    /// Display label for the account and its subtab.
    pub(super) name: String,
    pub(super) address: Option<String>,
    pub(super) host: String,
    /// Emitted only when Thunderbird states one, so an account that rides on
    /// the default keeps riding on it.
    pub(super) port: Option<u16>,
    pub(super) security: Security,
    /// The login name, which is not the address often enough that the two are
    /// separate fields all the way down.
    pub(super) username: String,
    /// The outgoing server, when the profile names one for this account.
    /// `None` leaves the emitted account without an `smtp:` block, and the
    /// adapter then says so when asked to send rather than guessing a host.
    pub(super) smtp: Option<Smtp>,
    /// Lines to print after the run and to leave in the file as comments —
    /// everything the importer had to decide rather than read.
    pub(super) notes: Vec<String>,
}

/// A server in the profile that this adapter cannot take over.
#[derive(Debug)]
pub(super) struct Skipped {
    pub(super) label: String,
    pub(super) reason: String,
}

/// Read every account of a profile, in the order Thunderbird lists them.
pub(super) fn collect(prefs: &Prefs) -> (Vec<Account>, Vec<Skipped>) {
    let mut accounts = Vec::new();
    let mut skipped = Vec::new();
    let mut used_ids: Vec<String> = Vec::new();

    for key in prefs.list("mail.accountmanager.accounts") {
        let Some(server) = prefs.get(&format!("mail.account.{key}.server")) else {
            continue;
        };
        let g = |field: &str| prefs.get(&format!("mail.server.{server}.{field}"));
        let kind = g("type").unwrap_or("");
        let label = g("name")
            .or_else(|| g("hostname"))
            .unwrap_or(&key)
            .to_string();

        if kind != "imap" {
            // `none` is the Local Folders pseudo-account every profile has;
            // it is not a server anybody meant to import, so it is not worth
            // a line of the user's attention.
            if kind != "none" {
                skipped.push(Skipped {
                    label,
                    reason: format!("Thunderbird speaks {kind} to it; this adapter is IMAP only"),
                });
            }
            continue;
        }
        let Some(host) = g("hostname") else {
            skipped.push(Skipped {
                label,
                reason: "no hostname in the profile".into(),
            });
            continue;
        };

        let mut notes = Vec::new();
        let port = prefs.get_u16(&format!("mail.server.{server}.port"));
        let security = security_of(prefs, server, port, &mut notes);
        let username = g("userName").unwrap_or_default().to_string();
        let identity = first_identity(prefs, &key);
        let address = identity
            .as_deref()
            .and_then(|id| prefs.get(&format!("mail.identity.{id}.useremail")))
            .map(str::to_string);

        if username.is_empty() {
            skipped.push(Skipped {
                label,
                reason: "no login name in the profile".into(),
            });
            continue;
        }
        if let Some(addr) = &address {
            if addr != &username {
                notes.push(format!(
                    "the login name is `{username}`, not the address — Thunderbird logs in with it, so this file does too"
                ));
            }
        }
        note_auth_method(prefs, server, &mut notes);
        let smtp = smtp_of(prefs, identity.as_deref(), &username, &mut notes);

        // Leave `port:` out when Thunderbird did AND the adapter's default for
        // this security agrees with Thunderbird's — otherwise state it, so
        // nothing silently moves to another port.
        let port = match port {
            Some(p) if p == security.default_port() => None,
            other => other,
        };

        let id = unique_id(address.as_deref(), &host, &mut used_ids);
        accounts.push(Account {
            id,
            name: label,
            address,
            host: host.to_string(),
            port,
            security,
            username,
            smtp,
            notes,
        });
    }

    (accounts, skipped)
}

/// Thunderbird's `socketType`, which is a small enum and not a guess:
/// 0 plain, 1 the retired "try STARTTLS", 2 always STARTTLS, 3 implicit TLS.
///
/// A profile that names none is the only case with anything to decide, and the
/// port is the better evidence than any default would be.
fn security_of(
    prefs: &Prefs,
    server: &str,
    port: Option<u16>,
    notes: &mut Vec<String>,
) -> Security {
    match prefs.get_i32(&format!("mail.server.{server}.socketType")) {
        Some(0) => Security::None,
        Some(1) => {
            notes.push(
                "Thunderbird has this on its retired \"try STARTTLS\" setting; imported as starttls, which does not fall back to plain text".into(),
            );
            Security::Starttls
        }
        Some(2) => Security::Starttls,
        Some(3) => Security::Tls,
        other => {
            let security = match port {
                Some(143) => Security::Starttls,
                Some(993) | None => Security::Tls,
                Some(_) => Security::Tls,
            };
            notes.push(format!(
                "the profile states no socketType{}, so `security: {}` was derived from the port — check it",
                other.map(|v| format!(" this importer knows (it says {v})")).unwrap_or_default(),
                security.as_yaml()
            ));
            security
        }
    }
}

/// Thunderbird's `authMethod`: 10 is OAuth2, which this adapter does not do
/// yet (plan phase 8). Everything else reaches the server as an IMAP LOGIN.
fn note_auth_method(prefs: &Prefs, server: &str, notes: &mut Vec<String>) {
    let is_gmail = prefs.get_bool(&format!("mail.server.{server}.is_gmail")) == Some(true);
    if prefs.get_i32(&format!("mail.server.{server}.authMethod")) == Some(10) {
        notes.push(
            "Thunderbird signs this account in with OAuth2. This adapter logs in with a password, so the store needs an APP PASSWORD for it, not the account password".into(),
        );
        if is_gmail {
            notes.push(
                "Google issues those under Account → Security → App passwords, with 2-step verification switched on".into(),
            );
        }
    }
}

/// The account's first identity — the one carrying its address, its display
/// name and the outgoing server it submits through. Thunderbird allows several
/// per account (an alias, a second signature); the first is the one it sends
/// with unless told otherwise, and importing all of them would mean inventing
/// a concept this adapter does not have.
fn first_identity(prefs: &Prefs, account: &str) -> Option<String> {
    prefs
        .list(&format!("mail.account.{account}.identities"))
        .into_iter()
        .find(|id| {
            prefs
                .get(&format!("mail.identity.{id}.useremail"))
                .is_some()
        })
}

/// The outgoing server for one identity.
///
/// Thunderbird keeps submission in its own `mail.smtpserver.*` block and links
/// it from the identity; an identity naming none rides on
/// `mail.smtp.defaultserver`. Both indirections are followed here, because the
/// alternative is an account that reads mail and cannot answer it.
fn smtp_of(
    prefs: &Prefs,
    identity: Option<&str>,
    imap_username: &str,
    notes: &mut Vec<String>,
) -> Option<Smtp> {
    let key = identity
        .and_then(|id| prefs.get(&format!("mail.identity.{id}.smtpServer")))
        .filter(|k| !k.is_empty())
        .or_else(|| prefs.get("mail.smtp.defaultserver"))
        .filter(|k| !k.is_empty())?;

    let g = |field: &str| prefs.get(&format!("mail.smtpserver.{key}.{field}"));
    let Some(host) = g("hostname").filter(|h| !h.is_empty()) else {
        notes.push(
            "Thunderbird names an outgoing server for this account but no hostname for it, so no `smtp:` block was written — the account reads mail and refuses to send".into(),
        );
        return None;
    };

    let port = prefs.get_u16(&format!("mail.smtpserver.{key}.port"));
    // `try_ssl` is the submission side's spelling of `socketType`, same enum.
    let security = match prefs.get_i32(&format!("mail.smtpserver.{key}.try_ssl")) {
        Some(0) => Security::None,
        Some(1) => {
            notes.push(
                "the outgoing server is on Thunderbird's retired \"try STARTTLS\" setting; imported as starttls, which does not fall back to plain text".into(),
            );
            Security::Starttls
        }
        Some(2) => Security::Starttls,
        Some(3) => Security::Tls,
        other => {
            let derived = match port {
                Some(25) => Security::None,
                Some(587) => Security::Starttls,
                _ => Security::Tls,
            };
            notes.push(format!(
                "the outgoing server states no try_ssl{}, so its `security: {}` was derived from the port — check it",
                other
                    .map(|v| format!(" this importer knows (it says {v})"))
                    .unwrap_or_default(),
                derived.as_yaml()
            ));
            derived
        }
    };

    if prefs.get_i32(&format!("mail.smtpserver.{key}.authMethod")) == Some(10) {
        notes.push(
            "Thunderbird submits through this server with OAuth2 as well, so the app password the store holds for reading is what sending needs too".into(),
        );
    }

    // A submission login that matches the reading one needs no `auth:` at
    // all — the adapter falls back to the account's credentials, which is one
    // password in one place rather than the same password in two.
    let username = g("username")
        .filter(|u| !u.is_empty() && *u != imap_username)
        .map(str::to_string);
    if let Some(name) = &username {
        notes.push(format!(
            "submission logs in as `{name}`, not as the reading login — its own `auth:` block was written, pointing at the SAME store path; correct it if the two passwords differ"
        ));
    }

    let from_name = identity
        .and_then(|id| prefs.get(&format!("mail.identity.{id}.fullName")))
        .filter(|n| !n.is_empty())
        .map(str::to_string);

    Some(Smtp {
        host: host.to_string(),
        port: match port {
            Some(p) if p == security.default_submission_port() => None,
            other => other,
        },
        security,
        username,
        from_name,
    })
}

/// A short, stable id derived from the account's own domain.
///
/// The domain and not the host, because two mailboxes can share a host — the
/// pair behind a local Exchange bridge does exactly that, and an id taken from
/// `127.0.0.1` would collide for the one thing an id must not collide on.
fn unique_id(address: Option<&str>, host: &str, used: &mut Vec<String>) -> String {
    let source = address
        .and_then(|a| a.split('@').nth(1))
        .filter(|d| !d.is_empty())
        .unwrap_or(host);
    let base = slug(source);
    let mut candidate = base.clone();
    let mut n = 2;
    while used.contains(&candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    used.push(candidate.clone());
    candidate
}

/// `north.example.org` → `north-example-org`. Dots become dashes rather than
/// disappearing, so two domains differing only in their suffix stay two ids.
fn slug(source: &str) -> String {
    let cleaned: String = source
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = cleaned.trim_matches('-').replace("--", "-");
    if slug.is_empty() {
        "account".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile in the shape Thunderbird writes, with invented accounts:
    /// one on implicit TLS, one on STARTTLS with no port of its own, one
    /// OAuth2 mailbox, and two that share a host behind a local bridge.
    const PROFILE: &str = r#"
user_pref("mail.accountmanager.accounts", "account1,account2,account3,account4,account5,account9");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account1.identities", "id1");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.socketType", 3);
user_pref("mail.server.server1.userName", "ada@example.org");
user_pref("mail.server.server1.name", "Private");
user_pref("mail.identity.id1.useremail", "ada@example.org");
user_pref("mail.identity.id1.fullName", "Ada Lovelace");
user_pref("mail.identity.id1.smtpServer", "smtp1");

user_pref("mail.account.account2.server", "server2");
user_pref("mail.account.account2.identities", "id2");
user_pref("mail.server.server2.type", "imap");
user_pref("mail.server.server2.hostname", "mail.example.net");
user_pref("mail.server.server2.socketType", 2);
user_pref("mail.server.server2.userName", "shortname");
user_pref("mail.server.server2.name", "Club");
user_pref("mail.identity.id2.useremail", "ada@example.net");
user_pref("mail.identity.id2.smtpServer", "smtp2");

user_pref("mail.account.account3.server", "server3");
user_pref("mail.account.account3.identities", "id3");
user_pref("mail.server.server3.type", "imap");
user_pref("mail.server.server3.hostname", "imap.example.com");
user_pref("mail.server.server3.port", 993);
user_pref("mail.server.server3.socketType", 3);
user_pref("mail.server.server3.authMethod", 10);
user_pref("mail.server.server3.is_gmail", true);
user_pref("mail.server.server3.userName", "ada@example.com");
user_pref("mail.identity.id3.useremail", "ada@example.com");

user_pref("mail.account.account4.server", "server4");
user_pref("mail.account.account4.identities", "id4");
user_pref("mail.server.server4.type", "imap");
user_pref("mail.server.server4.hostname", "localhost");
user_pref("mail.server.server4.port", 1143);
user_pref("mail.server.server4.socketType", 0);
user_pref("mail.server.server4.userName", "ada@first.example");
user_pref("mail.identity.id4.useremail", "ada@first.example");

user_pref("mail.account.account5.server", "server5");
user_pref("mail.account.account5.identities", "id5");
user_pref("mail.server.server5.type", "imap");
user_pref("mail.server.server5.hostname", "localhost");
user_pref("mail.server.server5.port", 1143);
user_pref("mail.server.server5.socketType", 0);
user_pref("mail.server.server5.userName", "ada@second.example");
user_pref("mail.identity.id5.useremail", "ada@second.example");

user_pref("mail.smtpservers", "smtp1,smtp2");
user_pref("mail.smtp.defaultserver", "smtp1");
user_pref("mail.smtpserver.smtp1.hostname", "smtp.example.org");
user_pref("mail.smtpserver.smtp1.port", 465);
user_pref("mail.smtpserver.smtp1.try_ssl", 3);
user_pref("mail.smtpserver.smtp1.username", "ada@example.org");

user_pref("mail.smtpserver.smtp2.hostname", "smtp.example.net");
user_pref("mail.smtpserver.smtp2.port", 587);
user_pref("mail.smtpserver.smtp2.try_ssl", 2);
user_pref("mail.smtpserver.smtp2.username", "sender");

user_pref("mail.account.account9.server", "server9");
user_pref("mail.server.server9.type", "none");
user_pref("mail.server.server9.hostname", "Local Folders");
user_pref("mail.server.server9.userName", "nobody");
"#;

    fn imported() -> (Vec<Account>, Vec<Skipped>) {
        collect(&Prefs::parse(PROFILE))
    }

    #[test]
    fn every_imap_account_arrives_and_local_folders_does_not() {
        let (accounts, skipped) = imported();
        let ids: Vec<&str> = accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "example-org",
                "example-net",
                "example-com",
                "first-example",
                "second-example"
            ]
        );
        // Local Folders is a pseudo-account, not something the user meant to
        // import, so it is dropped without a complaint to read.
        assert!(skipped.is_empty(), "unexpected: {skipped:?}");
    }

    #[test]
    fn two_mailboxes_behind_one_bridge_keep_separate_ids() {
        // Both are localhost:1143. An id taken from the host would collide on
        // the one field that must not.
        let (accounts, _) = imported();
        let bridged: Vec<&Account> = accounts.iter().filter(|a| a.host == "localhost").collect();
        assert_eq!(bridged.len(), 2);
        assert_ne!(bridged[0].id, bridged[1].id);
        assert!(bridged.iter().all(|a| a.security == Security::None));
        assert!(bridged.iter().all(|a| a.port == Some(1143)));
    }

    #[test]
    fn a_port_that_is_already_the_default_is_left_out() {
        let (accounts, _) = imported();
        let tls = accounts.iter().find(|a| a.id == "example-org").unwrap();
        assert_eq!(tls.security, Security::Tls);
        // Thunderbird states 993; that is what this adapter uses for `tls`
        // anyway, so writing it down would only be one more thing to keep true.
        assert_eq!(tls.port, None);

        let starttls = accounts.iter().find(|a| a.id == "example-net").unwrap();
        assert_eq!(starttls.security, Security::Starttls);
        assert_eq!(starttls.port, None);
    }

    /// Submission is read from the identity's own `smtpServer`, with its port
    /// dropped when it already is this adapter's default — 465 for `tls`,
    /// which is a different table from reading's 993 and the reason the two
    /// defaults are not shared.
    #[test]
    fn the_outgoing_server_of_an_identity_is_imported() {
        let (accounts, _) = imported();
        let smtp = accounts
            .iter()
            .find(|a| a.id == "example-org")
            .unwrap()
            .smtp
            .as_ref()
            .expect("the identity names an outgoing server");
        assert_eq!(smtp.host, "smtp.example.org");
        assert_eq!(smtp.security, Security::Tls);
        assert_eq!(smtp.port, None);
        assert_eq!(smtp.from_name.as_deref(), Some("Ada Lovelace"));
        // Same login as reading, so no second credential block: one password,
        // one place to change it.
        assert_eq!(smtp.username, None);
    }

    /// The Club account submits as `sender` while it reads as `shortname` —
    /// the case that needs a credential block of its own, and the case a
    /// silent reuse would turn into a rejected login nobody can explain.
    #[test]
    fn a_submission_login_of_its_own_is_kept_and_said_out_loud() {
        let (accounts, _) = imported();
        let club = accounts.iter().find(|a| a.id == "example-net").unwrap();
        let smtp = club
            .smtp
            .as_ref()
            .expect("the Club identity submits somewhere");
        assert_eq!(smtp.username.as_deref(), Some("sender"));
        assert_eq!(smtp.security, Security::Starttls);
        assert_eq!(smtp.port, None, "587 is the starttls default already");
        assert!(
            club.notes
                .iter()
                .any(|n| n.contains("submission logs in as")),
            "notes: {:?}",
            club.notes
        );
    }

    /// An identity that names no server falls back to Thunderbird's default
    /// one — the two bridged mailboxes do exactly that, and an account that
    /// reads but cannot answer would be a silent downgrade.
    #[test]
    fn an_identity_without_a_server_of_its_own_falls_back_to_the_default() {
        let (accounts, _) = imported();
        let bridged = accounts.iter().find(|a| a.id == "first-example").unwrap();
        let smtp = bridged.smtp.as_ref().expect("the default server applies");
        assert_eq!(smtp.host, "smtp.example.org");
    }

    #[test]
    fn a_login_name_that_is_not_the_address_is_carried_and_pointed_at() {
        let (accounts, _) = imported();
        let acc = accounts.iter().find(|a| a.id == "example-net").unwrap();
        assert_eq!(acc.username, "shortname");
        assert_eq!(acc.address.as_deref(), Some("ada@example.net"));
        assert!(
            acc.notes.iter().any(|n| n.contains("not the address")),
            "the difference has to be visible, not just correct: {:?}",
            acc.notes
        );
    }

    #[test]
    fn an_oauth_account_says_it_needs_an_app_password() {
        let (accounts, _) = imported();
        let acc = accounts.iter().find(|a| a.id == "example-com").unwrap();
        assert!(acc.notes.iter().any(|n| n.contains("APP PASSWORD")));
    }

    #[test]
    fn a_pop_account_is_named_rather_than_dropped_in_silence() {
        // Silently importing nothing is how a mailbox goes missing without
        // anybody noticing, so a server we cannot speak to is reported.
        let prefs = Prefs::parse(
            r#"user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.server.server1.type", "pop3");
user_pref("mail.server.server1.hostname", "pop.example.org");
user_pref("mail.server.server1.name", "Old mail");"#,
        );
        let (accounts, skipped) = collect(&prefs);
        assert!(accounts.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].label, "Old mail");
        assert!(skipped[0].reason.contains("pop3"));
    }

    #[test]
    fn a_profile_without_a_socket_type_derives_one_and_admits_it() {
        let prefs = Prefs::parse(
            r#"user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.server.server1.port", 143);
user_pref("mail.server.server1.userName", "ada");"#,
        );
        let (accounts, _) = collect(&prefs);
        assert_eq!(accounts[0].security, Security::Starttls);
        assert!(
            accounts[0]
                .notes
                .iter()
                .any(|n| n.contains("derived from the port"))
        );
    }
}
