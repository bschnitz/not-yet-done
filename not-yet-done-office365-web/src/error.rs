//! Error type shared across the session, registry and domain APIs.

/// Failure of an Office 365 web session operation.
#[derive(Debug, thiserror::Error)]
pub enum MsOfficeError {
    /// The session is not authenticated and interactive MFA is required. The
    /// sidecar opens a visible browser window for the user to complete login;
    /// callers should surface this and retry afterwards.
    #[error("interactive login required")]
    LoginRequired,

    /// The sidecar process failed (spawn, I/O, or a reported operation error).
    #[error("sidecar error: {0}")]
    Sidecar(String),

    /// A request to the sidecar did not complete within the configured timeout.
    #[error("sidecar request timed out")]
    Timeout,

    /// The sidecar spoke, but not in the shape we expected.
    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("{0}")]
    Other(String),
}
