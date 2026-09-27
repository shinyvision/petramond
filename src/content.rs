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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    ContentPack,
    Addon,
    Mod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Addon,
    Mod,
}

pub fn dir() -> PathBuf {
    petramond_util::paths::base_data_dir().join("content")
}

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

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

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

pub fn stages() -> Vec<&'static dyn Stage> {
    let mut stages: Vec<&'static dyn Stage> = petramond_worldgen::data::content_stages().to_vec();
    stages.push(&crate::mob::CATALOG);
    stages
}

pub fn install_from_env(extra: &[&'static dyn Stage]) -> Result<Content, ContentErrors> {
    let mut all = stages();
    all.extend_from_slice(extra);
    petramond_world::content::install_from_env(&all)
}

pub fn for_world(disabled: &BTreeSet<String>) -> Result<Content, ContentErrors> {
    petramond_world::content::for_world(disabled, &stages())
}
