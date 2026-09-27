use std::collections::{BTreeMap, BTreeSet};

use crate::host::prelude::*;

use super::backoff::{Struck, Tries, Until};
use super::presence::Presentation;
use super::tuning::every::SEARCH_EVERY;
use super::tuning::patience::FACELESS_TRIES;
use super::tuning::waits::FACELESS_SPACING;
use super::waiting::Waiting;
use super::{
    bridge, rescue, route, Pillar, Step, Task, FACE_TAG, GOAL_TAG, HOLD_TAG, LOOK_TAG, PROJECT_TAG,
};
use crate::fx::{HashMap, HashSet};
use crate::survey::ItemKey;

#[derive(Default)]
pub struct Crew {
    pub mob: Option<u64>,
    pub last_mob: Option<u64>,
    pub(super) searched: u64,
    pub step: Step,
    pub built: HashSet<usize>,
    pub note: String,
    pub(super) why: Waiting,
    pub(super) trail: route::Trail,
    pub(super) presence: Presentation,
    pub(super) deferrals: Deferrals,
    pub(super) faces: Faces,
    pub(super) aloft: Aloft,
    pub(super) access: Access,
    pub(super) scaffolding: ScaffoldState,
    pub(super) pace: Pace,
    pub(super) glazing: GlazingState,
    pub(super) cargo: CargoState,
    pub(super) rescue: Rescue,
}

#[derive(Default)]
pub struct Deferrals {
    pub(super) deferred: Until<Task>,
    pub(super) blind: Struck<(Task, [i32; 3])>,
    pub(super) tried: HashSet<usize>,
    pub(super) bury_waits: Tries<usize>,
    pub(super) cutters: HashSet<usize>,
    pub(super) cutter_waits: Tries<usize>,
    pub(super) support_waits: Tries<usize>,
    site_changes: u64,
    nowhere: HashMap<Task, ([i32; 3], u64, u64, bool)>,
}

impl Deferrals {
    pub(super) fn site_changed(&mut self) {
        self.site_changes += 1;
    }

    pub(super) fn found_nowhere(&mut self, task: Task, from: [i32; 3], until: u64, unseen: bool) {
        self.nowhere
            .insert(task, (from, self.site_changes, until, unseen));
    }

    pub(super) fn still_nowhere(&self, task: Task, from: [i32; 3], now: u64) -> Option<bool> {
        let &(at, changes, until, unseen) = self.nowhere.get(&task)?;
        (at == from && changes == self.site_changes && now < until).then_some(unseen)
    }

    pub(super) fn defer(&mut self, task: Task, until: u64) {
        self.deferred.set(task, until);
    }

    pub(super) fn deferred(&self, task: Task, now: u64) -> bool {
        self.deferred.holds(&task, now)
    }

    pub(super) fn blind(&self, task: Task, stance: [i32; 3]) -> bool {
        self.blind.struck(&(task, stance))
    }

    pub(super) fn strike(&mut self, task: Task, stance: [i32; 3]) {
        self.blind.strike((task, stance));
    }
}

#[derive(Default)]
pub struct Faces {
    pub(super) floating: HashSet<usize>,
    pub(super) faceless: Tries<usize>,
    pub(super) propped: HashMap<usize, Vec<[i32; 3]>>,
    pub(super) hangs: HashSet<usize>,
    pub(super) unheld: HashSet<usize>,
}

impl Faces {
    pub(super) fn faceless_try(&mut self, i: usize, now: u64) -> bool {
        self.faceless.count_spaced(i, now, FACELESS_SPACING) >= FACELESS_TRIES
    }

    pub(super) fn waits_on_the_build(&self, unit: usize) -> bool {
        self.hangs.contains(&unit) || self.floating.contains(&unit) || self.unheld.contains(&unit)
    }

    pub(super) fn unprop(&mut self, unit: usize) -> Option<Vec<[i32; 3]>> {
        let props = self.propped.remove(&unit)?;
        self.floating.remove(&unit);
        self.faceless.forget(&unit);
        Some(props)
    }
}

#[derive(Default)]
pub struct Aloft {
    pub(super) perch: Option<Pillar>,
    pub(super) climbed_for: Option<(Task, u32)>,
    pub(super) raises: u8,
    pub(super) bridge: Option<bridge::Bridge>,
    pub(super) settle_until: u64,
    pub(super) aimed: Option<(Task, [i32; 3])>,
    pub(super) descending: Option<i32>,
}

impl Aloft {
    pub(super) fn dismount_lost(&mut self) {
        self.perch = None;
        self.climbed_for = None;
    }

    pub(super) fn dismount_to_raise(&mut self) {
        self.perch = None;
    }

    pub(super) fn dismount_down(&mut self) {
        self.perch = None;
        self.descending = None;
    }
}

#[derive(Default)]
pub struct Access {
    pub(super) unreachable: HashSet<usize>,
    pub(super) reopen: HashMap<usize, Vec<usize>>,
    pub(super) trims: HashSet<[i32; 3]>,
    pub(super) digs: HashSet<[i32; 3]>,
    pub(super) no_go: Vec<([i32; 3], [i32; 3])>,
    pub(super) door_tried: Until<[i32; 3]>,
    pub(super) door_scan_at: u64,
    pub(super) dig_scan_at: u64,
    pub(super) way_in: Option<(Vec<[i32; 3]>, u64)>,
    pub(super) opened: Vec<[i32; 3]>,
    pub(super) pending_use: Option<[i32; 3]>,
}

#[derive(Default)]
pub struct ScaffoldState {
    pub(super) block: Option<BlockRecord>,
    pub(super) cells: HashSet<[i32; 3]>,
    pub(super) want: u32,
    pub(super) short: bool,
    pub(super) urgent: Vec<[i32; 3]>,
    pub(super) shunned: Tries<[i32; 3]>,
    pub(super) left_standing: u32,
}

#[derive(Default)]
pub struct Pace {
    pub(super) progress_at: u64,
    pub(super) busy_at: u64,
    pub(super) placed_at: u64,
    pub(super) band_top: Option<i32>,
    pub(super) band_progress_at: u64,
    pub(super) focus: Option<[i32; 3]>,
    pub(super) cursor: usize,
    pub(super) home_checked: u64,
}

#[derive(Default)]
pub struct GlazingState {
    pub(super) ahead: HashSet<usize>,
    pub(super) ways: Stamped<HashSet<usize>>,
    pub(super) under_way: bool,
}

#[derive(Default)]
pub struct CargoState {
    pub(super) in_reach: Stamped<Option<BTreeMap<ItemKey, u32>>>,
    pub(super) tool_trip: Stamped<BTreeSet<String>>,
    pub(super) chests_full: bool,
}

#[derive(Default)]
pub struct Rescue {
    pub(super) stuck: Option<rescue::Stuck>,
    pub(super) site_loaded_since: Option<u64>,
    pub(super) off_route_since: Option<u64>,
    pub(super) stalled: ([i32; 3], u8),
    pub(super) burrow_from: [i32; 3],
}

#[derive(Default)]
pub struct Stamped<T> {
    pub(super) at: u64,
    pub(super) value: T,
}

impl<T> Stamped<T> {
    pub(super) fn stale(&self, now: u64, every: u64) -> bool {
        now >= self.at + every || self.at == 0
    }
}

impl Crew {
    pub fn site_changed(&mut self) {
        self.deferrals.site_changed();
    }

    pub(super) fn why(&mut self, reason: Waiting) {
        if self.why != reason {
            trace!("TRACE plan waits: {}", reason.label());
        }
        self.why = reason;
    }

    pub(super) fn find(&mut self, now: u64, project: u64) -> Option<u64> {
        if let Some(id) = self.mob {
            if mob_info(id).is_some() {
                return Some(id);
            }
            self.mob = None;
            self.presence.forget_tags();
        }
        if now < self.searched + SEARCH_EVERY && self.searched != 0 {
            return None;
        }
        self.searched = now;
        let id = mobs_with_tag(PROJECT_TAG, Some(MobTagValue::I64(project as i64)))
            .first()
            .map(|m| m.id)?;
        mob_tag_delete(id, GOAL_TAG);
        mob_tag_delete(id, HOLD_TAG);
        mob_tag_delete(id, FACE_TAG);
        mob_tag_delete(id, LOOK_TAG);
        mob_held_display(id, None, None);
        set_mob_draw(id, DrawFrame::World, Vec::new());
        self.presence.mark = None;
        self.pace.busy_at = now;
        self.mob = Some(id);
        self.last_mob = Some(id);
        self.step = Step::Plan;
        self.pace.progress_at = now;
        self.pace.band_progress_at = now;
        Some(id)
    }
}
