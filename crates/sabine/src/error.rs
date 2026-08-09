use sabine_runtime::RuntimeError;
use std::{io, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SabineError {
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("failed to read Sabine manifest at {path}: {source}")]
    ManifestRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse Sabine manifest at {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("window creation failed: {message}")]
    CreationFailed { message: String },
    #[error("another instance is already running")]
    InstanceAlreadyRunning,
    #[error("Sabine currently supports Linux, macOS, and Windows")]
    MobileUnsupported,
}

pub type SabineResult<T> = std::result::Result<T, SabineError>;
