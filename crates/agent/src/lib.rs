pub mod config;
pub mod connection;
pub mod metrics;
pub mod security;
pub mod store;
pub mod systemd;
pub mod xray;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentError {
    PrivilegeDenied,
    Configuration,
    Storage,
    Connection,
    Authentication,
    Protocol,
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PrivilegeDenied => {
                "Agent must run as an unprivileged user without privilege escalation"
            }
            Self::Configuration => "Invalid agent configuration or private data path",
            Self::Storage => "Agent identity/journal storage failed or is already locked",
            Self::Connection => "Agent connection failed; credentials and URLs are redacted",
            Self::Authentication => "Agent credential is invalid, expired or revoked; enroll again",
            Self::Protocol => "Agent protocol, identity or task validation failed",
        })
    }
}
impl std::error::Error for AgentError {}
impl From<sqlx::Error> for AgentError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}

pub mod traffic;
