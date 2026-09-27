pub mod http;

#[cfg(test)]
mod tests;

pub use http::{ServiceError, Stream, Timeouts, TransportError};

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
