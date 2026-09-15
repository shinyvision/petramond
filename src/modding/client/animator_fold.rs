//! The local player's animator claims as this client presents them: the
//! server's answer, with every key a client mod owns predicted by the
//! server's own resolution — the LAST client mod in mod-id order holding a
//! claim on the key wins it, and one that released its claim uncovers the
//! claim beneath instead of erasing it.

use std::collections::{BTreeMap, BTreeSet};

use super::state::AnimatorOwnership;
use crate::player::{AnimatorClaims, AnimatorParam, AnimatorPlay, RigId};

/// Fold `replicated` with `stores` — each client mod's owned keys and its
/// resolved claims, in mod-id order. A key any client mod owns is predicted
/// (and absent when none of them holds a claim on it); every other key keeps
/// the replicated answer.
pub(super) fn fold<'a>(
    replicated: &AnimatorClaims,
    stores: impl Iterator<Item = (&'a AnimatorOwnership, &'a AnimatorClaims)>,
) -> AnimatorClaims {
    let mut owned_params: BTreeSet<(RigId, u16)> = BTreeSet::new();
    let mut owned_slots: BTreeSet<(RigId, u16)> = BTreeSet::new();
    let mut params: BTreeMap<(RigId, u16), AnimatorParam> = BTreeMap::new();
    let mut plays: BTreeMap<(RigId, u16), AnimatorPlay> = BTreeMap::new();
    for (owns, claims) in stores {
        owned_params.extend(&owns.params);
        owned_slots.extend(&owns.slots);
        for p in &claims.params {
            params.insert((p.rig, p.param), p.clone());
        }
        for p in &claims.plays {
            plays.insert((p.rig, p.slot), *p);
        }
    }
    for p in &replicated.params {
        if !owned_params.contains(&(p.rig, p.param)) {
            params.insert((p.rig, p.param), p.clone());
        }
    }
    for p in &replicated.plays {
        if !owned_slots.contains(&(p.rig, p.slot)) {
            plays.insert((p.rig, p.slot), *p);
        }
    }
    AnimatorClaims {
        params: params.into_values().collect(),
        plays: plays.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::AnimatorClock;

    /// Two client mods claim one slot: the later in mod-id order wins, and
    /// once it releases, the earlier mod's claim shows — as the server
    /// resolves it — never nothing, and never the replicated answer.
    #[test]
    fn a_released_later_mod_uncovers_the_earlier_mods_claim_on_the_same_key() {
        let play = |slot: u16, clip: u16| AnimatorPlay {
            rig: RigId(0),
            slot,
            clip,
            clock: AnimatorClock::Scrub(0.5),
            mirror: false,
            priority: 0,
        };
        let owns = AnimatorOwnership {
            params: BTreeSet::new(),
            slots: BTreeSet::from([(RigId(0), 2)]),
        };
        let claims = |plays: Vec<AnimatorPlay>| AnimatorClaims { params: Vec::new(), plays };
        let replicated = claims(vec![play(2, 9), play(3, 7)]);
        let (earlier, later, released) = (claims(vec![play(2, 1)]), claims(vec![play(2, 2)]), claims(Vec::new()));
        let shown = |stores: &[(&AnimatorOwnership, &AnimatorClaims)]| {
            fold(&replicated, stores.iter().copied())
                .plays
                .iter()
                .map(|p| (p.slot, p.clip))
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&[(&owns, &earlier), (&owns, &later)]), [(2, 2), (3, 7)]);
        assert_eq!(
            shown(&[(&owns, &earlier), (&owns, &released)]),
            [(2, 1), (3, 7)],
            "the release uncovers the earlier claim"
        );
        assert_eq!(
            shown(&[(&owns, &released)]),
            [(3, 7)],
            "an owned key no mod claims is predicted empty, not replicated"
        );
    }
}
