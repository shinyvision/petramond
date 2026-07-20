//! Juvenile growth: a lamb becomes a sheep when its `farming:baby` tag goes.
//!
//! Growth is REMOVAL-TRIGGERED, not timer-triggered: the husbandry sweep
//! merely deletes the tag when its deadline passes, and THIS handler — on the
//! engine's `mob_tag_removed` post event — performs the metamorphosis. Any
//! other remover (a future early-grow mechanic, a debug hand) grows the
//! juvenile through the same path.
//!
//! The metamorphosis is composed from primitives: read the juvenile's pose
//! (`mob_info`), checked-spawn the adult on it, despawn the juvenile. A
//! failed checked spawn (the adult body doesn't fit where the juvenile
//! stands) re-stamps the baby tag with a short extension and retries — a
//! juvenile never vanishes without its adult appearing.

use mod_sdk::*;

use crate::content::Content;
use crate::husbandry::BABY;

/// Retry delay after a growth attempt found no room for the adult body.
const GROW_RETRY: u64 = 200;

pub fn on_tag_removed(content: &Content, mob_id: u64, kind: MobId, key: &str) {
    if key != BABY {
        return;
    }
    // Which adult this juvenile grows into is spec-table data.
    let Some(def) = content.husbandry.iter().find(|d| d.offspring_kind == kind) else {
        return;
    };
    // Gone between the removal and this drain point (died, unloaded): the
    // growth simply doesn't happen — no adult from a corpse.
    let Some(snap) = mob_info(mob_id) else {
        return;
    };
    if spawn_mob_checked(def.key, snap.pos, snap.yaw).is_some() {
        despawn_mob(mob_id);
    } else {
        mob_tag_set(
            mob_id,
            BABY,
            MobTagValue::I64((current_tick() + GROW_RETRY) as i64),
        );
    }
}
