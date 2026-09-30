//! Versioned save formats: typed decode errors, the per-format migration
//! chain, and the world-level `format.json` stamp.
//!
//! Every persisted record (section record, `level.dat`, `players/*.dat`)
//! leads with its version. A [`Format`] names the version this build writes
//! and the chain of [`Upgrade`] steps that lifts every older version it still
//! reads, one version at a time, to the current layout — so a record is
//! migrated lazily as it is read and written back in the current layout the
//! next time it is saved. A record this build cannot read is a
//! [`RecordError`], never "absent": the caller decides whether to quarantine
//! it, refuse to overwrite it, or refuse to open the world.
//!
//! Policy for changing a persisted layout:
//! - A record built as a tagged record (`save::wire`: player files, mobs,
//!   item entities) GAINS a field by adding a new tag with a default: no
//!   bump, no step — older records read the default. A field an older build
//!   does not know is kept, never dropped: player files carry it back
//!   through the save, and a mob or item entity carrying one stays in disk
//!   form (kept beside its section) instead of coming to life without it.
//! - Any other change bumps the format's `current` and adds ONE upgrade step
//!   from the previous version. The step works on that version's bytes as
//!   they were shipped, so it must not call the live decoders (which follow
//!   the new layout).
//! - Commit a fixture of the new version under `src/save/fixtures/`
//!   with a test that decodes it; keep every older fixture decoding.
//! - The oldest readable version (`current - steps`) only moves by deleting
//!   the oldest step, which strands the worlds that still hold it: after 1.0
//!   that is a deliberate, announced break, never a side effect.
//!
//! `format.json` records the newest version of each format the world may
//! hold. Opening checks it first: a world written by a newer build is
//! refused before anything reads or rewrites it, and a world with a version
//! older than this build's migrations reach is refused too. A world that is
//! only older gets its small files (level, players, palette) copied into
//! `backup/` and its stamp raised, then migrates record by record.

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::path::Path;

pub type Upgrade = fn(&[u8]) -> Result<Vec<u8>, RecordError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    Newer {
        format: &'static str,
        found: u32,
        newest: u32,
    },
    Retired {
        format: &'static str,
        found: u32,
        oldest: u32,
    },
    UnknownPayload {
        format: &'static str,
        flags: u32,
    },
    Corrupt {
        format: &'static str,
        what: &'static str,
        offset: usize,
    },
    Io {
        format: &'static str,
        kind: io::ErrorKind,
    },
}

impl RecordError {
    pub fn corrupt(format: &'static str, what: &'static str, offset: usize) -> Self {
        Self::Corrupt {
            format,
            what,
            offset,
        }
    }

    pub fn is_from_newer_build(&self) -> bool {
        matches!(self, Self::Newer { .. } | Self::UnknownPayload { .. })
    }
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Newer {
                format,
                found,
                newest,
            } => write!(
                f,
                "{format} v{found} was written by a newer build (this build reads up to v{newest})"
            ),
            Self::Retired {
                format,
                found,
                oldest,
            } => write!(
                f,
                "{format} v{found} is older than this build can migrate (oldest v{oldest})"
            ),
            Self::UnknownPayload { format, flags } => write!(
                f,
                "{format} carries payloads this build does not know (flags {flags:#08x})"
            ),
            Self::Corrupt {
                format,
                what,
                offset,
            } => write!(f, "{format} is corrupt: bad {what} at byte {offset}"),
            Self::Io { format, kind } => write!(f, "{format} could not be read: {kind}"),
        }
    }
}

impl std::error::Error for RecordError {}

impl From<RecordError> for io::Error {
    fn from(e: RecordError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }
}

pub struct Format {
    pub name: &'static str,
    pub current: u32,
    steps: &'static [Upgrade],
}

impl Format {
    pub const fn new(name: &'static str, current: u32, steps: &'static [Upgrade]) -> Self {
        assert!(steps.len() < current as usize, "versions start at 1");
        Self {
            name,
            current,
            steps,
        }
    }

    pub const fn oldest(&self) -> u32 {
        self.current - self.steps.len() as u32
    }

    pub fn check(&self, version: u32) -> Result<(), RecordError> {
        if version > self.current {
            return Err(RecordError::Newer {
                format: self.name,
                found: version,
                newest: self.current,
            });
        }
        if version < self.oldest() {
            return Err(RecordError::Retired {
                format: self.name,
                found: version,
                oldest: self.oldest(),
            });
        }
        Ok(())
    }

    pub fn upgrade<'a>(&self, version: u32, body: &'a [u8]) -> Result<Cow<'a, [u8]>, RecordError> {
        self.check(version)?;
        let first = (version - self.oldest()) as usize;
        let mut out = Cow::Borrowed(body);
        for step in &self.steps[first..] {
            out = Cow::Owned(step(&out)?);
        }
        Ok(out)
    }

    pub fn upgrade_u32_record<'a>(&self, bytes: &'a [u8]) -> Result<Cow<'a, [u8]>, RecordError> {
        let (header, body) = bytes
            .split_first_chunk::<4>()
            .ok_or_else(|| RecordError::corrupt(self.name, "version header", 0))?;
        self.upgrade(u32::from_le_bytes(*header), body)
    }
}

const STAMP: &str = "format.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorldFormat {
    pub section: u32,
    pub level: u32,
    pub player: u32,
}

impl WorldFormat {
    pub fn current() -> Self {
        Self {
            section: super::codec::SECTION.current,
            level: super::level::FORMAT.current,
            player: super::player::FORMAT.current,
        }
    }

    fn formats(&self) -> [(&'static Format, u32); 3] {
        [
            (&super::codec::SECTION, self.section),
            (&super::level::FORMAT, self.level),
            (&super::player::FORMAT, self.player),
        ]
    }

    pub fn check(&self) -> Result<(), RecordError> {
        self.formats()
            .into_iter()
            .try_for_each(|(format, version)| format.check(version))
    }
}

pub fn read_stamp(dir: &Path) -> io::Result<Option<WorldFormat>> {
    let path = dir.join(STAMP);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    serde_json::from_str(&text).map(Some).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unreadable {}: {e}", path.display()),
        )
    })
}

pub fn prepare_world(dir: &Path) -> io::Result<Option<WorldFormat>> {
    let found = read_stamp(dir)?;
    let current = WorldFormat::current();
    if let Some(stamp) = found {
        stamp.check()?;
        if stamp != current {
            let name = format!(
                "format-s{}-l{}-p{}",
                stamp.section, stamp.level, stamp.player
            );
            back_up_small_files(dir, &name)?;
            log::info!(
                "world {} is saved in an older format ({stamp:?}); its records migrate to \
                 {current:?} as they are saved",
                dir.display()
            );
        }
    }
    if found != Some(current) {
        let json = serde_json::to_string_pretty(&current).expect("stamp serializes");
        petramond_persist::atomic_file::replace(&dir.join(STAMP), json.as_bytes())?;
    }
    Ok(found)
}

pub fn back_up_small_files(dir: &Path, name: &str) -> io::Result<()> {
    let backup = dir.join("backup").join(name);
    if backup.exists() {
        return Ok(());
    }
    let staging = backup.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(staging.join("players"))?;
    for name in [
        super::level::FILE,
        super::level::BACKUP,
        "palette.json",
        STAMP,
        "mods.json",
        "settings.json",
    ] {
        copy_if_present(&dir.join(name), &staging.join(name))?;
    }
    match std::fs::read_dir(dir.join("players")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    std::fs::copy(
                        entry.path(),
                        staging.join("players").join(entry.file_name()),
                    )?;
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::rename(&staging, &backup)?;
    log::info!("backed up {} before migrating it", backup.display());
    Ok(())
}

fn copy_if_present(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::copy(from, to) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn quarantine(dir: &Path, relative: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
    let base = dir.join("quarantine").join(relative);
    if let Some(parent) = base.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for n in 0u32.. {
        let path = if n == 0 {
            base.clone()
        } else {
            base.with_file_name(format!("{file_name}.{n}"))
        };
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(path),
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                petramond_persist::atomic_file::replace(&path, bytes)?;
                return Ok(path);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("u32 quarantine names exhausted")
}

#[cfg(test)]
mod tests;
