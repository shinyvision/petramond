use serde_json::json;

use super::http::{field_i64, field_str, get, post};
use super::{AccountError, AccountIdentity};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub token: Option<String>,
    pub identity: AccountIdentity,
    pub refresh_in: i64,
    pub expires_in: i64,
}

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

pub fn refresh(token: &str) -> Result<SignedIn, AccountError> {
    let body = post(
        "/api/v1/auth/refresh",
        json!({ "token": token, "client": super::client_label() }),
        None,
    )?;
    signed_in(&body)
}

pub fn identity(token: &str) -> Result<SignedIn, AccountError> {
    let body = get("/api/v1/auth/identity", token)?;
    signed_in(&body)
}

pub fn sign_out(token: &str) {
    let _ = post("/api/v1/auth/sign-out", json!({ "token": token }), None);
}

pub fn join_ticket(token: &str, server_id: &str) -> Result<String, AccountError> {
    let body = post(
        "/api/v1/auth/join-ticket",
        json!({ "serverId": server_id }),
        Some(token),
    )?;
    field_str(&body, "ticket")
}

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
