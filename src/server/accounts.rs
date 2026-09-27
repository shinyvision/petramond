//! Player accounts: which authenticated identity ([`PlayerKey`]) goes by
//! which display name on this world.
//!
//! The registry is the ONE place a name maps to an identity. Saves and
//! operator rights key on the identity; names are only for display and for
//! commands typed by people (`op <name>`). A display name belongs to the
//! identity that last used it, so two players can never both answer to one
//! name: a join asking for a name another identity owns (or a connected
//! session is using) gets the lowest free numeric suffix instead.
//!
//! Persisted as `players/names.json` (`{"<hex key>": "<name>"}`), rewritten
//! whenever an identity first joins or changes its name. A remote join
//! splits its claim in two: the name is reserved on the server thread
//! ([`PlayerRegistry::reserve`], no I/O), and the player file read plus the
//! registry write run off it ([`restore`], [`RegistrySnapshot::write`]).
//!
//! Migration from pre-identity worlds: the FIRST time an identity is seen,
//! it adopts the legacy `players/<name>.dat` of the name it ends up with
//! (`WorldSave::adopt_legacy_player`, which moves the file so nobody else can
//! adopt it), and a legacy operator entry for that name
//! (`permissions::Operators::claim_legacy`). That is trust-on-first-use for
//! old worlds only; everything after the first claim is authenticated.

use std::collections::BTreeMap;

use crate::net::identity::{canonical_name, PlayerKey};
use crate::player::Player;
use crate::save::{PlayerFiles, WorldSave};

#[derive(Debug, Default)]
pub struct PlayerRegistry {
    names: BTreeMap<PlayerKey, String>,
    generation: u64,
}

pub struct Reservation {
    pub name: String,
    pub known: bool,
    pub changed: Option<RegistrySnapshot>,
}

pub struct RegistrySnapshot {
    generation: u64,
    bytes: Vec<u8>,
}

impl RegistrySnapshot {
    pub fn write(&self, files: &PlayerFiles) {
        if let Err(e) = files.store_player_registry(self.generation, &self.bytes) {
            log::warn!("could not write the player registry: {e}");
        }
    }
}

pub struct Restored {
    pub player: Option<Player>,
    pub first_seen: bool,
}

pub fn restore(files: Option<&PlayerFiles>, key: PlayerKey, name: &str, known: bool) -> Restored {
    let own = files.map_or(Ok(None), |f| f.load_player(&key));
    let first_seen = !known && matches!(own, Ok(None));
    let loaded = if first_seen {
        files.map_or(Ok(None), |f| f.adopt_legacy_player(name, &key))
    } else {
        own
    };
    let player = match loaded {
        Ok(data) => data.map(|data| data.restore()),
        Err(e) => {
            log::error!("player '{name}' ({key}) spawns fresh: {e}");
            None
        }
    };
    Restored { player, first_seen }
}

pub struct Claim {
    pub name: String,
    pub restored: Option<Player>,
    pub first_seen: bool,
}

impl PlayerRegistry {
    pub fn load(save: Option<&WorldSave>) -> PlayerRegistry {
        let Some(bytes) = save.and_then(WorldSave::load_player_registry) else {
            return PlayerRegistry::default();
        };
        match serde_json::from_slice::<BTreeMap<String, String>>(&bytes) {
            Ok(raw) => PlayerRegistry {
                generation: 0,
                names: raw
                    .into_iter()
                    .filter_map(|(key, name)| match key.parse::<PlayerKey>() {
                        Ok(key) => Some((key, name)),
                        Err(e) => {
                            log::warn!("player registry: skipping entry {key:?}: {e}");
                            None
                        }
                    })
                    .collect(),
            },
            Err(e) => {
                log::warn!("ignoring malformed player registry: {e}");
                PlayerRegistry::default()
            }
        }
    }

    fn snapshot(&self) -> RegistrySnapshot {
        let raw: BTreeMap<String, &str> = self
            .names
            .iter()
            .map(|(key, name)| (key.to_string(), name.as_str()))
            .collect();
        RegistrySnapshot {
            generation: self.generation,
            bytes: serde_json::to_vec_pretty(&raw).expect("a string map always serializes"),
        }
    }

    pub fn name_of(&self, key: &PlayerKey) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    pub fn key_for_name(&self, name: &str) -> Option<PlayerKey> {
        let wanted = canonical_name(name);
        self.names
            .iter()
            .find(|(_, owned)| canonical_name(owned) == wanted)
            .map(|(key, _)| *key)
    }

    pub fn reserve(
        &mut self,
        key: PlayerKey,
        requested: &str,
        in_use_by_other: impl Fn(&str) -> bool,
    ) -> Reservation {
        let taken = |candidate: &str| {
            in_use_by_other(candidate) || self.key_for_name(candidate).is_some_and(|k| k != key)
        };
        let name = if taken(requested) {
            (2u32..)
                .map(|n| format!("{requested}{n}"))
                .find(|candidate| !taken(candidate))
                .expect("a finite registry leaves a free suffix")
        } else {
            requested.to_string()
        };
        let known = self.names.contains_key(&key);
        let changed = (self.names.get(&key) != Some(&name)).then(|| {
            self.names.insert(key, name.clone());
            self.generation += 1;
            self.snapshot()
        });
        Reservation {
            name,
            known,
            changed,
        }
    }

    pub fn claim(
        &mut self,
        save: Option<&WorldSave>,
        key: PlayerKey,
        requested: &str,
        in_use_by_other: impl Fn(&str) -> bool,
    ) -> Claim {
        let reservation = self.reserve(key, requested, in_use_by_other);
        let files = save.map(WorldSave::player_files);
        let restored = restore(files.as_deref(), key, &reservation.name, reservation.known);
        if let (Some(files), Some(changed)) = (&files, &reservation.changed) {
            changed.write(files);
        }
        Claim {
            name: reservation.name,
            restored: restored.player,
            first_seen: restored.first_seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PlayerKey = PlayerKey([1; 32]);
    const B: PlayerKey = PlayerKey([2; 32]);

    fn nobody_connected(_: &str) -> bool {
        false
    }

    #[test]
    fn a_name_belongs_to_one_identity_and_others_get_a_suffix() {
        let mut reg = PlayerRegistry::default();
        let a = reg.claim(None, A, "Rachel", nobody_connected);
        assert_eq!(a.name, "Rachel");
        assert!(a.first_seen);

        let b = reg.claim(None, B, "rachel", nobody_connected);
        assert_eq!(b.name, "rachel2", "A owns the name even while offline");
        assert_eq!(reg.key_for_name("RACHEL"), Some(A));
        assert_eq!(reg.key_for_name("Rachel2"), Some(B));

        let again = reg.claim(None, A, "Rachel", nobody_connected);
        assert_eq!(again.name, "Rachel", "the owner keeps its name");
        assert!(!again.first_seen);
    }

    #[test]
    fn renaming_frees_the_old_name_and_connected_sessions_block_theirs() {
        let mut reg = PlayerRegistry::default();
        reg.claim(None, A, "Old", nobody_connected);
        assert_eq!(reg.claim(None, A, "New", nobody_connected).name, "New");
        assert_eq!(reg.name_of(&A), Some("New"));
        assert_eq!(
            reg.claim(None, B, "Old", nobody_connected).name,
            "Old",
            "a released name is free again"
        );

        let host_uses = |candidate: &str| candidate.eq_ignore_ascii_case("Host");
        assert_eq!(reg.claim(None, A, "Host", host_uses).name, "Host2");
    }

    #[test]
    fn the_registry_persists_and_legacy_files_migrate_once() {
        let dir = std::env::temp_dir().join(format!(
            "petramond-accounts-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("temp save opens");
        std::fs::create_dir_all(dir.join("players")).expect("players dir");
        let mut legacy = Player::new(petramond_math::world_pos::WorldPos::new(3.0, 70.0, 4.0));
        legacy.inventory.set_active(5);
        std::fs::write(
            dir.join("players/Visitor.dat"),
            crate::save::player::encode(&legacy, opened.save.palette()),
        )
        .expect("legacy file");

        let mut reg = PlayerRegistry::load(Some(&opened.save));
        let claim = reg.claim(Some(&opened.save), A, "Visitor", nobody_connected);
        let restored = claim.restored.expect("the legacy player migrates");
        assert_eq!(restored.inventory.active_slot(), 5);

        let reloaded = PlayerRegistry::load(Some(&opened.save));
        assert_eq!(reloaded.name_of(&A), Some("Visitor"));

        let mut reg = reloaded;
        let other = reg.claim(Some(&opened.save), B, "Visitor", nobody_connected);
        assert_eq!(other.name, "Visitor2");
        assert!(other.restored.is_none());

        let mut save = opened.save;
        save.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
