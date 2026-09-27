//! petramond.com as a service the game talks to: one blocking HTTP transport
//! that every client of the site shares (the account, the content library),
//! each of which classifies the answers its own way.
//!
//! Every call BLOCKS on the network, so callers run them on worker threads;
//! nothing on a tick path or a render path speaks HTTP.

pub mod http;

#[cfg(test)]
mod tests;

pub use http::{ServiceError, Stream, Timeouts, TransportError};

/// The site's origin: `PETRAMOND_ACCOUNT_URL` (a local website checkout) or
/// the live site, without a trailing slash so paths append cleanly.
pub fn origin() -> String {
    let raw = std::env::var("PETRAMOND_ACCOUNT_URL").unwrap_or_default();
    let raw = raw.trim();
    let base = if raw.is_empty() {
        "https://petramond.com"
    } else {
        raw
    };
    base.trim_end_matches('/').to_owned()
}
