use mod_sdk::*;

use crate::content::Content;
use crate::husbandry::BABY;

const GROW_RETRY: u64 = 200;

pub fn on_tag_removed(content: &Content, mob_id: u64, kind: MobId, key: &str) {
    if key != BABY {
        return;
    }
    let Some(def) = content.husbandry.iter().find(|d| {
        d.offspring
            .as_ref()
            .is_some_and(|(_, young)| *young == kind)
    }) else {
        return;
    };
    let Some(snap) = mob_info(mob_id) else {
        return;
    };
    if spawn_mob_checked(&def.key, snap.pos, snap.yaw).is_some() {
        despawn_mob(mob_id);
    } else {
        mob_tag_set(
            mob_id,
            BABY,
            MobTagValue::I64((current_tick() + GROW_RETRY) as i64),
        );
    }
}
