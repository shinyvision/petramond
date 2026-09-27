use std::time::{SystemTime, UNIX_EPOCH};

mod api;
mod http;
pub mod session;
pub mod store;

#[cfg(test)]
mod tests;

pub use api::{verify_join, SignedIn};
pub use http::TIMEOUT as SERVICE_TIMEOUT;
pub use store::SavedSignIn;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountIdentity {
    pub user_id: i64,
    pub username: String,
    pub avatar_url: String,
    pub profile_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountError {
    SignInRequired(String),
    Refused(String),
    Unreachable(String),
}

impl AccountError {
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountPolicy {
    #[default]
    Online,
    Offline,
}

impl AccountPolicy {
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

pub fn service_url() -> String {
    crate::service::origin()
}

pub fn client_label() -> String {
    format!(
        "Petramond {} / {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn new_server_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("the OS provides randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
