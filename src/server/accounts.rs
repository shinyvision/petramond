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

/// Identity → display name for everyone who ever joined this world.
#[derive(Debug, Default)]
pub struct PlayerRegistry {
    names: BTreeMap<PlayerKey, String>,
    /// Bumped on every change, so registry writes finishing out of order
    /// never roll the file back ([`PlayerFiles::store_player_registry`]).
    generation: u64,
}

/// A name reserved for a joining identity by [`PlayerRegistry::reserve`].
pub struct Reservation {
    /// The session's final display name (the request, possibly suffixed).
    pub name: String,
    /// Whether the registry already knew the identity before this claim —
    /// half of the first-seen test [`restore`] completes.
    pub known: bool,
    /// The registry as it now stands, when this claim changed it.
    pub changed: Option<RegistrySnapshot>,
}

/// The registry's serialized form at one generation, ready to be written by
/// whichever thread gets to it.
pub struct RegistrySnapshot {
    generation: u64,
    bytes: Vec<u8>,
}

impl RegistrySnapshot {
    /// Write this snapshot unless a newer one already landed.
    pub fn write(&self, files: &PlayerFiles) {
        if let Err(e) = files.store_player_registry(self.generation, &self.bytes) {
            log::warn!("could not write the player registry: {e}");
        }
    }
}

/// What [`restore`] found for a joining identity.
pub struct Restored {
    /// The identity's saved player, when it has one on this world.
    pub player: Option<Player>,
    /// First time this world sees the identity: the caller migrates legacy
    /// name-keyed operator rights to it.
    pub first_seen: bool,
}

/// Read `key`'s saved player: its own file, or — the first time this world
/// sees the identity — the legacy file of the name it now goes by. Blocking
/// file I/O, safe off the server thread.
///
/// An unreadable file (own or legacy) is an error, never silently a fresh
/// player: the save quarantines it and, when it must not be overwritten,
/// stops saving this identity (`PlayerFiles::load_player`). The session
/// still spawns fresh.
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

/// The outcome of [`PlayerRegistry::claim`].
pub struct Claim {
    /// The session's final display name (the request, possibly suffixed).
    pub name: String,
    /// The identity's saved player, when it has one on this world.
    pub restored: Option<Player>,
    /// First time this world sees the identity: the caller migrates legacy
    /// name-keyed operator rights to it.
    pub first_seen: bool,
}

impl PlayerRegistry {
    /// Read the registry from the save (empty without a save, or when the
    /// file is missing; a corrupt file warns and starts empty — the player
    /// files themselves are keyed by identity and stay intact).
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

    /// The display name `key` last joined with.
    pub fn name_of(&self, key: &PlayerKey) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    /// The identity that owns `name` (case-insensitive).
    pub fn key_for_name(&self, name: &str) -> Option<PlayerKey> {
        let wanted = canonical_name(name);
        self.names
            .iter()
            .find(|(_, owned)| canonical_name(owned) == wanted)
            .map(|(key, _)| *key)
    }

    /// Reserve a display name for `key` and record it at once (no I/O), so
    /// concurrent joins resolve against it. `requested` must already be a
    /// validated name (`net::identity::validate_player_name`);
    /// `in_use_by_other(candidate)` says whether a connected (or joining)
    /// session of ANOTHER identity currently goes by `candidate`. The final
    /// name is `requested` or, when another identity owns or uses it, the
    /// lowest free suffix (`{name}2`, …).
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

    /// Bind `key` to a display name and restore its saved player, all on the
    /// calling thread — the listen server's own session at startup, where
    /// nothing else is running yet. Remote joins take the split path
    /// (`server::admissions`).
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

        // Another identity asking for the name later gets neither the name
        // nor the (already migrated) legacy state.
        let mut reg = reloaded;
        let other = reg.claim(Some(&opened.save), B, "Visitor", nobody_connected);
        assert_eq!(other.name, "Visitor2");
        assert!(other.restored.is_none());

        let mut save = opened.save;
        save.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
