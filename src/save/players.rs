//! Player files: `players/<hex key>.dat` per identity, the legacy
//! name-keyed files they migrate from, and the identity→display-name
//! registry `players/names.json`.
//!
//! Unlike the rest of [`WorldSave`](super::WorldSave), which the game thread
//! owns, [`PlayerFiles`] is a shared, thread-safe handle: a join reads and
//! decodes its player off the server thread (see `server::admissions`), so a
//! slow disk never stalls the tick. Writes still go through the save's
//! ordered write queue ([`WorldSave::save_player`](super::WorldSave::save_player));
//! only the encode lives here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::format::{self, RecordError};
use super::palette::Palette;
use super::player;
use super::worlds::{legacy_player_path, player_path};
use super::Unreadable;
use crate::net::identity::PlayerKey;

/// The identity→display-name registry file inside `players/`.
const PLAYER_REGISTRY_FILE: &str = "names.json";

/// The world's player files, shareable across threads.
pub struct PlayerFiles {
    /// `<world dir>/players/`.
    players_dir: PathBuf,
    /// The world's save directory (quarantine lives under it).
    dir: PathBuf,
    /// The world's name↔id palette every player record maps through.
    palette: Arc<Palette>,
    /// Identities whose file could not be read and must not be overwritten:
    /// their saves are dropped for the rest of the session.
    protected: Mutex<HashSet<PlayerKey>>,
    /// Slots and fields a live player cannot carry (a removed mod's items),
    /// by identity, written back with every save of that player.
    kept: Mutex<HashMap<PlayerKey, player::KeptPlayer>>,
    /// The newest registry generation written, so a slower writer holding
    /// an older snapshot never replaces a newer one.
    registry_written: Mutex<u64>,
}

impl PlayerFiles {
    pub(super) fn new(dir: PathBuf, palette: Arc<Palette>) -> PlayerFiles {
        PlayerFiles {
            players_dir: dir.join("players"),
            dir,
            palette,
            protected: Mutex::new(HashSet::new()),
            kept: Mutex::new(HashMap::new()),
            registry_written: Mutex::new(0),
        }
    }

    /// Encode `player` for its file, keeping what its last load could not
    /// bring to life; `None` when the identity's file is protected (see
    /// [`load_player`](Self::load_player)).
    pub(super) fn encode_for_save(
        &self,
        key: &PlayerKey,
        player: &crate::player::Player,
    ) -> Option<Vec<u8>> {
        if self.protected.lock().expect("protected players").contains(key) {
            return None;
        }
        Some(match self.kept.lock().expect("kept players").get(key) {
            Some(kept) => player::encode_keeping(player, &self.palette, kept),
            None => player::encode(player, &self.palette),
        })
    }

    /// Blocking read and decode of `players/<hex key>.dat`: `Ok(None)` when
    /// the player has no file yet. One small file; joins call it off the
    /// server thread.
    ///
    /// A file that exists but does not decode is an error, never a fresh
    /// player: its bytes are copied to `quarantine/players/` first, and when
    /// that fails (or the file is from a newer build) the player's saves are
    /// dropped for the session so the original is never overwritten.
    pub fn load_player(&self, key: &PlayerKey) -> Result<Option<player::PlayerData>, RecordError> {
        let path = player_path(&self.players_dir, key);
        let Some(bytes) = self.read_player_file(&path, key)? else {
            return Ok(None);
        };
        self.decode_player_file(&path, &bytes, key).map(Some)
    }

    /// Hand a pre-identity `players/<sanitized name>.dat` to `key`: once it
    /// decodes, the file MOVES to `key`'s own path (before anything else can
    /// claim it) and its contents are returned. `Ok(None)` = no legacy file
    /// for that name, or `key` already has a file of its own (never
    /// overwritten). Worlds saved before player identities existed migrate
    /// one player at a time this way, on that name's first authenticated
    /// claim.
    ///
    /// A legacy file that cannot be read or decoded is an error exactly like
    /// [`load_player`](Self::load_player): it is quarantined and never
    /// moved, and when it could not be kept aside (or is from a newer build)
    /// `key`'s saves are dropped for the session, so no fresh player file
    /// takes its place.
    pub fn adopt_legacy_player(
        &self,
        name: &str,
        key: &PlayerKey,
    ) -> Result<Option<player::PlayerData>, RecordError> {
        let legacy = legacy_player_path(&self.players_dir, name);
        let owned = player_path(&self.players_dir, key);
        if owned.exists() {
            return Ok(None);
        }
        let Some(bytes) = self.read_player_file(&legacy, key)? else {
            return Ok(None);
        };
        let data = self.decode_player_file(&legacy, &bytes, key)?;
        if let Err(e) = std::fs::rename(&legacy, &owned) {
            // The decoded state still restores; the player's next save lands
            // at `owned` and the legacy file is left as it was.
            log::warn!(
                "could not migrate legacy player file {}: {e}",
                legacy.display()
            );
            return Ok(Some(data));
        }
        if let Err(e) = petramond_util::atomic_file::sync_dir(&self.players_dir) {
            log::warn!("could not sync the players directory: {e}");
        }
        Ok(Some(data))
    }

    /// Read a player file: `Ok(None)` when it does not exist. Any other read
    /// failure protects `key`'s saves (nothing of it could be kept aside).
    fn read_player_file(&self, path: &Path, key: &PlayerKey) -> Result<Option<Vec<u8>>, RecordError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => {
                log::error!("could not read player file {}: {e}", path.display());
                self.protect(key);
                Err(RecordError::Io {
                    format: player::FORMAT.name,
                    kind: e.kind(),
                })
            }
        }
    }

    /// Decode a player file's bytes; an unreadable one is quarantined and,
    /// when it must not be overwritten, protects `key`'s saves.
    fn decode_player_file(
        &self,
        path: &Path,
        bytes: &[u8],
        key: &PlayerKey,
    ) -> Result<player::PlayerData, RecordError> {
        let error = match player::decode(bytes, &self.palette) {
            Ok(data) => {
                let mut kept = self.kept.lock().expect("kept players");
                if data.kept.is_empty() {
                    kept.remove(key);
                } else {
                    kept.insert(*key, data.kept.clone());
                }
                return Ok(data);
            }
            Err(error) => error,
        };
        let relative = Path::new("players").join(path.file_name().unwrap_or_default());
        let unreadable = Unreadable {
            quarantined: format::quarantine(&self.dir, &relative, bytes)
                .inspect_err(|e| log::error!("could not quarantine {}: {e}", path.display()))
                .ok(),
            error,
        };
        log::error!(
            "player file {} is unreadable ({}); kept at {:?}",
            path.display(),
            unreadable.error,
            unreadable.quarantined
        );
        if unreadable.must_not_overwrite() {
            self.protect(key);
        }
        Err(unreadable.error)
    }

    fn protect(&self, key: &PlayerKey) {
        log::warn!("player {key} will not be saved this session: its file must not be overwritten");
        self.protected.lock().expect("protected players").insert(*key);
    }

    /// The identity→display-name registry bytes (`None` = none saved yet).
    pub fn load_player_registry(&self) -> Option<Vec<u8>> {
        std::fs::read(self.players_dir.join(PLAYER_REGISTRY_FILE)).ok()
    }

    /// Replace the registry atomically with generation `generation`'s bytes.
    /// Writers are serialized, and a generation older than (or equal to) the
    /// last one written is skipped: two joins whose registry updates finish
    /// out of order can never roll the file back. `Ok(false)` = skipped.
    pub fn store_player_registry(&self, generation: u64, bytes: &[u8]) -> std::io::Result<bool> {
        let mut written = self.registry_written.lock().expect("registry writer");
        if generation <= *written {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.players_dir)?;
        petramond_util::atomic_file::replace(&self.players_dir.join(PLAYER_REGISTRY_FILE), bytes)?;
        *written = generation;
        Ok(true)
    }
}
