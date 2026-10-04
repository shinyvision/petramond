//! Keeping the camps manned. Posts are read from each section the stream announces; once a second
//! the live skeletons are counted, every post whose guard is gone is refilled out of the players'
//! sight (while its camp flies its flag, when camps have flags), and every skeleton this session
//! has not dressed yet is dressed: what its hands hold and its stances are not saved, its loadout
//! tag is.

use mod_sdk::*;

use super::combat::{clear_line, wanted, Fight};
use super::keys::{FACING_TAG, LOADOUT_TAG, POST_TAG, ROLE_TAG, SKELETON};
use super::kit::pick;
use super::posts::{may_fill, standing_point, Marker, Scan};
use super::presence::{self, Presence};
use super::{Body, Skeletons};
use crate::keys::POST_MARKER;

const CENSUS: Cadence = Cadence::every(20);

/// Height above the feet a post is watched at.
const WATCHED_AT: f64 = 1.0;

pub fn tick(sk: &mut Skeletons) {
    let now = current_tick();
    scan(sk, now);
    if !CENSUS.due(now, 0) {
        return;
    }
    let live = mobs_with_tag(LOADOUT_TAG, None);
    let ids: FxHashSet<u64> = live.iter().map(|m| m.id).collect();
    sk.bodies.retain(|id, _| ids.contains(id));
    let strangers = live
        .iter()
        .map(|m| m.id)
        .filter(|id| !sk.bodies.contains_key(id))
        .collect();
    enlist(sk, strangers);
    let manned: FxHashSet<[i32; 3]> = sk.bodies.values().filter_map(|b| b.post).collect();
    sk.posts.census(|cell| manned.contains(cell));
    let roster: Vec<PlayerSnapshot> = players().into_iter().map(|p| p.state).collect();
    let eyes: Vec<[f64; 3]> = roster
        .iter()
        .map(|s| [s.pos[0], s.pos[1] + f64::from(s.eye_height), s.pos[2]])
        .collect();
    if let Some(standards) = &mut sk.standards {
        standards.follow();
    }
    refill(sk, now, &eyes);
}

fn scan(sk: &mut Skeletons, now: u64) {
    for section in sk.posts.pending() {
        let scan = match section_kv_find(section, POST_MARKER) {
            None => Scan::NotReady,
            Some(cells) if cells.is_empty() => Scan::Found(Vec::new()),
            Some(cells) => {
                let values = section_kv_get_many(POST_MARKER, cells.clone());
                Scan::Found(
                    cells
                        .into_iter()
                        .zip(values)
                        .filter_map(|(cell, bytes)| Some((cell, Marker::decode(&bytes?)?)))
                        .collect(),
                )
            }
        };
        sk.posts.settle(section, scan, now);
    }
}

fn refill(sk: &mut Skeletons, now: u64, eyes: &[[f64; 3]]) {
    for (cell, marker) in sk.posts.due(now) {
        let feet = standing_point(cell);
        let watched = [feet[0], feet[1] + WATCHED_AT, feet[2]];
        let allowed = if sk.posts.fresh(&cell) {
            may_fill(watched, eyes, |_, _| false)
        } else {
            may_fill(watched, eyes, clear_line)
        };
        if !allowed
            || sk
                .standards
                .as_mut()
                .is_some_and(|s| !s.flies(marker.camp, now))
        {
            continue;
        }
        let roll = splitmix64_mix(rng_u64("skeleton_post") ^ cell_hash(cell));
        let yaw = marker
            .yaw
            .unwrap_or_else(|| (roll >> 40) as f32 / (1u64 << 24) as f32 * std::f32::consts::TAU);
        let Some(id) = spawn_mob_checked(SKELETON, feet, yaw) else {
            continue;
        };
        let loadout = pick(&sk.kits.loadouts, marker.watch(), splitmix64_mix(roll));
        let name = loadout.map_or("", |i| sk.kits.loadouts[i].name.as_str());
        let mut writes = vec![
            set(id, LOADOUT_TAG, MobTagValue::Str(name.to_owned())),
            set(id, POST_TAG, cell.to_tag()),
            set(id, ROLE_TAG, MobTagValue::I64(i64::from(marker.role))),
        ];
        if let Some(yaw) = marker.yaw {
            writes.push(set(id, FACING_TAG, MobTagValue::F64(f64::from(yaw))));
        }
        mob_tags_write(writes);
        let body = Body {
            loadout,
            post: Some(cell),
            watch: marker.watch(),
            presence: Presence::default(),
            fight: Fight::default(),
        };
        dress(sk, id, body);
        sk.posts.set_occupied(cell);
    }
}

/// Takes on skeletons this session has not met yet: reads what their tags say they are, gives one
/// with no loadout it knows a fresh one, and dresses it.
pub fn enlist(sk: &mut Skeletons, ids: Vec<u64>) {
    if ids.is_empty() {
        return;
    }
    let tags = paged(ids.clone(), mob_tags_get_many);
    for (id, tags) in ids.into_iter().zip(tags) {
        let Some(tags) = tags else {
            continue;
        };
        let get = |key: &str| tags.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let post = get(POST_TAG).and_then(<[i32; 3]>::from_tag);
        let watch = post.and_then(|p| sk.posts.get(&p)).map_or_else(
            || get(ROLE_TAG) == Some(&MobTagValue::I64(i64::from(super::posts::WATCH_ROLE))),
            |p| p.marker.watch(),
        );
        let known = match get(LOADOUT_TAG) {
            Some(MobTagValue::Str(name)) => sk.kits.index_of(name),
            _ => None,
        };
        let loadout = known.or_else(|| {
            let fresh = pick(
                &sk.kits.loadouts,
                watch,
                splitmix64_mix(rng_u64("skeleton_loadout") ^ id),
            );
            let name = fresh.map_or("", |i| sk.kits.loadouts[i].name.as_str());
            mob_tag_set(id, LOADOUT_TAG, MobTagValue::Str(name.to_owned()));
            fresh
        });
        let body = Body {
            loadout,
            post,
            watch,
            presence: Presence::default(),
            fight: Fight::default(),
        };
        dress(sk, id, body);
    }
}

fn dress(sk: &mut Skeletons, id: u64, mut body: Body) {
    let kit = sk.kits.kit(body.loadout);
    if let Some([main, off]) = body
        .presence
        .display(kit.held.each_ref().map(|v| v.as_deref()))
    {
        mob_held_display(id, main, off);
    }
    let mut ops = Vec::new();
    body.presence
        .frame(id, &wanted(kit, &body.fight, false), &[], &mut ops);
    presence::send(ops);
    sk.bodies.insert(id, body);
}

fn set(mob_id: u64, key: &str, value: MobTagValue) -> MobTagOp {
    MobTagOp::Set {
        mob_id,
        key: key.to_owned(),
        value,
    }
}

fn cell_hash([x, y, z]: [i32; 3]) -> u64 {
    splitmix64_mix((x as u32 as u64) | ((z as u32 as u64) << 32)) ^ (y as u32 as u64)
}
