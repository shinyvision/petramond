//! The account service's calls over the shared transport
//! ([`crate::service::http`]), and the mapping from an answer to an
//! [`AccountError`].
//!
//! The status mapping is the load-bearing part. A 401/403 whose body says
//! `signInRequired` means the stored token is dead and the client must clear it;
//! every other refusal leaves the stored sign-in alone; anything the service
//! could not answer at all is [`AccountError::Unreachable`], which callers treat
//! as "try again later", never as "you are signed out".

use serde_json::Value;

use super::AccountError;
use crate::service::http::{self as transport_http, TransportError};

/// Per-call network budget. A join waits on this, so it is short enough that a
/// dead service fails the join instead of hanging the connect screen — and the
/// server's pre-join deadline is extended by it, so a slow service refuses the
/// join with a reason rather than having the connection dropped underneath it.
pub const TIMEOUT: std::time::Duration = transport_http::JSON_TIMEOUT;

/// POST `body` as JSON to `path`; `bearer` adds the `Authorization` header.
pub(super) fn post(path: &str, body: Value, bearer: Option<&str>) -> Result<Value, AccountError> {
    let (status, body) = transport_http::post_json(path, &body, bearer).map_err(transport)?;
    classify(status, body)
}

/// GET `path` with a bearer token.
pub(super) fn get(path: &str, bearer: &str) -> Result<Value, AccountError> {
    let (status, body) = transport_http::get_json(path, Some(bearer)).map_err(transport)?;
    classify(status, body)
}

/// Turn a status + body into either the payload or the error the caller acts on.
pub(super) fn classify(status: u16, body: Value) -> Result<Value, AccountError> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("The Petramond account service refused the request")
        .to_owned();
    let sign_in_required = body
        .get("signInRequired")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match status {
        _ if sign_in_required => Err(AccountError::SignInRequired(message)),
        400..=499 => Err(AccountError::Refused(message)),
        _ => Err(AccountError::Unreachable(message)),
    }
}

fn transport(e: TransportError) -> AccountError {
    AccountError::Unreachable(if e.timed_out {
        "The Petramond account service did not respond".to_owned()
    } else {
        format!(
            "Could not reach the Petramond account service: {}",
            e.detail
        )
    })
}

/// A required string field, or a "the service answered something we cannot use"
/// error — a malformed reply must never read as a successful sign-in.
pub(super) fn field_str(body: &Value, key: &str) -> Result<String, AccountError> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| malformed(key))
}

pub(super) fn field_i64(body: &Value, key: &str) -> Result<i64, AccountError> {
    body.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed(key))
}

fn malformed(key: &str) -> AccountError {
    AccountError::Unreachable(format!(
        "The Petramond account service answered without '{key}'"
    ))
}
