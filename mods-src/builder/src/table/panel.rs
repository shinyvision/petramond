//! The table panel: every key its document binds, and what they say for the
//! project the table speaks for.

use mod_sdk::*;

use crate::jobs::Builder;
use crate::project::{Hold, Phase, Project};
use crate::table::{blueprint_at, may_edit, project_at};

/// The theme palette a status line is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Muted,
    Plain,
    Accent,
    Warn,
    Danger,
}

impl Tone {
    pub fn palette(self) -> &'static str {
        match self {
            Tone::Muted => "text_muted",
            Tone::Plain => "text",
            Tone::Accent => "accent",
            Tone::Warn => "warn",
            Tone::Danger => "danger",
        }
    }
}

/// A table with no project shows nothing but its words: the default.
#[derive(Default)]
pub struct TablePanel {
    pub title: String,
    pub status: String,
    pub tone: Tone,
    can_choose: bool,
    can_position: bool,
    has_design: bool,
    can_ghost: bool,
    ghost_shown: bool,
    show_start: bool,
    can_start: bool,
    show_pause: bool,
    show_resume: bool,
    can_resume: bool,
    show_cancel: bool,
}

impl PanelState for TablePanel {
    fn values(&self) -> Vec<(&'static str, GuiValue)> {
        vec![
            ("builder:title", gui_text(&self.title)),
            ("builder:status", gui_text(&self.status)),
            ("builder:status_palette", gui_text(self.tone.palette())),
            ("builder:can_choose", gui_flag(self.can_choose)),
            ("builder:can_position", gui_flag(self.can_position)),
            ("builder:has_design", gui_flag(self.has_design)),
            ("builder:can_ghost", gui_flag(self.can_ghost)),
            ("builder:ghost_frame", gui_flag(self.ghost_shown)),
            ("builder:show_start", gui_flag(self.show_start)),
            ("builder:can_start", gui_flag(self.can_start)),
            ("builder:show_pause", gui_flag(self.show_pause)),
            ("builder:show_resume", gui_flag(self.show_resume)),
            ("builder:can_resume", gui_flag(self.can_resume)),
            ("builder:show_cancel", gui_flag(self.show_cancel)),
        ]
    }
}

/// What `player` sees on the table at `anchor`.
pub fn describe(builder: &mut Builder, player: PlayerId, anchor: [i32; 3], now: u64) -> TablePanel {
    let blueprint = blueprint_at(anchor);
    let project = project_at(builder, anchor).and_then(|id| builder.projects.get(id).cloned());
    let mut panel = match &project {
        Some(project) => of_project(builder, player, anchor, blueprint.as_ref(), project, now),
        None => TablePanel {
            title: if blueprint.is_some() {
                "Blank blueprint"
            } else {
                "No blueprint"
            }
            .into(),
            status: if blueprint.is_some() {
                "Choose a schematic to build"
            } else {
                "Insert a blueprint"
            }
            .into(),
            can_choose: blueprint.is_some(),
            ghost_shown: true,
            ..TablePanel::default()
        },
    };
    // A refused press keeps the status line for a moment: the next publish
    // would otherwise wipe it before it could be read.
    match builder.tables.refused.get(&(player, anchor)) {
        Some((reason, until)) if now < *until => {
            panel.status = reason.clone();
            panel.tone = Tone::Danger;
        }
        _ => {}
    }
    panel
}

fn of_project(
    builder: &mut Builder,
    player: PlayerId,
    anchor: [i32; 3],
    blueprint: Option<&ItemStackData>,
    project: &Project,
    now: u64,
) -> TablePanel {
    let editor = may_edit(player, project);
    let brief = project.brief();
    let active = brief.phase.active();
    let lost = brief.lost_worker();
    // A started job that ended is taken up again from the table its
    // blueprint now lies in.
    let tabled = blueprint.is_some_and(|b| builder.projects.bound(b) == Some(project.id));
    let resumable = brief.resumable() && tabled;
    let show_start = brief.phase == Phase::Draft || lost || resumable;
    // Asked only where the answer shows: on Start, and on a settled draft's
    // status line.
    let admission = (show_start && brief.anchored.is_some()).then(|| {
        let from = if resumable { brief.at(anchor) } else { brief };
        builder.admission(&from, now)
    });
    let (status, tone) = status(builder, project, admission.as_ref());
    // A blueprint that has been started once is that build for good; one
    // whose project ended unstarted is blank paper again.
    let draft = brief.open_to_change() || (brief.phase.finished() && !brief.started);
    TablePanel {
        title: if project.title.is_empty() {
            "Blueprint".into()
        } else {
            project.title.clone()
        },
        status,
        tone,
        can_choose: blueprint.is_some() && editor && draft,
        can_position: editor && brief.phase == Phase::Draft && brief.asset.is_some(),
        has_design: brief.anchored.is_some(),
        can_ghost: editor,
        ghost_shown: brief.show_ghost,
        show_start,
        can_start: show_start && editor && admission == Some(Ok(())),
        show_pause: active && brief.hold.is_none() && !brief.cancelling,
        show_resume: active && brief.hold.is_some() && !lost,
        can_resume: editor,
        show_cancel: editor
            && !brief.cancelling
            && (active || brief.anchored.is_some() && brief.phase == Phase::Draft),
    }
}

/// The status line. Why work stands still is the golem's to say: it wears a
/// mark and answers when used. The table speaks only where no golem can.
fn status(
    builder: &Builder,
    project: &Project,
    admission: Option<&Result<(), crate::jobs::Refusal>>,
) -> (String, Tone) {
    let job = builder.jobs.map.get(&project.id);
    let percent = (job.map_or(0.0, |j| j.done()) * 100.0).floor();
    let away = job.is_some_and(|j| j.crew.mob.is_none());
    let working = matches!(project.phase(), Phase::Working | Phase::Returning);
    match (project.phase(), project.hold()) {
        (Phase::Draft, _) if project.asset.is_none() => {
            ("Choose a schematic to build".into(), Tone::Muted)
        }
        (Phase::Draft, _) if project.origin.is_none() => ("Position the ghost".into(), Tone::Muted),
        (Phase::Draft, _) => match admission {
            Some(Err(refusal)) => (refusal.to_string(), Tone::Warn),
            _ => ("Ready to build".into(), Tone::Accent),
        },
        (Phase::Emerging, _) => ("The golem is digging itself out".into(), Tone::Plain),
        (_, Some(Hold::Player)) if working => ("Paused".into(), Tone::Muted),
        (_, Some(Hold::Worker | Hold::Table)) if working => (project.note.clone(), Tone::Danger),
        _ if working && away => ("The golem is outside the loaded world".into(), Tone::Warn),
        (_, Some(_)) if working => (format!("Waiting: {percent}% done"), Tone::Warn),
        (Phase::Working, _) => (format!("Building: {percent}% done"), Tone::Plain),
        (Phase::Returning, _) => ("Returning materials".into(), Tone::Plain),
        (Phase::Burrowing, _) => ("The golem is burrowing home".into(), Tone::Plain),
        (Phase::Complete, _) => {
            let manual = job.and_then(|j| j.summary()).map_or(0, |s| s.unsupported);
            let mut notes = Vec::new();
            if manual > 0 {
                notes.push(format!("{manual} blocks need building by hand"));
            }
            if !project.note.is_empty() {
                notes.push(project.note.clone());
            }
            if notes.is_empty() {
                ("Complete".into(), Tone::Accent)
            } else {
                (format!("Complete; {}", notes.join("; ")), Tone::Warn)
            }
        }
        (Phase::Cancelled, _) => ("Cancelled".into(), Tone::Muted),
    }
}
