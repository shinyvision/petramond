//! The golem's own panel: what it carries and what it has to say, opened by
//! using the golem. A golem with its panel open stands still.

use mod_sdk::*;

use crate::content::GOLEM_KIND;
use crate::jobs::Builder;
use crate::table::Tone;
use crate::worker::trouble::{self, Trouble};

const PUBLISH: Cadence = Cadence::every(4);

#[derive(Default)]
struct GolemPanel {
    status: String,
    tone: Tone,
}

impl PanelState for GolemPanel {
    fn values(&self) -> Vec<(&'static str, GuiValue)> {
        vec![
            ("builder:golem_status", gui_text(&self.status)),
            (
                "builder:golem_status_palette",
                gui_text(self.tone.palette()),
            ),
        ]
    }
}

fn golem_of(viewer: &GuiViewerData) -> Option<u64> {
    match viewer.anchor {
        Some(ContainerAddress::Mob(mob)) if viewer.kind == GOLEM_KIND => Some(mob),
        _ => None,
    }
}

/// Golems with their panel open this tick, and who has it open.
pub fn asked(viewers: &[GuiViewerData]) -> Vec<(u64, PlayerId)> {
    viewers
        .iter()
        .filter_map(|viewer| Some((golem_of(viewer)?, viewer.player_id)))
        .collect()
}

pub fn used(builder: &Builder, mob: u64) -> Outcome {
    if mob_info(mob).is_none_or(|info| info.kind != builder.content.golem) {
        return Outcome::Continue;
    }
    if gui_open(GOLEM_KIND, Some(ContainerAddress::Mob(mob))) {
        Outcome::Cancel
    } else {
        Outcome::Continue
    }
}

pub fn publish(builder: &mut Builder, now: u64, viewers: &[GuiViewerData]) {
    for viewer in viewers {
        let Some(mob) = golem_of(viewer) else {
            continue;
        };
        if !PUBLISH.due(now, mob) && !builder.panels.is_fresh(viewer) {
            continue;
        }
        let state = describe(builder, mob, now);
        builder.panels.publish(viewer, &state);
    }
}

fn describe(builder: &mut Builder, mob: u64, now: u64) -> GolemPanel {
    let told = builder
        .jobs
        .map
        .values()
        .find(|job| job.crew.mob == Some(mob))
        .and_then(|job| {
            let project = builder.projects.get(job.id)?;
            let trouble = trouble::of(project, &job.crew, now);
            Some((trouble::reason(project, &job.crew, trouble), trouble))
        });
    let (status, tone) = match told {
        Some((status, Some(Trouble::Stuck))) => (status, Tone::Danger),
        Some((status, Some(Trouble::Thinking))) => (status, Tone::Warn),
        Some((status, None)) => (status, Tone::Plain),
        None => ("This golem has no work".to_owned(), Tone::Muted),
    };
    GolemPanel { status, tone }
}
