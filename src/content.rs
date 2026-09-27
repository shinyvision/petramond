//! Content: the engine's registry bootstrap and the content library.
//!
//! The bootstrap: the extension stages the engine layers over the world
//! crate's registry build (worldgen's catalogs, the mob catalog), and the
//! one-call install every engine binary runs before touching content.
//!
//! The content library's engine half: which tier an installed pack is, the
//! install records, the website's listing and downloads, the guarded archive
//! reader, staging a download as a pending change, and applying pending
//! changes at startup under the content lock.
//!
//! Nothing on disk changes under a running game: installs, updates and
//! removals are STAGED, and the next startup applies them before pack
//! discovery. The screen that drives all this is the client's; nothing here
//! knows what it looks like. Every network call BLOCKS, so callers run them
//! on worker threads.

pub mod api;
pub mod archive;
pub mod icon;
pub mod install;
pub mod records;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::PathBuf;

use petramond_world::content::{Content, ContentErrors, Stage};
use serde::{Deserialize, Serialize};

pub use api::ListingRow;
pub use install::{apply_pending, ApplyReport, ContentLock, Dirs, PendingChange};
pub use records::InstallRecord;

/// What an installed pack is, decided on disk and never from its archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Shipped with the game: never deletable, never served.
    ContentPack,
    /// Petramond-first, installed from petramond.com.
    Addon,
    /// Third party: installed from petramond.com, or dropped in by hand.
    Mod,
}

/// The website's editorial kind of a downloadable pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Addon,
    Mod,
}

/// Where the content library keeps its own state (records, pending changes,
/// the icon cache, the lock).
pub fn dir() -> PathBuf {
    petramond_util::paths::base_data_dir().join("content")
}

/// The ONE classifier: a pack from a shipped root is a content pack; one in
/// the installed root is what its VALID install record says, else a mod.
pub fn tier(pack: &petramond_world::assets::Pack) -> Tier {
    tier_in(pack, &Dirs::installed())
}

pub fn tier_in(pack: &petramond_world::assets::Pack, dirs: &Dirs) -> Tier {
    if pack.origin == petramond_world::assets::PackOrigin::Shipped {
        return Tier::ContentPack;
    }
    let Some(id) = pack.id.as_deref() else {
        return Tier::Mod;
    };
    match records::load(dirs, id).filter(|r| records::valid(dirs, r)) {
        Some(record) if record.kind == Kind::Addon => Tier::Addon,
        _ => Tier::Mod,
    }
}

/// The installed packs the first-sight rule holds OFF in an existing world:
/// installed by the content library (a valid record) and able to change a
/// world. Loose, shipped and presentation-only packs are never among them.
pub fn held_off_at_first_sight() -> BTreeSet<String> {
    let dirs = Dirs::installed();
    petramond_world::assets::packs()
        .iter()
        .filter(|pack| pack.origin == petramond_world::assets::PackOrigin::Installed)
        .filter(|pack| pack.touches_world)
        .filter_map(|pack| pack.id.clone())
        .filter(|id| records::load(&dirs, id).is_some_and(|r| records::valid(&dirs, &r)))
        .collect()
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Wall-clock milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A name that is unique for this process's life: staging names never
/// collide with a job still cleaning up.
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}{:x}{:x}",
        std::process::id(),
        now_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Every catalog stage the engine adds to a registry build, in order.
pub fn stages() -> Vec<&'static dyn Stage> {
    let mut stages: Vec<&'static dyn Stage> = petramond_worldgen::data::content_stages().to_vec();
    stages.push(&crate::mob::CATALOG);
    stages
}

/// Build the launch environment's registry with the engine's stages plus
/// `extra` (a client adds its presentation catalogs) and install it for the
/// process. On error nothing is installed and every problem comes back.
pub fn install_from_env(extra: &[&'static dyn Stage]) -> Result<Content, ContentErrors> {
    let mut all = stages();
    all.extend_from_slice(extra);
    petramond_world::content::install_from_env(&all)
}

/// The registry for a world that switched `disabled` off, with the engine's
/// stages (see `petramond_world::content::for_world`).
pub fn for_world(disabled: &BTreeSet<String>) -> Result<Content, ContentErrors> {
    petramond_world::content::for_world(disabled, &stages())
}
