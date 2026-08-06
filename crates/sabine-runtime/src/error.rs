use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime not found: {0}")]
    NotFound(String),
    #[error("runtime at {path} has version {found}, minimum required is {required}")]
    VersionTooLow {
        path: PathBuf,
        found: String,
        required: String,
    },
    #[error("runtime integrity check failed for {path}")]
    IntegrityFailed { path: PathBuf },
    #[error("runtime installation failed: {0}")]
    InstallationFailed(String),
    #[error("runtime downloads are disabled by configuration")]
    DownloadsDisabled,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
