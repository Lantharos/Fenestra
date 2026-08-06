use sabine_runtime::RuntimeError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SabineError {
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("{0}")]
    Io(String),
    #[error("window creation failed: {message}")]
    CreationFailed { message: String },
    #[error("Sabine currently supports Linux, macOS, and Windows")]
    MobileUnsupported,
}

pub type SabineResult<T> = std::result::Result<T, SabineError>;
