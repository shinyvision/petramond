//! World-persisted operator permissions.
//!
//! Names are matched case-insensitively and stored in the world's ordinary
//! engine KV map, so the operator set follows the same autosave/save-all path
//! as the day/night clock. The listen server's local session is intrinsically
//! an operator and does not need an entry here.

use std::collections::BTreeSet;

use crate::world::World;

const OPERATORS_KEY: &str = "petramond:operators";

pub(crate) fn canonical_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

pub(crate) fn load(world: &World) -> BTreeSet<String> {
    let Some(bytes) = world.mod_kv_get(OPERATORS_KEY) else {
        return BTreeSet::new();
    };
    match serde_json::from_slice::<BTreeSet<String>>(bytes) {
        Ok(names) => names
            .into_iter()
            .map(|name| canonical_name(&name))
            .filter(|name| !name.is_empty())
            .collect(),
        Err(e) => {
            log::warn!("ignoring malformed operator list in world data: {e}");
            BTreeSet::new()
        }
    }
}

pub(crate) fn store(world: &mut World, names: &BTreeSet<String>) {
    let bytes = serde_json::to_vec(names).expect("a string set always serializes");
    world.mod_kv_set(OPERATORS_KEY.into(), bytes);
}
