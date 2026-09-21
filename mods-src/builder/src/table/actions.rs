//! The panel's buttons, and what the schematic picker and the positioning
//! tool report back.

use mod_sdk::*;

use crate::content::{MATERIALS_KIND, TABLE_KIND};
use crate::jobs::{Builder, Refusal};
use crate::project::{id_of_tag, tag_of, Phase, Project};
use crate::table::blueprint::bind_blueprint;
use crate::table::{blueprint_at, may_edit, project_at, publish_to, REFUSAL_TICKS};

/// A button of the table's or the materials page's document, by widget id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Materials,
    Back,
    Choose,
    Position,
    Start,
    Pause,
    Resume,
    Cancel,
    Ghost,
}

impl Action {
    fn parse(widget: &str) -> Option<Self> {
        Some(match widget {
            "materials" => Action::Materials,
            "back" => Action::Back,
            "choose" => Action::Choose,
            "position" => Action::Position,
            "start" => Action::Start,
            "pause" => Action::Pause,
            "resume" => Action::Resume,
            "cancel" => Action::Cancel,
            "ghost" => Action::Ghost,
            _ => return None,
        })
    }
}

pub fn click(builder: &mut Builder, kind: &str, widget: &str, at: Option<ContainerAddress>) {
    let Some(ContainerAddress::Block(pos)) = at else {
        return;
    };
    if kind != TABLE_KIND && kind != MATERIALS_KIND {
        return;
    }
    if get_block(pos) != Some(builder.content.table) {
        return;
    }
    let (Some(player), Some(action)) = (player_state().id, Action::parse(widget)) else {
        return;
    };
    match action {
        Action::Materials => {
            gui_open(MATERIALS_KIND, Some(pos));
            return;
        }
        Action::Back => {
            gui_open(TABLE_KIND, Some(pos));
            return;
        }
        _ => {}
    }
    let bound = project_at(builder, pos).and_then(|id| builder.projects.get(id).cloned());
    if bound.as_ref().is_some_and(|p| !may_edit(player, p)) {
        return;
    }
    let now = current_tick();
    match press(builder, player, pos, action, bound) {
        Ok(()) => builder.tables.refused.remove(&(player, pos)),
        Err(refusal) => builder
            .tables
            .refused
            .insert((player, pos), (refusal.to_string(), now + REFUSAL_TICKS)),
    };
    let viewer = GuiViewerData {
        player_id: player,
        kind: kind.to_owned(),
        anchor: at,
    };
    publish_to(builder, &viewer, pos, now);
}

fn press(
    builder: &mut Builder,
    player: PlayerId,
    pos: [i32; 3],
    action: Action,
    bound: Option<Project>,
) -> Result<(), Refusal> {
    match (action, bound) {
        (Action::Choose, project) => {
            let id = match project {
                Some(p) if p.phase() == Phase::Draft => {
                    builder.projects.update(p.id, |p| p.table = pos);
                    p.id
                }
                Some(p) if !p.phase().finished() || p.started() => return Ok(()),
                _ => {
                    let (Some(who), Some(blueprint)) = (player_identity(player), blueprint_at(pos))
                    else {
                        return Ok(());
                    };
                    let id = builder.projects.create(who.name, pos);
                    bind_blueprint(builder, pos, blueprint, id);
                    id
                }
            };
            schematic_choose(player, &tag_of(id));
        }
        (Action::Position, Some(p)) if p.phase() == Phase::Draft => {
            if let Some(asset) = p.asset {
                schematic_position(player, &p.tag(), asset, p.origin, p.turns);
            }
        }
        (Action::Start, Some(p)) => {
            if p.resumable() {
                builder.projects.update(p.id, |p| p.table = pos);
            }
            return builder.start(p.id);
        }
        (Action::Pause, Some(p)) => builder.pause(p.id),
        (Action::Ghost, Some(p)) => {
            builder
                .projects
                .update(p.id, |p| p.show_ghost = !p.show_ghost);
        }
        (Action::Resume, Some(p)) => return builder.resume(p.id, pos),
        (Action::Cancel, Some(p)) => builder.cancel(p.id),
        _ => {}
    }
    Ok(())
}

pub fn chosen(builder: &mut Builder, player: PlayerId, tag: &str, asset: SchematicId) {
    let Some(project) = id_of_tag(tag).and_then(|id| builder.projects.get(id)) else {
        return;
    };
    if project.phase() != Phase::Draft || !may_edit(player, project) {
        return;
    }
    let id = project.id;
    let title = match schematic_info(asset) {
        SchematicLookup::Ready(info) => info.title,
        _ => String::new(),
    };
    builder.projects.update(id, |p| {
        if p.asset != Some(asset) {
            p.origin = None;
            p.turns = 0;
        }
        p.asset = Some(asset);
        p.title = title;
    });
    crate::table::label_blueprint(builder, id);
    schematic_position(player, tag, asset, None, 0);
}

pub fn positioned(
    builder: &mut Builder,
    player: PlayerId,
    tag: &str,
    asset: SchematicId,
    origin: [i32; 3],
    turns: u8,
) {
    let Some(project) = id_of_tag(tag).and_then(|id| builder.projects.get(id)) else {
        return;
    };
    if project.phase() != Phase::Draft || project.asset != Some(asset) || !may_edit(player, project)
    {
        return;
    }
    let id = project.id;
    builder.projects.update(id, |p| {
        p.origin = Some(origin);
        p.turns = turns % 4;
    });
}

/// Using a bound blueprint repositions its draft's ghost from anywhere, for
/// building by hand without a table.
pub fn use_blueprint(builder: &mut Builder) -> Outcome {
    let Some(player) = player_state().id else {
        return Outcome::Continue;
    };
    let Some(held) = player_held(player) else {
        return Outcome::Continue;
    };
    let Some(project) = builder
        .projects
        .bound(&held)
        .and_then(|id| builder.projects.get(id))
    else {
        return Outcome::Continue;
    };
    let (Phase::Draft, Some(asset)) = (project.phase(), project.asset) else {
        return Outcome::Continue;
    };
    if may_edit(player, project) {
        schematic_position(player, &project.tag(), asset, project.origin, project.turns);
    }
    Outcome::Cancel
}
