//! Handing the message to a server.
//!
//! Reading mail and sending it are two protocols against two hosts, and this
//! is the second one: SMTP submission, built per send rather than kept open.
//! A mailbox is read all day and written a few times an hour, so a pooled
//! connection would mostly be an idle socket the provider closes anyway.
//!
//! What happens *after* the send — the copy in Sent, the `\Answered` flag —
//! is IMAP again and lives in [`crate::imap::ops`]. The order matters: this
//! step is the irreversible one, so nothing that can fail runs before it.

use std::collections::HashMap;

use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::{Security, SmtpConfig};
use crate::error::{MailError, MailResult};

/// Submit one message.
///
/// `mechanism` and `fields` are the account's resolved credentials — the
/// same shape [`crate::imap::login`] gets, because in the common case they
/// *are* the same credentials: one provider, one login, two ports.
pub(crate) async fn send(
    smtp: &SmtpConfig,
    mechanism: &str,
    fields: &HashMap<String, String>,
    message: &Message,
) -> MailResult<()> {
    let transport = transport(smtp, mechanism, fields)?;
    transport.send(message.clone()).await.map_err(classify)?;
    Ok(())
}

fn transport(
    smtp: &SmtpConfig,
    mechanism: &str,
    fields: &HashMap<String, String>,
) -> MailResult<AsyncSmtpTransport<Tokio1Executor>> {
    // `builder_dangerous` only means "no TLS unless you say so" — which is
    // exactly what an explicit `security:` is for. Every mode below says so.
    let mut builder =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host).port(smtp.port());
    builder = match smtp.security {
        Security::None => builder,
        mode => {
            let params = TlsParameters::builder(smtp.host.clone())
                .dangerous_accept_invalid_certs(smtp.accept_invalid_certs)
                .dangerous_accept_invalid_hostnames(smtp.accept_invalid_certs)
                .build()
                .map_err(|e| MailError::Config(format!("smtp tls for {}: {e}", smtp.host)))?;
            match mode {
                // Implicit TLS: the handshake happens before the greeting.
                Security::Tls => builder.tls(Tls::Wrapper(params)),
                // `Required`, never `Opportunistic`: a submission server that
                // does not offer STARTTLS is one that would take the password
                // in the clear.
                _ => builder.tls(Tls::Required(params)),
            }
        }
    };

    let (user, secret, mechanisms) = match mechanism {
        "password" => (
            field(fields, "username")?,
            field(fields, "password")?,
            vec![Mechanism::Plain, Mechanism::Login],
        ),
        "xoauth2" => (
            field(fields, "username")?,
            field(fields, "token")?,
            vec![Mechanism::Xoauth2],
        ),
        other => {
            return Err(MailError::Config(format!(
                "unsupported smtp auth mechanism `{other}`"
            )));
        }
    };
    Ok(builder
        .credentials(Credentials::new(user, secret))
        .authentication(mechanisms)
        .build())
}

fn field(fields: &HashMap<String, String>, name: &str) -> MailResult<String> {
    fields
        .get(name)
        .cloned()
        .ok_or_else(|| MailError::Auth(format!("the credential `{name}` was not resolved")))
}

/// Which kind of failure this was — the same three questions the IMAP side
/// asks, and the same consequences: only a refused login may invalidate the
/// cached password, and only a transport failure is worth another attempt.
fn classify(e: lettre::transport::smtp::Error) -> MailError {
    let code = e.status().map(|c| c.to_string()).unwrap_or_default();
    // 530 (authentication required), 534 and 535 (rejected) are the whole
    // set a submission server answers a bad login with.
    if matches!(code.as_str(), "530" | "534" | "535") {
        return MailError::Auth(format!("the server refused the login ({code}): {e}"));
    }
    if e.is_permanent() {
        return MailError::Server(format!("the server refused the message: {e}"));
    }
    MailError::Transport(format!("the message could not be handed over: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Security;

    fn smtp(security: Security) -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.invalid".into(),
            port: None,
            security,
            accept_invalid_certs: false,
            auth: None,
            from_name: None,
        }
    }

    fn password() -> HashMap<String, String> {
        HashMap::from([
            ("username".to_string(), "u".to_string()),
            ("password".to_string(), "p".to_string()),
        ])
    }

    /// Building the transport is where a bad mechanism or a missing
    /// credential is caught — before anything reaches the wire.
    #[test]
    fn every_security_mode_builds_a_transport() {
        for mode in [Security::Tls, Security::Starttls, Security::None] {
            transport(&smtp(mode), "password", &password()).expect("builds");
        }
    }

    #[test]
    fn a_mechanism_the_adapter_cannot_speak_is_named_in_the_error() {
        let error = transport(&smtp(Security::Tls), "gssapi", &password())
            .expect_err("not a mechanism we have");
        assert!(error.to_string().contains("gssapi"), "{error}");
    }

    #[test]
    fn a_credential_the_provider_did_not_deliver_is_named_too() {
        let error =
            transport(&smtp(Security::Tls), "xoauth2", &password()).expect_err("no token in there");
        assert!(error.to_string().contains("token"), "{error}");
    }
}
