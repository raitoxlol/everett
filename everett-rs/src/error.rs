use std::fmt;

/// One error type for every Everett failure that maps to a CLI exit code.
/// Codes match the Python implementation: 2 bad input, 3 no Jev key,
/// 4 busy/locked, 5 harness start/timeout, 6 harness failure, 7 hop limit.
#[derive(Debug, Clone)]
pub struct EverettError {
    pub code: i32,
    pub message: String,
}

impl EverettError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        EverettError { code, message: message.into() }
    }
}

impl fmt::Display for EverettError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for EverettError {}

pub type Result<T> = std::result::Result<T, EverettError>;
