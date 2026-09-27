//! The client's sign-in lifecycle: the policy that sits between the stored
//! credential ([`super::store`]) and the raw endpoints ([`super::api`]).
//!
//! The rules it owns, in one place because getting any of them wrong locks a
//! player out of their own account:
//!
//! - Rotation is lazy: a token is traded for a fresh one when it is due, and
//!   only as part of doing something that needs it.
//! - A rotation the service could not answer is NOT a sign-out. As long as the
//!   old token has not hit its hard expiry it keeps being used, so a player
//!   whose connection is flaky still joins.
//! - A refusal that says the credential is dead ([`AccountError::SignInRequired`])
//!   clears the stored sign-in here, once, rather than in each caller.
//! - Rotation is serialized process-wide: two workers spending the token at
//!   once (a verify and a download) must never present a token the other just
//!   retired and sign the player out for it. [`spend`] is the ONE way to call
//!   an endpoint with the stored token.
//!
//! Every function BLOCKS on the network; callers run them on worker threads.

use std::sync::{Mutex, MutexGuard};

use super::store::{self, SavedSignIn};
use super::{api, AccountError, AccountIdentity};

/// Held while the stored token is read and, when due, rotated — and while a
/// dead-token refusal decides whether to clear it.
static ROTATION: Mutex<()> = Mutex::new(());

fn rotation() -> MutexGuard<'static, ()> {
    ROTATION.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// An error a call spending the token can end in: whether it says the token
/// is worthless, and how a sign-in problem found before the call reads.
pub trait SpendError: From<AccountError> {
    fn clears_sign_in(&self) -> bool;
}

impl SpendError for AccountError {
    fn clears_sign_in(&self) -> bool {
        AccountError::clears_sign_in(self)
    }
}

impl From<AccountError> for crate::service::ServiceError {
    fn from(e: AccountError) -> Self {
        match e {
            AccountError::SignInRequired(m) => Self::SignInRequired(m),
            AccountError::Refused(m) => Self::Refused(m),
            AccountError::Unreachable(m) => Self::Unreachable(m),
        }
    }
}

impl SpendError for crate::service::ServiceError {
    fn clears_sign_in(&self) -> bool {
        matches!(self, crate::service::ServiceError::SignInRequired(_))
    }
}

/// Call an endpoint with the stored token, rotated first when due. A refusal
/// that says the token is dead clears the stored sign-in — unless another
/// worker rotated it meanwhile, in which case the call is made once more
/// with the fresh token instead of signing the player out.
pub fn spend<T, E: SpendError>(mut call: impl FnMut(&str) -> Result<T, E>) -> Result<T, E> {
    let mut retried = false;
    loop {
        let saved = usable()?;
        match call(&saved.token) {
            Err(e) if e.clears_sign_in() => {
                let _held = rotation();
                let current = store::load();
                if !retried && current.as_ref().is_some_and(|c| c.token != saved.token) {
                    retried = true;
                    continue;
                }
                if current.is_some_and(|c| c.token == saved.token) {
                    store::clear();
                }
                return Err(e);
            }
            other => return other,
        }
    }
}

/// The stored sign-in, whatever its freshness. `None` = nobody is signed in.
pub fn current() -> Option<SavedSignIn> {
    store::load()
}

/// Sign in with a password and remember the result.
pub fn sign_in(identifier: &str, password: &str) -> Result<AccountIdentity, AccountError> {
    let signed_in = api::sign_in(identifier, password)?;
    let saved = SavedSignIn::from_service("", &signed_in);
    if saved.token.is_empty() {
        return Err(AccountError::Unreachable(
            "The Petramond account service answered without a token".to_owned(),
        ));
    }
    let _held = rotation();
    remember(saved)
}

/// Forget the stored sign-in and tell the service to drop it. The local file is
/// removed even when the call fails — the player asked to be signed out.
pub fn sign_out() {
    if let Some(saved) = store::load() {
        api::sign_out(&saved.token);
    }
    store::clear();
}

/// Confirm the stored sign-in against the service and refresh what is displayed
/// about it (a renamed account, a changed avatar). Rotates first when due.
pub fn verify() -> Result<AccountIdentity, AccountError> {
    let (token, signed_in) = spend(|token| Ok((token.to_owned(), api::identity(token)?)))?;
    let _held = rotation();
    remember(SavedSignIn::from_service(&token, &signed_in))
}

/// A single-use credential for the server that offered `server_id`, rotating the
/// stored token first if it is due.
pub fn join_ticket_for(server_id: &str) -> Result<String, AccountError> {
    spend(|token| api::join_ticket(token, server_id))
}

/// The stored sign-in, rotated if due — the precondition of every call that
/// spends the token. Read under the rotation lock, so a rotation another
/// worker just made is what this one sees.
fn usable() -> Result<SavedSignIn, AccountError> {
    let _held = rotation();
    let saved = store::load().ok_or_else(|| {
        AccountError::SignInRequired(
            "Sign in to your Petramond account to play on a server".to_owned(),
        )
    })?;
    if saved.expired() {
        store::clear();
        return Err(AccountError::SignInRequired(
            "Your Petramond sign-in has expired — sign in again".to_owned(),
        ));
    }
    if !saved.due_for_refresh() {
        return Ok(saved);
    }
    match api::refresh(&saved.token) {
        Ok(signed_in) => {
            let rotated = SavedSignIn::from_service(&saved.token, &signed_in);
            store::store(&rotated).unwrap_or_else(|e| {
                // The service has already invalidated the old token, so a token
                // we cannot write down is gone: say so rather than let the next
                // launch present a credential the service has retired.
                log::error!("could not store the rotated Petramond sign-in: {e}");
                store::clear();
            });
            Ok(rotated)
        }
        // Unreachable mid-rotation: the old token is still inside its hard
        // expiry (checked above), so keep using it. If the rotation actually
        // COMMITTED and only the reply was lost, the old token is already dead
        // and the next call refuses with `SignInRequired` — one forced sign-in,
        // which is the honest outcome of losing a rotation over a dropped
        // connection, not a bug to chase.
        Err(AccountError::Unreachable(_)) => Ok(saved),
        Err(e) => Err(forget_if_dead(e)),
    }
}

fn remember(saved: SavedSignIn) -> Result<AccountIdentity, AccountError> {
    let identity = saved.identity();
    store::store(&saved).map_err(|e| {
        AccountError::Unreachable(format!("Could not save your Petramond sign-in: {e}"))
    })?;
    Ok(identity)
}

/// Clear the stored sign-in exactly for the refusals that mean it is worthless.
fn forget_if_dead(e: AccountError) -> AccountError {
    if e.clears_sign_in() {
        store::clear();
    }
    e
}
