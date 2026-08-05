use mullion_runtime::RuntimeError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MullionError {
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("{0}")]
    Io(String),
    #[error("window creation failed: {message}")]
    CreationFailed { message: String },
    #[error("Mullion currently supports Linux, macOS, and Windows")]
    MobileUnsupported,
}

pub type MullionResult<T> = std::result::Result<T, MullionError>;
