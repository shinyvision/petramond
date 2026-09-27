use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::net::identity::{canonical_name, PlayerKey};
use crate::world::ServerWorld;

const OPERATORS_KEY: &str = "petramond:operators";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Operators {
    keys: BTreeSet<PlayerKey>,
    legacy_names: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Stored {
    Current {
        keys: BTreeSet<String>,
        legacy_names: BTreeSet<String>,
    },
    Legacy(BTreeSet<String>),
}

impl Operators {
    pub fn contains(&self, key: &PlayerKey) -> bool {
        self.keys.contains(key)
    }

    pub fn insert(&mut self, key: PlayerKey) -> bool {
        self.keys.insert(key)
    }

    pub fn remove(&mut self, key: &PlayerKey) -> bool {
        self.keys.remove(key)
    }

    pub fn claim_legacy(&mut self, name: &str, key: PlayerKey) -> bool {
        if !self.legacy_names.remove(&canonical_name(name)) {
            return false;
        }
        self.keys.insert(key);
        true
    }
}

pub fn load(world: &ServerWorld) -> Operators {
    let Some(bytes) = world.data().world_kv_get(OPERATORS_KEY) else {
        return Operators::default();
    };
    let (keys, legacy_names) = match serde_json::from_slice::<Stored>(bytes) {
        Ok(Stored::Current { keys, legacy_names }) => (keys, legacy_names),
        Ok(Stored::Legacy(names)) => (BTreeSet::new(), names),
        Err(e) => {
            log::warn!("ignoring malformed operator list in world data: {e}");
            return Operators::default();
        }
    };
    Operators {
        keys: keys
            .into_iter()
            .filter_map(|key| match key.parse() {
                Ok(key) => Some(key),
                Err(e) => {
                    log::warn!("operator list: skipping entry {key:?}: {e}");
                    None
                }
            })
            .collect(),
        legacy_names: legacy_names
            .into_iter()
            .map(|name| canonical_name(&name))
            .filter(|name| !name.is_empty())
            .collect(),
    }
}

pub fn store(world: &mut ServerWorld, operators: &Operators) {
    let stored = Stored::Current {
        keys: operators.keys.iter().map(PlayerKey::to_string).collect(),
        legacy_names: operators.legacy_names.clone(),
    };
    let bytes = serde_json::to_vec(&stored).expect("string sets always serialize");
    world.world_kv_set(OPERATORS_KEY.into(), bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_name_lists_convert_to_the_first_claimant_only() {
        let mut world = ServerWorld::new(1, 2);
        world.world_kv_set(OPERATORS_KEY.into(), br#"[" Rachel ", "bob"]"#.to_vec());
        let mut ops = load(&world);
        let (rachel, mallory) = (PlayerKey([1; 32]), PlayerKey([2; 32]));
        assert!(!ops.contains(&rachel), "a bare name grants nobody yet");

        assert!(ops.claim_legacy("RACHEL", rachel));
        assert!(ops.contains(&rachel));
        assert!(
            !ops.claim_legacy("rachel", mallory),
            "the name converted once; a later claimant inherits nothing"
        );
        assert!(!ops.contains(&mallory));

        store(&mut world, &ops);
        assert_eq!(load(&world), ops, "the current shape roundtrips");
    }
}
