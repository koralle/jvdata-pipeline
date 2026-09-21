use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("record too short: need at least {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },

    #[error("unknown record spec: {0:?}")]
    UnknownSpec(String),

    #[error("invalid numeric field at offset {offset}: {raw:?}")]
    InvalidNumeric { offset: usize, raw: String },

    #[error("invalid date field at offset {offset}: {raw:?}")]
    InvalidDate { offset: usize, raw: String },
}
