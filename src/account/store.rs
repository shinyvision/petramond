//! The stored sign-in: `account.json` in the base data dir.
//!
//! Separate from `client.json` on purpose. `client.json` is materialized with
//! defaults so its knobs are discoverable by opening it; this file holds a
//! bearer credential, exists only while somebody is signed in, and is written
//! owner-readable only.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the client remembers about the signed-in player between launches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSignIn {
    /// The sign-in token. Never sent to a game server — see [`super`].
    pub token: String,
    pub user_id: i64,
    /// The account username. This is the player name in every online session.
    pub username: String,
    pub avatar_url: String,
    pub profile_url: String,
    /// Unix seconds: past this the client rotates the token before using it.
    pub refresh_after: i64,
    /// Unix seconds: past this the token is dead and the password is needed.
    pub expires_at: i64,
}

impl SavedSignIn {
    /// Bank a fresh service answer against the local clock. `token` carries over
    /// when the answer minted none (an identity check).
    pub fn from_service(previous_token: &str, signed_in: &super::SignedIn) -> SavedSignIn {
        let now = super::now_unix();
        SavedSignIn {
            token: signed_in
                .token
                .clone()
                .unwrap_or_else(|| previous_token.to_owned()),
            user_id: signed_in.identity.user_id,
            username: signed_in.identity.username.clone(),
            avatar_url: signed_in.identity.avatar_url.clone(),
            profile_url: signed_in.identity.profile_url.clone(),
            refresh_after: now + signed_in.refresh_in,
            expires_at: now + signed_in.expires_in,
        }
    }

    pub fn identity(&self) -> super::AccountIdentity {
        super::AccountIdentity {
            user_id: self.user_id,
            username: self.username.clone(),
            avatar_url: self.avatar_url.clone(),
            profile_url: self.profile_url.clone(),
        }
    }

    /// The token is worth trading for a fresh one.
    pub fn due_for_refresh(&self) -> bool {
        super::now_unix() >= self.refresh_after
    }

    /// The token is past its hard limit — only a password can replace it.
    pub fn expired(&self) -> bool {
        super::now_unix() >= self.expires_at
    }
}

fn path() -> PathBuf {
    crate::save::base_data_dir().join("account.json")
}

/// The stored sign-in, or `None` when nobody is signed in (absent file) or the
/// file is unusable. An unreadable credential file is the same situation as no
/// credential: the player signs in again.
pub fn load() -> Option<SavedSignIn> {
    let bytes = std::fs::read(path()).ok()?;
    match serde_json::from_slice::<SavedSignIn>(&bytes) {
        Ok(saved) if !saved.token.is_empty() => Some(saved),
        Ok(_) => None,
        Err(e) => {
            log::warn!("stored Petramond sign-in is unreadable ({e}); signing out");
            None
        }
    }
}

/// Replace the stored sign-in (atomic tmp+rename, then owner-only permissions).
pub fn store(saved: &SavedSignIn) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other)?;
    petramond_persist::atomic_file::replace(&path, &bytes)?;
    restrict(&path);
    Ok(())
}

/// Forget the stored sign-in. Absent is the goal, so a missing file is success.
pub fn clear() {
    match std::fs::remove_file(path()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("could not remove the stored Petramond sign-in: {e}"),
    }
}

#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        log::warn!("could not restrict permissions on {}: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}
