#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Config(String),
    #[error("path escapes repo root: {0}")]
    PathEscape(String),
    #[error("tags: {0}")]
    Tags(String),
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
}

pub type Result<T> = std::result::Result<T, Error>;
