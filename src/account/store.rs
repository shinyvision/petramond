use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSignIn {
    pub token: String,
    pub user_id: i64,
    pub username: String,
    pub avatar_url: String,
    pub profile_url: String,
    pub refresh_after: i64,
    pub expires_at: i64,
}

impl SavedSignIn {
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

    pub fn due_for_refresh(&self) -> bool {
        super::now_unix() >= self.refresh_after
    }

    pub fn expired(&self) -> bool {
        super::now_unix() >= self.expires_at
    }
}

fn path() -> PathBuf {
    crate::save::base_data_dir().join("account.json")
}

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
