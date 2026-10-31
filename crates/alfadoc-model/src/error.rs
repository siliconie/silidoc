use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not an .alfadoc file: {0}")]
    NotAlfadoc(String),
    #[error("unsafe or invalid entry name in package: {0}")]
    BadEntryName(String),
    #[error("package limit exceeded: {0}")]
    Limit(String),
    #[error("package is missing the required entry {0}")]
    MissingEntry(&'static str),
    #[error("unknown sheet id {0}")]
    UnknownSheet(u16),
    #[error("dimension vector table is full for one canonical vector (256 collisions)")]
    DimCollisionOverflow,
    #[error("this file is protected with a passphrase")]
    Encrypted,
    #[error("wrong passphrase, or the file was changed")]
    WrongPassphrase,
    #[error("security: {0}")]
    Security(String),
    #[error("damaged data pages: {0}")]
    Paged(String),
    #[error("zip error: {0}")]
    Zip(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::Zip(e.to_string())
    }
}
