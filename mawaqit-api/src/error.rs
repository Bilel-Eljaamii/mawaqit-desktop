use thiserror::Error;

#[derive(Debug, Error)]
pub enum MawaqitError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("mosque not found: {0}")]
    MosqueNotFound(String),

    #[error("confData not found in the page of {0}: the page layout may have changed")]
    ConfDataNotFound(String),

    #[error("month must be between 1 and 12, got {0}")]
    InvalidMonth(u32),

    #[error("no calendar data for this mosque")]
    NoCalendar,

    #[error("unexpected response (HTTP {status}) from {url}")]
    Api { status: u16, url: String },

    #[error("malformed payload: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, MawaqitError>;
