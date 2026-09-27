//! Petramond account identity: the player's sign-in with the account service,
//! and the join credential a game server redeems to learn who is connecting.
//!
//! Two credentials, and the split is the whole design:
//!
//! - The **sign-in token** is long-lived, rotating, and lives only on the
//!   player's machine ([`store`]). It is what the client trades a password for
//!   once, and what it silently refreshes forever after.
//! - A **join ticket** is single-use, expires in about two minutes, and is
//!   bound to the opaque `server_id` the joining handshake offered. That is the
//!   only credential a game server ever sees, so a hostile server learns
//!   nothing it can replay and can never refresh the player's sign-in.
//!
//! Every call here BLOCKS on the network, so every caller runs it on a worker
//! thread: the client's connect/sign-in workers, and the server's per-join
//! verification thread. Nothing on a tick path or a render path speaks HTTP.

use std::time::{SystemTime, UNIX_EPOCH};

mod api;
mod http;
pub mod session;
pub mod store;

#[cfg(test)]
mod tests;

pub use api::{verify_join, SignedIn};
/// How long one call to the account service may take.
pub use http::TIMEOUT as SERVICE_TIMEOUT;
pub use store::SavedSignIn;

/// Who the account service says somebody is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountIdentity {
    pub user_id: i64,
    /// The account's username — the player name the game uses for this session.
    pub username: String,
    /// Site-relative URL of the account's avatar image.
    pub avatar_url: String,
    /// Site-relative URL of the account's public profile page.
    pub profile_url: String,
}

/// Why an account call did not produce an identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountError {
    /// The credential is worthless and no retry will help — the caller CLEARS
    /// the stored sign-in so the player is offered the sign-in form again.
    SignInRequired(String),
    /// The service understood and refused (wrong password, unverified email,
    /// disabled account, too many attempts). The stored sign-in, if any, stands.
    Refused(String),
    /// The service could not be reached or could not be understood.
    Unreachable(String),
}

impl AccountError {
    /// Whether the caller must discard the stored sign-in.
    pub fn clears_sign_in(&self) -> bool {
        matches!(self, AccountError::SignInRequired(_))
    }

    pub fn message(&self) -> &str {
        match self {
            AccountError::SignInRequired(m)
            | AccountError::Refused(m)
            | AccountError::Unreachable(m) => m,
        }
    }
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Whether a server demands a Petramond account of the players joining it.
///
/// `Offline` exists for private servers and for the test suite: the join then
/// carries a plain name and nothing contacts the account service. It is NOT a
/// fallback — a server is one or the other, and the handshake tells the client
/// which before it offers any credential.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountPolicy {
    #[default]
    Online,
    Offline,
}

impl AccountPolicy {
    /// `PETRAMOND_ONLINE_MODE=0|false|off` opts a server out of account checks.
    pub fn from_env() -> AccountPolicy {
        match std::env::var("PETRAMOND_ONLINE_MODE") {
            Ok(v)
                if matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off"
                ) =>
            {
                AccountPolicy::Offline
            }
            _ => AccountPolicy::Online,
        }
    }

    pub fn requires_account(self) -> bool {
        self == AccountPolicy::Online
    }
}

/// The account service's origin: `PETRAMOND_ACCOUNT_URL` (for a local website
/// checkout) or the live site. A trailing slash is trimmed so paths append
/// cleanly.
pub fn service_url() -> String {
    crate::service::origin()
}

/// How this build identifies itself in the player's list of signed-in clients.
pub fn client_label() -> String {
    format!(
        "Petramond {} / {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// Wall-clock seconds since the epoch. Token lifetimes arrive as DURATIONS and
/// are banked against this, so a wrong machine clock costs nothing until it is
/// changed mid-session.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A fresh opaque id for one running server, offered in the join handshake and
/// presented again when the server redeems a ticket. Generated per process: a
/// ticket only has to outlive the join that minted it, so nothing persists and
/// nothing has to agree across restarts.
pub fn new_server_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("the OS provides randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
