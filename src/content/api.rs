//! petramond.com's content endpoints: the listing, a pack's icon and its
//! download, and how the content library reads their answers. No policy:
//! what to do with a busy site or a changed package is the caller's.
//!
//! The client trusts nothing a listing says: every row is validated and its
//! text cleaned, and no host other than the site's own is ever followed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::Kind;
use crate::service::{ServiceError, Timeouts};

/// The largest archive the site serves.
pub const MAX_ARCHIVE: u64 = super::archive::MAX_BYTES as u64;

/// A download's budgets: a stalled connection fails in 30 s; the whole body
/// has ten minutes (20 MiB at a poor 35 KiB/s).
pub const DOWNLOAD: Timeouts = Timeouts {
    connect: Duration::from_secs(10),
    headers: Duration::from_secs(30),
    idle_read: Duration::from_secs(30),
    total: Duration::from_secs(600),
};

/// An icon is small and public.
const ICON: Timeouts = Timeouts {
    connect: Duration::from_secs(10),
    headers: Duration::from_secs(30),
    idle_read: Duration::from_secs(30),
    total: Duration::from_secs(60),
};

/// Retry-After bounds, and the wait when it says nothing usable.
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(300);
const RETRY_DEFAULT: Duration = Duration::from_secs(10);

/// One downloadable pack, as the listing describes it, validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingRow {
    pub content_id: i64,
    pub mod_id: String,
    pub kind: Kind,
    pub name: String,
    pub summary: String,
    pub description: String,
    pub version: String,
    pub byte_size: u64,
    pub sha256: String,
    pub download_path: String,
    pub icon_path: Option<String>,
}

/// Control characters stripped (a description keeps its line breaks), then
/// cut to `max` characters.
pub fn clean(text: &str, max: usize, keep_line_breaks: bool) -> String {
    text.chars()
        .filter(|c| !c.is_control() || (keep_line_breaks && *c == '\n'))
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn valid_mod_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn site_path(path: &str) -> bool {
    path.starts_with("/api/v1/content/")
}

/// One listing row, or why it is skipped.
fn row(item: &Value) -> Result<ListingRow, String> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
    let mod_id = text("modId");
    if !valid_mod_id(mod_id) {
        return Err(format!("invalid modId '{mod_id}'"));
    }
    let kind = match text("kind") {
        "addon" => Kind::Addon,
        "mod" => Kind::Mod,
        other => {
            return Err(format!(
                "'{mod_id}' has a kind this build does not know: {other}"
            ))
        }
    };
    let byte_size = item.get("byteSize").and_then(Value::as_u64).unwrap_or(0);
    if !(1..=MAX_ARCHIVE).contains(&byte_size) {
        return Err(format!("'{mod_id}' has an impossible size {byte_size}"));
    }
    let sha256 = text("sha256").to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("'{mod_id}' has no usable sha256"));
    }
    let download_path = text("downloadPath");
    if !site_path(download_path) {
        return Err(format!("'{mod_id}' names a download off the site"));
    }
    let icon_path = item
        .get("iconPath")
        .and_then(Value::as_str)
        .filter(|p| site_path(p))
        .map(str::to_owned);
    Ok(ListingRow {
        content_id: item.get("id").and_then(Value::as_i64).unwrap_or(0),
        mod_id: mod_id.to_owned(),
        kind,
        name: clean(text("name"), 128, false),
        summary: clean(text("summary"), 200, false),
        description: clean(text("description"), 2000, true),
        version: clean(text("version"), 12, false),
        byte_size,
        sha256,
        download_path: download_path.to_owned(),
        icon_path,
    })
}

/// The listing's rows the client will show; each skipped row is logged once.
pub fn parse_listing(body: &Value) -> Vec<ListingRow> {
    let Some(items) = body.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match row(item) {
            Ok(row) => Some(row),
            Err(why) => {
                log::info!("content listing: skipping a row: {why}");
                None
            }
        })
        .collect()
}

/// How the content library reads a non-success answer. ONLY a 429 whose
/// code is `too_many_downloads` is `Busy`; any other refusal is a refusal.
pub fn classify(status: u16, body: &Value, retry_after: Option<&str>) -> ServiceError {
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("petramond.com refused the request")
        .to_owned();
    let code = body.get("code").and_then(Value::as_str);
    let sign_in = body
        .get("signInRequired")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match status {
        _ if sign_in => ServiceError::SignInRequired(message),
        429 if code == Some("too_many_downloads") => ServiceError::Busy {
            retry_after: retry_wait(retry_after, crate::account::now_unix()),
        },
        400..=499 => ServiceError::Refused(message),
        _ => ServiceError::Unreachable(message),
    }
}

/// `Retry-After` as delta-seconds or an HTTP-date, clamped to 1..=300 s;
/// 10 s when absent or unreadable.
pub fn retry_wait(header: Option<&str>, now_unix: i64) -> Duration {
    let Some(value) = header.map(str::trim).filter(|v| !v.is_empty()) else {
        return RETRY_DEFAULT;
    };
    let secs = match value.parse::<u64>() {
        Ok(secs) => Some(secs),
        Err(_) => jiff::fmt::rfc2822::DateTimeParser::new()
            .parse_timestamp(value)
            .ok()
            .map(|at| at.as_second().saturating_sub(now_unix).max(0) as u64),
    };
    secs.map_or(RETRY_DEFAULT, |s| {
        Duration::from_secs(s).clamp(RETRY_MIN, RETRY_MAX)
    })
}

/// The listing, with the stored token (rotated when due).
pub fn listing() -> Result<Vec<ListingRow>, ServiceError> {
    crate::account::session::spend(|token| {
        let (status, body) = crate::service::http::get_json("/api/v1/content", Some(token))
            .map_err(|e| {
                ServiceError::Unreachable(if e.timed_out {
                    "petramond.com did not respond".to_owned()
                } else {
                    format!("Could not reach petramond.com: {}", e.detail)
                })
            })?;
        if (200..300).contains(&status) {
            Ok(parse_listing(&body))
        } else {
            Err(classify(status, &body, None))
        }
    })
}

/// An icon's bytes (public, no token), at most 512 KiB.
pub fn icon(path: &str) -> Result<Vec<u8>, ServiceError> {
    if !site_path(path) {
        return Err(ServiceError::Refused("not a petramond.com icon".into()));
    }
    let stream = crate::service::http::get_stream(path, None, ICON)?;
    if !(200..300).contains(&stream.status) {
        let retry = stream.retry_after.clone();
        let status = stream.status;
        return Err(classify(status, &stream.json(), retry.as_deref()));
    }
    let mut bytes = Vec::new();
    stream
        .take(super::icon::MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ServiceError::Unreachable(format!("the icon did not arrive: {e}")))?;
    if bytes.len() > super::icon::MAX_INPUT {
        return Err(ServiceError::Refused("the icon is too large".into()));
    }
    Ok(bytes)
}

/// Why a download produced no archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// The site answered with a refusal, was busy, or did not answer.
    Service(ServiceError),
    /// 404: the package is no longer on the site.
    Gone,
    /// The package was replaced after the listing was fetched (its length
    /// or hash no longer matches the row).
    Changed,
    /// The body broke off, overran, or does not hash to what was promised.
    Broken(String),
    Cancelled,
}

impl DownloadError {
    pub fn message(&self) -> String {
        match self {
            DownloadError::Service(e) => e.message(),
            DownloadError::Gone => "No longer on petramond.com".into(),
            DownloadError::Changed => "Changed on petramond.com; refresh and try again.".into(),
            DownloadError::Broken(why) => why.clone(),
            DownloadError::Cancelled => "Cancelled".into(),
        }
    }
}

/// Bytes done and expected, for a progress display read every frame.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

/// Download `row`'s archive into `into` (a fresh `.partial` file), hashing
/// as it arrives; `Ok` only for exactly the promised bytes. The partial file
/// is removed on every failure.
pub fn download(
    row: &ListingRow,
    into: &Path,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<PathBuf, DownloadError> {
    let result = crate::account::session::spend(|token| {
        crate::service::http::get_stream(&row.download_path, Some(token), DOWNLOAD).and_then(
            |stream| {
                if (200..300).contains(&stream.status) || stream.status == 404 {
                    Ok(stream)
                } else {
                    let (status, retry) = (stream.status, stream.retry_after.clone());
                    Err(classify(status, &stream.json(), retry.as_deref()))
                }
            },
        )
    });
    let stream = result.map_err(DownloadError::Service)?;
    if stream.status == 404 {
        return Err(DownloadError::Gone);
    }
    if stream.content_length != Some(row.byte_size) || stream.sha256.as_deref() != Some(&row.sha256)
    {
        return Err(DownloadError::Changed);
    }
    let written = receive(stream, row, into, progress, cancel);
    if written.is_err() {
        let _ = std::fs::remove_file(into);
    }
    written.map(|()| into.to_owned())
}

/// Stream the body to `into` with its length and hash checked.
pub fn receive(
    mut body: impl Read,
    row: &ListingRow,
    into: &Path,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    use sha2::Digest;
    if let Some(parent) = into.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DownloadError::Broken(e.to_string()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(into)
        .map_err(|e| DownloadError::Broken(format!("could not save the download: {e}")))?;
    progress.total.store(row.byte_size, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let mut hash = sha2::Sha256::new();
    let mut done = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let n = body
            .read(&mut buffer)
            .map_err(|e| DownloadError::Broken(format!("The download broke off: {e}")))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > row.byte_size {
            return Err(DownloadError::Broken(
                "The download was longer than promised".into(),
            ));
        }
        hash.update(&buffer[..n]);
        file.write_all(&buffer[..n])
            .map_err(|e| DownloadError::Broken(format!("could not save the download: {e}")))?;
        progress.done.store(done, Ordering::Relaxed);
    }
    if done != row.byte_size {
        return Err(DownloadError::Broken(
            "The download stopped before it finished".into(),
        ));
    }
    if super::hex(&hash.finalize()) != row.sha256 {
        return Err(DownloadError::Broken(
            "The download does not match what petramond.com published".into(),
        ));
    }
    file.sync_all()
        .map_err(|e| DownloadError::Broken(format!("could not save the download: {e}")))
}
