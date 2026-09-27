//! Staging, pending changes, applying them, and the content lock.
//!
//! A download is unpacked and admitted into `mods/.staging/` (hidden, so
//! discovery never sees it, and on the same filesystem, so the final rename
//! is atomic) and a pending change is written. Applying it holds the content
//! lock exclusively, in steps
//! ordered so a crash anywhere leaves a state the next run finishes and no
//! failure ever costs the player the version they had.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::records::{self, InstallRecord};
use super::{archive, Kind};

/// Where installed packs and the library's state live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// The installed root (`installed_mods_dir()`).
    pub mods: PathBuf,
    /// The library's own state (`content::dir()`).
    pub content: PathBuf,
}

impl Dirs {
    pub fn installed() -> Self {
        Self {
            mods: petramond_util::paths::installed_mods_dir(),
            content: super::dir(),
        }
    }

    pub fn staging(&self) -> PathBuf {
        self.mods.join(".staging")
    }

    fn pending_dir(&self) -> PathBuf {
        self.content.join("pending")
    }

    fn pending_path(&self, dir: &str) -> PathBuf {
        self.pending_dir().join(format!("{dir}.json"))
    }
}

/// `content/lock`: every process that reads packs or writes content state
/// holds it SHARED for its whole life; applying changes holds it
/// exclusively, so no process ever has a pack renamed under it.
pub struct ContentLock {
    file: File,
    gate: File,
    gate_held: bool,
}

impl ContentLock {
    fn open(dirs: &Dirs) -> std::io::Result<File> {
        std::fs::create_dir_all(&dirs.content)?;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dirs.content.join("lock"))
    }

    fn open_gate(dirs: &Dirs) -> std::io::Result<File> {
        std::fs::create_dir_all(&dirs.content)?;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dirs.content.join("lock-gate"))
    }

    /// Wait for, then hold, the shared lock.
    pub fn shared(dirs: &Dirs) -> std::io::Result<Self> {
        let gate = Self::open_gate(dirs)?;
        gate.lock_shared()?;
        let file = Self::open(dirs)?;
        let held = file.lock_shared();
        gate.unlock()?;
        held?;
        Ok(Self {
            file,
            gate,
            gate_held: false,
        })
    }

    /// The exclusive lock, or `None` while another process holds it.
    pub fn try_exclusive(dirs: &Dirs) -> std::io::Result<Option<Self>> {
        let gate = Self::open_gate(dirs)?;
        match gate.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        let file = Self::open(dirs)?;
        let held = file.try_lock();
        match held {
            Ok(()) => Ok(Some(Self {
                file,
                gate,
                gate_held: true,
            })),
            Err(std::fs::TryLockError::WouldBlock) => {
                gate.unlock()?;
                Ok(None)
            }
            Err(std::fs::TryLockError::Error(e)) => {
                gate.unlock()?;
                Err(e)
            }
        }
    }

    /// Block new readers while converting the shared lock to exclusive. On
    /// contention, restore the shared lock before opening the gate again.
    pub fn try_upgrade(&mut self) -> std::io::Result<bool> {
        if self.gate_held {
            return Err(std::io::Error::other("content lock is already exclusive"));
        }
        match self.gate.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        let held = (|| {
            self.file.unlock()?;
            let held = self.file.try_lock();
            if held.is_err() {
                self.file.lock_shared()?;
            }
            Ok::<_, std::io::Error>(held)
        })();
        match held {
            Ok(Ok(())) => {
                self.gate_held = true;
                Ok(true)
            }
            Ok(Err(std::fs::TryLockError::WouldBlock)) => {
                self.gate.unlock()?;
                Ok(false)
            }
            Ok(Err(std::fs::TryLockError::Error(e))) | Err(e) => {
                self.gate.unlock()?;
                Err(e)
            }
        }
    }

    /// Return to the shared lock after an in-process apply.
    pub fn downgrade_in_place(&mut self) -> std::io::Result<()> {
        if !self.gate_held {
            return Ok(());
        }
        self.file.unlock()?;
        self.file.lock_shared()?;
        self.gate.unlock()?;
        self.gate_held = false;
        Ok(())
    }

    /// Exclusive to shared, for the rest of the process's life.
    pub fn downgrade(self) -> std::io::Result<Self> {
        let mut lock = self;
        lock.downgrade_in_place()?;
        Ok(lock)
    }
}

/// One staged change, `content/pending/<dir>.json`, keyed by DIRECTORY name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingChange {
    pub format: u32,
    #[serde(flatten)]
    pub op: Op,
    pub dir: String,
    /// `(moved-aside path, original path)`, written before anything moves:
    /// what a failed or interrupted apply puts back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub journal: Vec<(PathBuf, PathBuf)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Install {
        /// Relative to the installed root.
        staged: PathBuf,
        record: Box<InstallRecord>,
    },
    Remove,
}

/// What the startup apply did, shown once to the player (the title's notice,
/// the library's rows): a release build may have no console to log to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Directory names installed, updated or removed.
    pub applied: Vec<String>,
    /// `(directory name, why)` for each change that was dropped.
    pub failed: Vec<(String, String)>,
    /// Another Petramond process held the lock: nothing was applied.
    pub deferred: bool,
}

/// What is being installed: the listing's facts, never the archive's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub mod_id: String,
    pub kind: Kind,
    /// `None` for a local development install.
    pub content_id: Option<i64>,
    pub name: String,
    pub version: String,
}

impl From<&super::ListingRow> for Offer {
    fn from(row: &super::ListingRow) -> Self {
        Self {
            mod_id: row.mod_id.clone(),
            kind: row.kind,
            content_id: Some(row.content_id),
            name: row.name.clone(),
            version: row.version.clone(),
        }
    }
}

/// Every pending change, by directory name.
pub fn pending(dirs: &Dirs) -> Vec<PendingChange> {
    let Ok(entries) = std::fs::read_dir(dirs.pending_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<PendingChange> = entries
        .flatten()
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

fn write_pending(dirs: &Dirs, change: &PendingChange) -> std::io::Result<()> {
    std::fs::create_dir_all(dirs.pending_dir())?;
    let bytes = serde_json::to_vec_pretty(change).map_err(std::io::Error::other)?;
    petramond_persist::atomic_file::replace(&dirs.pending_path(&change.dir), &bytes)
}

/// Unpack the archive at `zip`, admit it as a pack, and stage it as a
/// pending install of `offer`. The zip is consumed; every failure removes
/// what this call wrote. `Err` is the player-facing reason.
pub fn stage_install(
    dirs: &Dirs,
    zip: &Path,
    offer: &Offer,
    shipped: &BTreeSet<String>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let staged = dirs
        .staging()
        .join(format!("{}-{}", offer.mod_id, super::nonce()));
    let result = stage(dirs, zip, offer, shipped, cancel, &staged);
    let _ = std::fs::remove_file(zip);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    result
}

fn stage(
    dirs: &Dirs,
    zip: &Path,
    offer: &Offer,
    shipped: &BTreeSet<String>,
    cancel: &AtomicBool,
    staged: &Path,
) -> Result<(), String> {
    if shipped.contains(&offer.mod_id) {
        return Err(format!(
            "'{}' is a content pack that ships with Petramond",
            offer.mod_id
        ));
    }
    if dirs.pending_path(&offer.mod_id).exists() {
        return Err("A change to this pack is already waiting to be applied".into());
    }
    let bytes = std::fs::read(zip).map_err(|e| format!("could not read the download: {e}"))?;
    std::fs::create_dir_all(dirs.staging()).map_err(|e| e.to_string())?;
    archive::Archive::open(&bytes)?.extract(staged, cancel)?;
    let header = petramond_world::assets::admit_pack_dir(staged)
        .map_err(|why| format!("Not a valid pack: {why}"))?;
    match header.id.as_deref() {
        Some(id) if id == offer.mod_id && !shipped.contains(id) => {}
        other => {
            return Err(format!(
                "Not a valid pack: its pack.json says id {:?}, not '{}'",
                other.unwrap_or(""),
                offer.mod_id
            ))
        }
    }
    let pack_json = std::fs::read(staged.join("pack.json")).map_err(|e| e.to_string())?;
    let record = InstallRecord {
        format: records::FORMAT,
        id: offer.mod_id.clone(),
        kind: offer.kind,
        content_id: offer.content_id,
        name: offer.name.clone(),
        version: offer.version.clone(),
        archive_sha256: super::sha256_hex(&bytes),
        archive_bytes: bytes.len() as u64,
        pack_json_sha256: super::sha256_hex(&pack_json),
        installed_ms: super::now_ms(),
    };
    let relative = staged
        .strip_prefix(&dirs.mods)
        .map_err(|_| "the staging folder is outside the installed root".to_owned())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(archive::CANCELLED.into());
    }
    write_pending(
        dirs,
        &PendingChange {
            format: records::FORMAT,
            op: Op::Install {
                staged: relative.to_owned(),
                record: Box::new(record),
            },
            dir: offer.mod_id.clone(),
            journal: Vec::new(),
        },
    )
    .map_err(|e| format!("could not stage the install: {e}"))
}

/// Stage removing the installed directory `dir`.
pub fn stage_remove(dirs: &Dirs, dir: &str) -> Result<(), String> {
    if dir.starts_with('.') || dir.contains(['/', '\\']) || !dirs.mods.join(dir).is_dir() {
        return Err(format!("'{dir}' is not an installed pack"));
    }
    if dirs.pending_path(dir).exists() {
        return Err("A change to this pack is already waiting to be applied".into());
    }
    write_pending(
        dirs,
        &PendingChange {
            format: records::FORMAT,
            op: Op::Remove,
            dir: dir.to_owned(),
            journal: Vec::new(),
        },
    )
    .map_err(|e| format!("could not stage the removal: {e}"))
}

/// Withdraw a pending change (and an install's staged files).
pub fn undo(dirs: &Dirs, dir: &str) {
    if let Some(change) = pending(dirs).into_iter().find(|c| c.dir == dir) {
        if let Op::Install { staged, .. } = &change.op {
            let _ = std::fs::remove_dir_all(dirs.mods.join(staged));
        }
    }
    let _ = std::fs::remove_file(dirs.pending_path(dir));
}

/// Apply every pending change, if no other Petramond process is running,
/// and return the report with the SHARED lock this process then holds for
/// its life. Must run before pack discovery.
pub fn apply_pending() -> (ApplyReport, Option<ContentLock>) {
    debug_assert!(
        !petramond_world::assets::discovery_started(),
        "pending content changes apply before any pack is discovered"
    );
    let dirs = Dirs::installed();
    let shipped = petramond_world::assets::shipped_pack_ids();
    let exclusive = match ContentLock::try_exclusive(&dirs) {
        Ok(lock) => lock,
        Err(e) => {
            log::warn!("content lock: {e}");
            return (ApplyReport::default(), None);
        }
    };
    let Some(exclusive) = exclusive else {
        let lock = ContentLock::shared(&dirs)
            .map_err(|e| log::warn!("content lock: {e}"))
            .ok();
        let report = ApplyReport {
            deferred: !pending(&dirs).is_empty(),
            ..Default::default()
        };
        return (report, lock);
    };
    let report = apply_in(&dirs, &shipped);
    for dir in &report.applied {
        log::info!("content: applied the pending change to '{dir}'");
    }
    for (dir, why) in &report.failed {
        log::error!("content: could not apply the change to '{dir}': {why}");
    }
    let lock = exclusive
        .downgrade()
        .map_err(|e| log::warn!("content lock: {e}"))
        .ok();
    (report, lock)
}

/// Apply staged changes while this process remains open. The caller must
/// switch to a newly built registry before entering another world session.
pub fn apply_pending_live(dirs: &Dirs, lock: &mut ContentLock) -> ApplyReport {
    match lock.try_upgrade() {
        Ok(true) => {}
        Ok(false) => {
            return ApplyReport {
                deferred: !pending(dirs).is_empty(),
                ..Default::default()
            };
        }
        Err(e) => {
            return ApplyReport {
                failed: vec![("content lock".to_owned(), e.to_string())],
                ..Default::default()
            };
        }
    }
    let report = apply_in(dirs, &petramond_world::assets::shipped_pack_ids());
    if let Err(e) = lock.downgrade_in_place() {
        log::error!("content lock: could not return to shared access: {e}");
    }
    report
}

/// The apply itself, with the lock already held.
pub fn apply_in(dirs: &Dirs, shipped: &BTreeSet<String>) -> ApplyReport {
    let mut report = ApplyReport::default();
    for change in pending(dirs) {
        let dir = change.dir.clone();
        let outcome = match &change.op {
            Op::Install { .. } => apply_install(dirs, shipped, change),
            Op::Remove => apply_remove(dirs, change),
        };
        match outcome {
            Ok(()) => report.applied.push(dir),
            Err(why) => report.failed.push((dir, why)),
        }
    }
    sweep(dirs);
    report
}

/// Installed-root directories holding pack `id`: `mods/<id>` and any other
/// whose `pack.json` says that id.
fn occupants(dirs: &Dirs, id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let own = dirs.mods.join(id);
    if own.exists() {
        out.push(own.clone());
    }
    if let Ok(entries) = std::fs::read_dir(&dirs.mods) {
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || path == own || !path.is_dir() {
                continue;
            }
            let says = std::fs::read(path.join("pack.json"))
                .ok()
                .and_then(|b| records::pack_json_id(&b));
            if says.as_deref() == Some(id) {
                out.push(path);
            }
        }
    }
    out
}

/// A version moved aside by an install: the sweep puts it back while its
/// place is empty.
const OLD: &str = ".old-";
/// A pack moved aside by a removal: the sweep only ever deletes it, so a
/// deletion that failed part way never restores what the player removed.
const TRASH: &str = ".trash-";

fn aside(dirs: &Dirs, original: &Path, suffix: &str) -> PathBuf {
    let name = original
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dirs.staging()
        .join(format!("{name}{suffix}{}", super::nonce()))
}

/// Put every moved-aside directory back where it was.
fn roll_back(journal: &[(PathBuf, PathBuf)]) {
    for (old, original) in journal.iter().rev() {
        if old.exists() && !original.exists() {
            if let Err(e) = std::fs::rename(old, original) {
                log::error!(
                    "content: could not put {} back at {}: {e}",
                    old.display(),
                    original.display()
                );
            }
        }
    }
}

fn drop_change(dirs: &Dirs, change: &PendingChange) {
    if let Op::Install { staged, .. } = &change.op {
        let _ = std::fs::remove_dir_all(dirs.mods.join(staged));
    }
    let _ = std::fs::remove_file(dirs.pending_path(&change.dir));
}

fn finish(dirs: &Dirs, change: &PendingChange) {
    for (old, _) in &change.journal {
        let _ = std::fs::remove_dir_all(old);
    }
    let _ = std::fs::remove_file(dirs.pending_path(&change.dir));
}

fn apply_install(
    dirs: &Dirs,
    shipped: &BTreeSet<String>,
    mut change: PendingChange,
) -> Result<(), String> {
    let Op::Install { staged, record } = change.op.clone() else {
        return Ok(());
    };
    let staged = dirs.mods.join(staged);
    let target = dirs.mods.join(&record.id);
    if shipped.contains(&record.id) {
        roll_back(&change.journal);
        drop_change(dirs, &change);
        return Err(format!(
            "'{}' is a content pack that ships with Petramond",
            record.id
        ));
    }
    if !staged.exists() {
        // Interrupted after the rename: finish it, or put back what was.
        let landed = std::fs::read(target.join("pack.json"))
            .is_ok_and(|b| super::sha256_hex(&b) == record.pack_json_sha256);
        if !landed {
            roll_back(&change.journal);
            drop_change(dirs, &change);
            return Err("the downloaded files are gone; get it again".into());
        }
    } else {
        if change.journal.is_empty() {
            change.journal = occupants(dirs, &record.id)
                .into_iter()
                .map(|original| (aside(dirs, &original, OLD), original))
                .collect();
            write_pending(dirs, &change).map_err(|e| e.to_string())?;
        }
        for (old, original) in &change.journal {
            if original.exists() {
                if let Err(e) = std::fs::rename(original, old) {
                    roll_back(&change.journal);
                    drop_change(dirs, &change);
                    return Err(format!("could not move the old version aside: {e}"));
                }
            }
        }
        if let Err(e) = std::fs::rename(&staged, &target) {
            roll_back(&change.journal);
            drop_change(dirs, &change);
            return Err(format!("could not put the new version in place: {e}"));
        }
    }
    if let Err(e) = records::write(dirs, &record) {
        // The new files go back to staging (and away with the change) so the
        // old version can return to its place.
        let _ = std::fs::rename(&target, &staged);
        roll_back(&change.journal);
        drop_change(dirs, &change);
        return Err(format!("could not record the install: {e}"));
    }
    finish(dirs, &change);
    Ok(())
}

fn apply_remove(dirs: &Dirs, mut change: PendingChange) -> Result<(), String> {
    let original = dirs.mods.join(&change.dir);
    if original.exists() {
        if change.journal.is_empty() {
            change.journal = vec![(aside(dirs, &original, TRASH), original.clone())];
            write_pending(dirs, &change).map_err(|e| e.to_string())?;
        }
        let (old, _) = &change.journal[0];
        if let Err(e) = std::fs::rename(&original, old) {
            drop_change(dirs, &change);
            return Err(format!("could not remove it: {e}"));
        }
    }
    // A record only ever describes `mods/<id>`: removing a stray folder that
    // merely claims an id leaves the real pack's record alone.
    records::remove(dirs, &change.dir);
    finish(dirs, &change);
    Ok(())
}

/// Clear what no pending change refers to: partial downloads, orphaned
/// stages and removed packs go; a version an install moved aside goes only
/// when something now occupies its place, else it goes BACK (a crash between
/// moving it and finishing must never cost the player their pack); records
/// of packs that are gone.
fn sweep(dirs: &Dirs) {
    let remaining = pending(dirs);
    let referenced: BTreeSet<PathBuf> = remaining
        .iter()
        .flat_map(|change| {
            let staged = match &change.op {
                Op::Install { staged, .. } => Some(dirs.mods.join(staged)),
                Op::Remove => None,
            };
            staged
                .into_iter()
                .chain(change.journal.iter().map(|(old, _)| old.clone()))
        })
        .collect();
    if let Ok(entries) = std::fs::read_dir(dirs.staging()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if referenced.contains(&path) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some((original, _)) = name.rsplit_once(OLD) {
                let original = dirs.mods.join(original);
                if !original.exists() {
                    let _ = std::fs::rename(&path, &original);
                    continue;
                }
            }
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    for record in records::all(dirs) {
        if !dirs.mods.join(&record.id).exists() {
            records::remove(dirs, &record.id);
        }
    }
}
