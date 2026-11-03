use std::path::PathBuf;

/// Error type shared by all core modules.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("workspace is in use by another instance (pid {pid} on {host})")]
    WorkspaceLocked { pid: u32, host: String },
    #[error("invalid workspace path: {0}")]
    InvalidWorkspace(String),
    #[error("no workspace is open")]
    NoWorkspace,
    #[error("unknown model id: {0}")]
    UnknownModel(String),
    #[error("invalid value: {0}")]
    Invalid(String),
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io { path: path.into(), source }
    }
    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        CoreError::Json { path: path.into(), source }
    }
    /// Stable machine-readable code for the UI (i18n lookup key).
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::Io { .. } => "io",
            CoreError::Json { .. } => "json",
            CoreError::WorkspaceLocked { .. } => "workspace_locked",
            CoreError::InvalidWorkspace(_) => "invalid_workspace",
            CoreError::NoWorkspace => "no_workspace",
            CoreError::UnknownModel(_) => "unknown_model",
            CoreError::Invalid(_) => "invalid",
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
