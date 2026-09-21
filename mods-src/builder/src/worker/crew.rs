//! What a job remembers about its golem between ticks.

use std::collections::{BTreeMap, BTreeSet};

use mod_sdk::*;

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
    /// The last golem this job had, kept when it goes missing: a dying golem
    /// stops answering before its death is announced.
    pub last_mob: Option<u64>,
    pub(super) searched: u64,
    pub step: Step,
    /// Units this golem placed. One found missing again is not rebuilt.
    pub built: HashSet<usize>,
    pub note: String,
    /// Why the last plan produced no step.
    pub(super) why: Waiting,
    /// Where the golem stood, for asking routes in pieces.
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

/// Work set aside, and for how long or how often.
#[derive(Default)]
pub struct Deferrals {
    pub(super) deferred: HashMap<Task, u64>,
    /// Stances a task was refused from for reach or sight.
    pub(super) blind: HashSet<(Task, [i32; 3])>,
    /// Units tried from a roof course since the last block landed.
    pub(super) tried: HashSet<usize>,
    /// Rounds each placement has waited for digging it would bury.
    pub(super) bury_waits: HashMap<usize, u8>,
    /// Units that would wall off ground the golem still reaches: built last.
    pub(super) cutters: HashSet<usize>,
    pub(super) cutter_waits: HashMap<usize, u8>,
    /// Rounds each unit's support column has waited for ground it would cut off.
    pub(super) support_waits: HashMap<usize, u8>,
    /// Counts the block changes in and around the site: what a stance search
    /// found stands until it moves on.
    site_changes: u64,
    /// Tasks no stance was found for: from which cell, at which count of site
    /// changes, until when, and whether stances stood but none saw the work.
    nowhere: HashMap<Task, ([i32; 3], u64, u64, bool)>,
}

impl Deferrals {
    /// Blocks changed in or around the site.
    pub(super) fn site_changed(&mut self) {
        self.site_changes += 1;
    }

    /// No stance was found for `task` from `from`; `unseen` = some stood but
    /// none saw the work. Searching again finds the same until the site
    /// changes or the golem stands elsewhere — or `until`, since routes that
    /// failed are forgiven with time.
    pub(super) fn found_nowhere(&mut self, task: Task, from: [i32; 3], until: u64, unseen: bool) {
        self.nowhere
            .insert(task, (from, self.site_changes, until, unseen));
    }

    /// The standing answer of a stance search for `task` from `from`, if one
    /// still holds: whether stances stood but none saw the work.
    pub(super) fn still_nowhere(&self, task: Task, from: [i32; 3], now: u64) -> Option<bool> {
        let &(at, changes, until, unseen) = self.nowhere.get(&task)?;
        (at == from && changes == self.site_changes && now < until).then_some(unseen)
    }

    pub(super) fn defer(&mut self, task: Task, until: u64) {
        self.deferred.insert(task, until);
    }

    pub(super) fn deferred(&self, task: Task, now: u64) -> bool {
        self.deferred.get(&task).is_some_and(|&t| t > now)
    }

    /// Whether the golem already learned it cannot see `task` from `stance`.
    pub(super) fn blind(&self, task: Task, stance: [i32; 3]) -> bool {
        self.blind.contains(&(task, stance))
    }

    /// Never `stance` for `task` again.
    pub(super) fn strike(&mut self, task: Task, stance: [i32; 3]) {
        self.blind.insert((task, stance));
    }

    /// Count another round unit `i` has waited in `waits`; whether it still
    /// waits, having waited no more than `most`.
    pub(super) fn round(waits: &mut HashMap<usize, u8>, i: usize, most: u8) -> bool {
        let rounds = waits.entry(i).or_default();
        *rounds += 1;
        *rounds <= most
    }
}

/// Units with nothing to be placed against, and what props them up.
#[derive(Default)]
pub struct Faces {
    /// Units refused for having nothing to be placed against, and how often.
    pub(super) floating: HashSet<usize>,
    pub(super) faceless: HashMap<usize, (u8, u64)>,
    /// Support scaffolds placed for a unit, taken down once it stands.
    pub(super) propped: HashMap<usize, Vec<[i32; 3]>>,
    /// Units a prop brought no usable face: the click that builds them lands
    /// on the build itself (a fixture hung from a chain), so they wait for it.
    pub(super) hangs: HashSet<usize>,
    /// Units the world does not hold up yet (a chain under a roof block still
    /// to be laid), as last asked.
    pub(super) unheld: HashSet<usize>,
}

impl Faces {
    /// Count a no-face refusal of unit `i`; true once it has lasted long enough
    /// for scaffolding. Asks close together are one try, since on a wall top
    /// every block laid asks again.
    pub(super) fn faceless_try(&mut self, i: usize, now: u64) -> bool {
        let (tries, at) = self.faceless.entry(i).or_default();
        if *tries == 0 || now >= *at + FACELESS_SPACING {
            *tries += 1;
            *at = now;
        }
        *tries >= FACELESS_TRIES
    }

    /// Whether `unit` waits for the build itself to bring what it is placed
    /// on or against. Such a unit must not hold the layer band down: what it
    /// waits for may lie above the band, and then nothing moves until the
    /// band's patience runs out.
    pub(super) fn waits_on_the_build(&self, unit: usize) -> bool {
        self.hangs.contains(&unit) || self.floating.contains(&unit) || self.unheld.contains(&unit)
    }

    /// Give up propping `unit`: its supports, to be taken down, and the unit
    /// planned afresh.
    pub(super) fn unprop(&mut self, unit: usize) -> Option<Vec<[i32; 3]>> {
        let props = self.propped.remove(&unit)?;
        self.floating.remove(&unit);
        self.faceless.remove(&unit);
        Some(props)
    }
}

/// Up a pillar, along its course, or out on a walkway from it.
#[derive(Default)]
pub struct Aloft {
    pub(super) perch: Option<Pillar>,
    /// The task the current pillar was climbed for, and the work done from it.
    pub(super) climbed_for: Option<(Task, u32)>,
    /// How often the pillar stood on was raised for work it did not see.
    pub(super) raises: u8,
    /// The scaffold walkway out from the current pillar, while it stands.
    pub(super) bridge: Option<bridge::Bridge>,
    /// Until when a golem just up a pillar settles onto its centre before
    /// judging what it sees from there.
    pub(super) settle_until: u64,
    /// The task a walk along a pillar's course went for, and the stance.
    pub(super) aimed: Option<(Task, [i32; 3])>,
    /// Coming down a pillar a level at a time, working what each level sees
    /// on the way: the level last looked from.
    pub(super) descending: Option<i32>,
}

impl Aloft {
    /// Off the pillar with no way back onto it (fallen off its course, sunk
    /// into the ground): what it was climbed for is forgotten with it.
    pub(super) fn dismount_lost(&mut self) {
        self.perch = None;
        self.climbed_for = None;
    }

    /// Off the pillar only to climb it again a few levels higher.
    pub(super) fn dismount_to_raise(&mut self) {
        self.perch = None;
    }

    /// The way down is over, at the ground or short of it.
    pub(super) fn dismount_down(&mut self) {
        self.perch = None;
        self.descending = None;
    }
}

/// Ways to work that ground, doors or the build itself shut off.
#[derive(Default)]
pub struct Access {
    /// Units no stance, pillar or roof course reaches: what walkways are for.
    pub(super) unreachable: HashSet<usize>,
    /// Units sealed in on every side, and the built unit beside each taken
    /// back down (and kept down) to open a way in.
    pub(super) reopen: HashMap<usize, Vec<usize>>,
    /// Overgrowth to cut away around work nothing reaches.
    pub(super) trims: HashSet<[i32; 3]>,
    /// Earth lying ON work the golem cannot otherwise reach or see: a course
    /// laid into a bank is uncovered before it is laid.
    pub(super) digs: HashSet<[i32; 3]>,
    /// Steps a way in asked for that the navigator then refused: the grid a
    /// way is weighed on is the mod's own model, and where it and the world
    /// disagree the world is right.
    pub(super) no_go: Vec<([i32; 3], [i32; 3])>,
    /// Doors opened to reach work, and when: one is not tried twice over.
    pub(super) door_tried: HashMap<[i32; 3], u64>,
    /// When the golem last looked for a door standing in the way of work.
    pub(super) door_scan_at: u64,
    /// When it last weighed digging its way to work ground shuts it out of.
    pub(super) dig_scan_at: u64,
    /// The work it is digging its way to, and when it began: one way in is
    /// finished before another is started.
    pub(super) way_in: Option<(Vec<[i32; 3]>, u64)>,
    /// Doors the golem opened after laying them, shut on its way home: a door
    /// swung shut behind it walls work in the way a wall does.
    pub(super) opened: Vec<[i32; 3]>,
    /// A door just laid, to be swung open before the next plan.
    pub(super) pending_use: Option<[i32; 3]>,
}

/// The golem's own scaffolding.
#[derive(Default)]
pub struct ScaffoldState {
    /// The block the next scaffold is made of, and the cells the project's
    /// scaffolding stands in: both re-read every tick.
    pub(super) block: Option<BlockRecord>,
    pub(super) cells: HashSet<[i32; 3]>,
    /// Scaffold blocks a climb asked for and the hands did not hold.
    pub(super) want: u32,
    pub(super) short: bool,
    /// Scaffolds to take down before anything else.
    pub(super) urgent: Vec<[i32; 3]>,
    /// Scaffolding found out of reach, and how often: a pillar the golem
    /// climbed out of a pit on cannot be walked back to.
    pub(super) shunned: HashMap<[i32; 3], u8>,
    /// Scaffolding given up on and left where it stands, for the report.
    pub(super) left_standing: u32,
}

/// How the work is getting on, and where in the build it is.
#[derive(Default)]
pub struct Pace {
    /// The tick a block last landed or was cleared.
    pub(super) progress_at: u64,
    /// The last tick that ended with the golem doing something.
    pub(super) busy_at: u64,
    /// When the golem last set a block down.
    pub(super) placed_at: u64,
    /// The highest layer work is taken from (none until work was first
    /// weighed), and the tick work within it last landed.
    pub(super) band_top: Option<i32>,
    pub(super) band_progress_at: u64,
    /// The corner of the build the golem is working through: work near it goes
    /// first, so a wall is finished before another is started.
    pub(super) focus: Option<[i32; 3]>,
    /// First unit index that might still be open.
    pub(super) cursor: usize,
    /// When home was last checked for still being ground to stand on.
    pub(super) home_checked: u64,
}

/// Windows are glazed last; what is glazed sooner, and when all of it is.
#[derive(Default)]
pub struct GlazingState {
    /// Window panes glazed ahead of the rest: a wall laid first would close
    /// them in.
    pub(super) ahead: HashSet<usize>,
    /// The open panels a body could pass through the gap of as the world
    /// stands (standing room on both sides), and when that was last asked.
    pub(super) ways: Stamped<HashSet<usize>>,
    /// Whether the glazing is going in now: nothing else landed for so long
    /// that holding it back only leaves the golem standing.
    pub(super) under_way: bool,
}

/// What the golem knows of its hands and the chests.
#[derive(Default)]
pub struct CargoState {
    /// What the hands and the chests held when last asked, and when: reading
    /// every chest is dear, and what they hold changes slowly.
    pub(super) in_reach: Stamped<Option<BTreeMap<ItemKey, u32>>>,
    /// When the golem last set out for tools the digging ahead wants, and
    /// which.
    pub(super) tool_trip: Stamped<BTreeSet<String>>,
    /// On the way home the chests had no room for what it carries.
    pub(super) chests_full: bool,
}

/// A golem with no way home, or one whose walks keep failing.
#[derive(Default)]
pub struct Rescue {
    /// Getting out from somewhere no route leads home from.
    pub(super) stuck: Option<rescue::Stuck>,
    /// Since when the whole site has stood loaded while the way home read shut.
    pub(super) site_loaded_since: Option<u64>,
    /// Since when the walking golem has stood off every way to its goal.
    pub(super) off_route_since: Option<u64>,
    /// The cell walks last failed from, and how many failed there in a row.
    pub(super) stalled: ([i32; 3], u8),
    pub(super) burrow_from: [i32; 3],
}

/// A value and the tick it was read at (0: never).
#[derive(Default)]
pub struct Stamped<T> {
    pub(super) at: u64,
    pub(super) value: T,
}

impl<T> Stamped<T> {
    /// Whether the value was never read, or was read `every` ticks ago or more.
    pub(super) fn stale(&self, now: u64, every: u64) -> bool {
        now >= self.at + every || self.at == 0
    }
}

impl Crew {
    /// Blocks changed in or around the site.
    pub fn site_changed(&mut self) {
        self.deferrals.site_changed();
    }

    /// Note why a plan produced no step.
    pub(super) fn why(&mut self, reason: Waiting) {
        if self.why != reason {
            trace!("TRACE plan waits: {}", reason.label());
        }
        self.why = reason;
    }

    /// The live golem bound to `project`, looked up by its tag.
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
        // A golem found after a reload still wears its last crew's steering
        // tags, and this crew only writes changes: a stale hold would pin it in
        // place.
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
