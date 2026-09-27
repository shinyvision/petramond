use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::format::{self, RecordError};
use super::palette::Palette;
use super::player;
use super::worlds::{legacy_player_path, player_path};
use super::Unreadable;
use crate::net::identity::PlayerKey;

const PLAYER_REGISTRY_FILE: &str = "names.json";

pub struct PlayerFiles {
    players_dir: PathBuf,
    dir: PathBuf,
    palette: Arc<Palette>,
    protected: Mutex<HashSet<PlayerKey>>,
    kept: Mutex<HashMap<PlayerKey, player::KeptPlayer>>,
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

    pub(super) fn encode_for_save(
        &self,
        key: &PlayerKey,
        player: &crate::player::Player,
    ) -> Option<Vec<u8>> {
        if self
            .protected
            .lock()
            .expect("protected players")
            .contains(key)
        {
            return None;
        }
        Some(match self.kept.lock().expect("kept players").get(key) {
            Some(kept) => player::encode_keeping(player, &self.palette, kept),
            None => player::encode(player, &self.palette),
        })
    }

    pub fn load_player(&self, key: &PlayerKey) -> Result<Option<player::PlayerData>, RecordError> {
        let path = player_path(&self.players_dir, key);
        let Some(bytes) = self.read_player_file(&path, key)? else {
            return Ok(None);
        };
        self.decode_player_file(&path, &bytes, key).map(Some)
    }

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
            log::warn!(
                "could not migrate legacy player file {}: {e}",
                legacy.display()
            );
            return Ok(Some(data));
        }
        if let Err(e) = petramond_persist::atomic_file::sync_dir(&self.players_dir) {
            log::warn!("could not sync the players directory: {e}");
        }
        Ok(Some(data))
    }

    fn read_player_file(
        &self,
        path: &Path,
        key: &PlayerKey,
    ) -> Result<Option<Vec<u8>>, RecordError> {
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
        self.protected
            .lock()
            .expect("protected players")
            .insert(*key);
    }

    pub fn load_player_registry(&self) -> Option<Vec<u8>> {
        std::fs::read(self.players_dir.join(PLAYER_REGISTRY_FILE)).ok()
    }

    pub fn store_player_registry(&self, generation: u64, bytes: &[u8]) -> std::io::Result<bool> {
        let mut written = self.registry_written.lock().expect("registry writer");
        if generation <= *written {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.players_dir)?;
        petramond_persist::atomic_file::replace(
            &self.players_dir.join(PLAYER_REGISTRY_FILE),
            bytes,
        )?;
        *written = generation;
        Ok(true)
    }
}
