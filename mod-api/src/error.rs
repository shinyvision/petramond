use core::fmt;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    WrongSide,
    NotInInit,
    NoContext,
    ReadOnly,
    NoActor,
    Forbidden,
    InvalidArgument,
    LimitExceeded,
    Refused,
}

impl ErrorCode {
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::LimitExceeded | Self::Refused)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub code: ErrorCode,
    pub detail: String,
}

impl HostError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for HostError {}

impl From<HostError> for String {
    fn from(error: HostError) -> Self {
        error.detail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_data_bounds_and_refusals_are_recoverable() {
        assert!(ErrorCode::LimitExceeded.is_recoverable());
        assert!(ErrorCode::Refused.is_recoverable());
        for code in [
            ErrorCode::WrongSide,
            ErrorCode::NotInInit,
            ErrorCode::NoContext,
            ErrorCode::ReadOnly,
            ErrorCode::NoActor,
            ErrorCode::Forbidden,
            ErrorCode::InvalidArgument,
        ] {
            assert!(!code.is_recoverable(), "{code:?}");
        }
    }

    #[test]
    fn display_names_the_code_and_the_detail() {
        let e = HostError::new(ErrorCode::LimitExceeded, "KV value is 70000 bytes");
        assert_eq!(e.to_string(), "LimitExceeded: KV value is 70000 bytes");
    }
}
