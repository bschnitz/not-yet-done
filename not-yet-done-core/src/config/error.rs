use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    ConfigNotFound(String),

    #[error("Failed to read config file: {0}")]
    ReadError(#[source] std::io::Error),

    #[error("Failed to parse config: {0}")]
    ParseError(#[source] serde_yaml::Error),

    #[error("Failed to write config file: {0}")]
    WriteError(#[source] std::io::Error),

    #[error("Failed to create config directory: {0}")]
    DirectoryError(#[source] std::io::Error),

    #[error("User declined to create config file")]
    CreationDeclined,
}

impl ConfigError {
    pub fn kind(&self) -> ConfigErrorKind {
        match self {
            ConfigError::ConfigNotFound(_) => ConfigErrorKind::NotFound,
            ConfigError::ReadError(_)
            | ConfigError::WriteError(_)
            | ConfigError::DirectoryError(_) => ConfigErrorKind::Io,
            ConfigError::ParseError(_) => ConfigErrorKind::Parse,
            ConfigError::CreationDeclined => ConfigErrorKind::UserDeclined,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigErrorKind {
    NotFound,
    Io,
    Parse,
    UserDeclined,
}
