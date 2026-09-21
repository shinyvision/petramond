//! Ghosts are presentation: they follow the live, anchored projects and are
//! set again after every restart.

use std::collections::BTreeMap;

use mod_sdk::*;

use crate::content::Content;
use crate::fx::HashSet;
use crate::jobs::admission::blueprint_tabled;
use crate::project::{tag_of, Brief, Phase, ProjectId, Projects};

/// How often a ghost's blueprint is looked for.
const BLUEPRINT_CHECK: Cadence = Cadence::every(10);

/// A ghost as set: placement, and whether it yields to positioning.
type Placed = (SchematicId, [i32; 3], u8, bool);

#[derive(Default)]
pub struct Ghosts {
    shown: BTreeMap<ProjectId, Placed>,
    /// The world has been told once this session: it keeps no ghosts over a
    /// restart.
    restored: bool,
    /// Whether each live project was last found planned.
    planned: BTreeMap<ProjectId, bool>,
}

impl Ghosts {
    pub fn sync(
        &mut self,
        content: &Content,
        projects: &mut Projects,
        live: &[ProjectId],
        now: u64,
    ) {
        // Whose blueprint is in a player's hand, asked once however many
        // drafts want to know.
        let mut in_hand: Option<HashSet<ProjectId>> = None;
        let mut wanted = BTreeMap::new();
        for &id in live {
            let Some(project) = projects.get(id).map(|p| p.brief()) else {
                continue;
            };
            if !project.show_ghost && project.phase != Phase::Draft {
                continue;
            }
            // A ghost is a plan: with the table broken, or a draft's blueprint
            // lying on the ground or put away, nothing is planned there.
            if BLUEPRINT_CHECK.due(now, id) || !self.planned.contains_key(&id) {
                if let Some(planned) = planned(content, projects, &project, &mut in_hand) {
                    self.planned.insert(id, planned);
                }
            }
            if self.planned.get(&id) != Some(&true) {
                continue;
            }
            if let Some((asset, origin, turns)) = project.anchored {
                wanted.insert(id, (asset, origin, turns, !project.show_ghost));
            }
        }
        // `live` comes sorted from its index.
        self.planned.retain(|id, _| live.binary_search(id).is_ok());
        if self.restored && wanted == self.shown {
            return;
        }
        for id in self.shown.keys().filter(|id| !wanted.contains_key(id)) {
            schematic_ghost_set(&tag_of(*id), None);
        }
        for (id, placed) in &wanted {
            if self.restored && self.shown.get(id) == Some(placed) {
                continue;
            }
            let (asset, origin, turns, yields_to_positioning) = *placed;
            schematic_ghost_set(
                &tag_of(*id),
                Some(SchematicGhostData {
                    asset,
                    origin,
                    turns,
                    viewers: Vec::new(),
                    yields_to_positioning,
                }),
            );
        }
        self.shown = wanted;
        self.restored = true;
    }
}

/// Whether something is planned where `project`'s ghost stands: a draft whose
/// blueprint is in its table or in a player's hand, or a started job whose
/// table still stands (its golem carries the blueprint). `None` while the
/// table's ground is not loaded (unknown, not gone).
fn planned(
    content: &Content,
    projects: &Projects,
    project: &Brief,
    in_hand: &mut Option<HashSet<ProjectId>>,
) -> Option<bool> {
    let block = get_block(project.table)?;
    if project.phase != Phase::Draft {
        return Some(block == content.table);
    }
    if blueprint_tabled(content, projects, project.id, project.table) {
        return Some(true);
    }
    let in_hand = in_hand.get_or_insert_with(|| {
        players()
            .into_iter()
            .filter_map(|row| projects.bound(&player_held(row.id)?))
            .collect()
    });
    Some(in_hand.contains(&project.id))
}
