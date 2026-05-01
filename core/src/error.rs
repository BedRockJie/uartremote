use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("authentication failed")]
    AuthFailed,

    #[error("writer is already claimed by {0}")]
    WriterAlreadyClaimed(String),

    #[error("writer is not owned by {0}")]
    WriterNotOwned(String),

    #[error("serial error: {0}")]
    Serial(String),

    #[error("invalid protocol frame: {0}")]
    InvalidFrame(String),
}
