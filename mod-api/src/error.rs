//! The host's typed refusal: what a host call answers when it cannot do what
//! it was asked.
//!
//! One channel for every call. A refusal carries an [`ErrorCode`] a guest can
//! branch on and a human-readable detail for the log. The SDK surfaces it as
//! a `Result` on the wrappers whose failure depends on DATA (a value grown
//! past its cap, a batch built from player-driven input), so a mod can
//! recover — shard the value, split the batch, log and skip — instead of
//! being disabled for the session. On the remaining wrappers a refusal is a
//! mod bug (a registration outside `mod_init`, a sim call from a client
//! module, a malformed argument) and the SDK panics, which traps and
//! disables the mod. A call the host cannot decode at all is a broken peer
//! and traps host-side; that is the only fatal condition outside the guest's
//! own choice.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Why a host call was refused.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The call is not legal on this instance side (a simulation call from a
    /// client module, a client call from the server...) — see
    /// [`Legality::sides`](crate::Legality::sides).
    WrongSide,
    /// A registration outside the `mod_init` window.
    NotInInit,
    /// A sim-scoped call with no dispatch context active.
    NoContext,
    /// A mutating call inside a read-only dispatch (a shape placement plan).
    ReadOnly,
    /// A call addressing "the acting player" in a dispatch that has none (a
    /// tick system, block hook, spawn pick, `mod_init` or mob action); name
    /// the player explicitly instead.
    NoActor,
    /// A write outside the caller's namespace (a KV key, an event key, a
    /// behavior key another mod owns).
    Forbidden,
    /// A malformed argument: an unregistered id, a non-finite float, an
    /// invalid key, inverted bounds, a shape the host cannot honour.
    InvalidArgument,
    /// A bound on the call's data was exceeded — a batch longer than
    /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX), a KV value, key or cell key
    /// count past its cap, an event payload past
    /// [`EVENT_MAX_DATA_BYTES`](crate::EVENT_MAX_DATA_BYTES). Recoverable:
    /// split or shard and retry.
    LimitExceeded,
}

impl ErrorCode {
    /// Whether a mod can reasonably recover from this refusal at runtime —
    /// the failure depends on the data it was handed, not on how the mod was
    /// written.
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::LimitExceeded)
    }
}

/// A typed host refusal ([`HostRet::Err`](crate::HostRet::Err)).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_data_bounds_are_recoverable() {
        assert!(ErrorCode::LimitExceeded.is_recoverable());
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
