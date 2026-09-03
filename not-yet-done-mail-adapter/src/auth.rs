//! What this adapter can speak against an IMAP server.
//!
//! The factory publishes this table (so `nyd config auth mail` and the
//! `nyd config build mail` wizard render it) and validates every account's
//! `auth:` block against it. [`crate::imap::login`] implements it — the two
//! belong together: a new mechanism is an entry here plus a branch there.

use not_yet_done_content::{AuthFieldSpec, MechanismSpec};

pub const MECHANISMS: &[MechanismSpec] = &[
    MechanismSpec {
        id: "password",
        label: "Username and password",
        doc: "Log in with the mailbox's login name and password (IMAP LOGIN, or \
              AUTHENTICATE PLAIN where the server prefers it). The login name is \
              often but not always the e-mail address — use whatever the mail \
              provider asks for.",
        fields: &[
            AuthFieldSpec::required("username", "Login name", false),
            AuthFieldSpec::required("password", "Password", true),
        ],
    },
    MechanismSpec {
        id: "xoauth2",
        label: "OAuth 2 bearer token (XOAUTH2)",
        doc: "Log in with an OAuth access token (AUTHENTICATE XOAUTH2). The \
              adapter performs no OAuth flow itself: the token is an ordinary \
              credential, so a `command:` provider running a refresh script is \
              what keeps it fresh.",
        fields: &[
            AuthFieldSpec::required("username", "E-mail address", false),
            AuthFieldSpec::required("token", "Access token", true),
        ],
    },
];
