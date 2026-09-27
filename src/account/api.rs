//! The account service's endpoints, one function apiece.
//!
//! Every function is a single blocking round trip. They are deliberately free of
//! policy: nothing here reads or writes the stored sign-in, decides when to
//! rotate, or clears anything — [`super::store`] owns the file and
//! `petramond-client`'s account session owns the sequencing.

use serde_json::json;

use super::http::{field_i64, field_str, get, post};
use super::{AccountError, AccountIdentity};

/// A live sign-in as the service reports it. `token` is present only on the
/// calls that mint one (sign-in and refresh); lifetimes are DURATIONS from now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub token: Option<String>,
    pub identity: AccountIdentity,
    /// Seconds until the client should trade this token for a fresh one.
    pub refresh_in: i64,
    /// Seconds until the token stops working entirely.
    pub expires_in: i64,
}

/// Trade a username-or-email and password for a sign-in token.
pub fn sign_in(identifier: &str, password: &str) -> Result<SignedIn, AccountError> {
    let body = post(
        "/api/v1/auth/sign-in",
        json!({
            "identifier": identifier,
            "password": password,
            "client": super::client_label(),
        }),
        None,
    )?;
    signed_in(&body)
}

/// Rotate a token. The presented token is dead once this returns `Ok`.
pub fn refresh(token: &str) -> Result<SignedIn, AccountError> {
    let body = post(
        "/api/v1/auth/refresh",
        json!({ "token": token, "client": super::client_label() }),
        None,
    )?;
    signed_in(&body)
}

/// Who a token belongs to, without rotating it.
pub fn identity(token: &str) -> Result<SignedIn, AccountError> {
    let body = get("/api/v1/auth/identity", token)?;
    signed_in(&body)
}

/// Revoke a token. Best effort: the local sign-in is cleared either way, and a
/// token the service still holds expires on its own.
pub fn sign_out(token: &str) {
    let _ = post("/api/v1/auth/sign-out", json!({ "token": token }), None);
}

/// Mint a single-use join credential for the server that offered `server_id`.
pub fn join_ticket(token: &str, server_id: &str) -> Result<String, AccountError> {
    let body = post(
        "/api/v1/auth/join-ticket",
        json!({ "serverId": server_id }),
        Some(token),
    )?;
    field_str(&body, "ticket")
}

/// The SERVER side: redeem a joining client's ticket and learn who they are.
/// Consumes the ticket, so exactly one server learns exactly one identity from
/// it, and only the server that offered `server_id`.
pub fn verify_join(ticket: &str, server_id: &str) -> Result<AccountIdentity, AccountError> {
    let body = post(
        "/api/v1/auth/verify-join",
        json!({ "ticket": ticket, "serverId": server_id }),
        None,
    )?;
    identity_of(&body)
}

fn signed_in(body: &serde_json::Value) -> Result<SignedIn, AccountError> {
    Ok(SignedIn {
        token: body
            .get("token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        identity: identity_of(body)?,
        refresh_in: field_i64(body, "refreshInSeconds")?,
        expires_in: field_i64(body, "expiresInSeconds")?,
    })
}

fn identity_of(body: &serde_json::Value) -> Result<AccountIdentity, AccountError> {
    Ok(AccountIdentity {
        user_id: field_i64(body, "userId")?,
        username: field_str(body, "username")?,
        avatar_url: field_str(body, "avatarUrl")?,
        profile_url: field_str(body, "profileUrl")?,
    })
}
