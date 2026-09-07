//! Error type shared across the session, registry and domain APIs.

/// Failure of an Office 365 web session operation.
#[derive(Debug, thiserror::Error)]
pub enum MsOfficeError {
    /// The browser could not be started or spoken to: it did not start, it closed its
    /// socket, or it said it could not do what was asked.
    #[error("browser error: {0}")]
    Browser(String),

    /// The flow ran and did not get where it was going — a claim that did not hold, a
    /// sign-on that gave up, a run somebody stopped. The sentence is the run's own.
    #[error("the run failed: {0}")]
    Run(String),

    /// The browser did not answer, or a run did not end, within the configured time.
    #[error("the browser did not answer in time")]
    Timeout,

    /// The browser spoke, but not in the shape we expected.
    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("{0}")]
    Other(String),
}
