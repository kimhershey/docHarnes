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
    #[error("invalid GGUF file {path}: {reason}")]
    Gguf { path: PathBuf, reason: String },
    #[error("engine binary not found: {0}")]
    EngineNotFound(String),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("engine is not running")]
    EngineNotRunning,
    #[error("download failed: {0}")]
    Download(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("database error: {0}")]
    Db(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("conversion failed: {0}")]
    Convert(String),
    #[error("render failed: {0}")]
    Render(String),
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io { path: path.into(), source }
    }
    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        CoreError::Json { path: path.into(), source }
    }
    pub fn gguf(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        CoreError::Gguf { path: path.into(), reason: reason.into() }
    }
    pub fn engine(reason: impl Into<String>) -> Self {
        CoreError::Engine(reason.into())
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
            CoreError::Gguf { .. } => "invalid_gguf",
            CoreError::EngineNotFound(_) => "engine_not_found",
            CoreError::Engine(_) => "engine",
            CoreError::EngineNotRunning => "engine_not_running",
            CoreError::Download(_) => "download",
            CoreError::Cancelled => "cancelled",
            CoreError::Timeout(_) => "timeout",
            CoreError::Db(_) => "db",
            CoreError::NotFound(_) => "not_found",
            CoreError::Unsupported(_) => "unsupported",
            CoreError::Convert(_) => "convert",
            CoreError::Render(_) => "render",
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
