//! Engine ↔ ABI type conversions (`crate::events` types to `mod_api` mirrors).
//!
//! One total function per direction actually used: engine payloads
//! flow OUT to guests; only `Outcome` and the taxonomy's mutable fields flow
//! back (handled at the wiring site, not here). Every match is exhaustive on
//! purpose — adding an engine event/stage without its ABI mirror must not
//! compile.

use crate::chunk::SectionPos;
use crate::events::{
    self, BlockBreakPre, BlockInteract, BlockPlacePre, ItemUsePre, MobDamageFeedbackComponent,
    MobDamagePre, MobDamageSound, MobInteract, PlayerDamagePre, PostEvent, PostEventKind,
};
use crate::facing::Facing;
use crate::mathh::{IVec3, Vec3};
use mod_api as api;

/// Engine → ABI world-cell position (a plain-fn `IVec3::to_array` for `.map`).
#[inline]
fn ivec(v: IVec3) -> [i32; 3] {
    v.to_array()
}

/// Engine → ABI vector (a plain-fn `Vec3::to_array` for `.map`).
#[inline]
fn vec(v: Vec3) -> [f32; 3] {
    v.to_array()
}

#[inline]
fn section(p: SectionPos) -> [i32; 3] {
    [p.cx, p.cy, p.cz]
}

pub(super) fn outcome(o: api::Outcome) -> events::Outcome {
    match o {
        api::Outcome::Continue => events::Outcome::Continue,
        api::Outcome::Cancel => events::Outcome::Cancel,
    }
}

pub(super) fn attach(stage: api::Stage, side: api::AttachSide) -> events::Attach {
    let stage = match stage {
        api::Stage::Mining => events::Stage::Mining,
        api::Stage::Placement => events::Stage::Placement,
        api::Stage::Attack => events::Stage::Attack,
        api::Stage::Drops => events::Stage::Drops,
        api::Stage::Menu => events::Stage::Menu,
        api::Stage::PlayerDamage => events::Stage::PlayerDamage,
        api::Stage::WorldScheduled => events::Stage::WorldScheduled,
        api::Stage::NaturalBreaks => events::Stage::NaturalBreaks,
        api::Stage::Pickup => events::Stage::Pickup,
        api::Stage::Mobs => events::Stage::Mobs,
        api::Stage::ItemPhysics => events::Stage::ItemPhysics,
        api::Stage::Spawning => events::Stage::Spawning,
    };
    match side {
        api::AttachSide::Before => events::Attach::Before(stage),
        api::AttachSide::After => events::Attach::After(stage),
    }
}

/// The engine queue key for an ABI post-event kind; `None` for pre kinds.
pub(super) fn post_kind(kind: api::EventKind) -> Option<PostEventKind> {
    use api::EventKind as K;
    Some(match kind {
        K::BlockPlacePre
        | K::BlockBreakPre
        | K::BlockInteract
        | K::ItemUsePre
        | K::MobInteract
        | K::MobDamagePre
        | K::PlayerDamagePre => return None,
        K::BlockPlaced => PostEventKind::BlockPlaced,
        K::BlockBroken => PostEventKind::BlockBroken,
        K::ItemUsed => PostEventKind::ItemUsed,
        K::MobDied => PostEventKind::MobDied,
        K::MobSpawned => PostEventKind::MobSpawned,
        K::PlayerDamaged => PostEventKind::PlayerDamaged,
        K::PlayerDied => PostEventKind::PlayerDied,
        K::ContainerOpened => PostEventKind::ContainerOpened,
        K::ContainerClosed => PostEventKind::ContainerClosed,
        K::SectionGenerated => PostEventKind::SectionGenerated,
        K::SectionLoaded => PostEventKind::SectionLoaded,
        K::PlayerDismounted => PostEventKind::PlayerDismounted,
    })
}

fn facing(f: Facing) -> api::Facing {
    match f {
        Facing::North => api::Facing::North,
        Facing::South => api::Facing::South,
        Facing::West => api::Facing::West,
        Facing::East => api::Facing::East,
    }
}

/// Engine container sessions speak `GuiKind` end-to-end; the ABI mirror keeps
/// its named engine variants (frozen wire shape), so the engine kinds map to
/// them here and every other registered kind rides `Mod { key }`.
fn container(kind: crate::gui::GuiKind) -> api::ContainerKind {
    use crate::gui::GuiKind;
    match kind {
        GuiKind::Inventory => api::ContainerKind::Inventory,
        GuiKind::CraftingTable => api::ContainerKind::CraftingTable,
        GuiKind::Furnace => api::ContainerKind::Furnace,
        GuiKind::Chest => api::ContainerKind::Chest,
        GuiKind::FurnitureWorkbench => api::ContainerKind::FurnitureWorkbench,
        kind => api::ContainerKind::Mod {
            key: crate::gui::kind_key(kind).unwrap_or("?").to_owned(),
        },
    }
}

/// ABI → engine GUI state value.
pub(super) fn gui_value(v: api::GuiValue) -> crate::gui::GuiValue {
    match v {
        api::GuiValue::F32(x) => crate::gui::GuiValue::F32(x),
        api::GuiValue::I32(x) => crate::gui::GuiValue::I32(x),
        api::GuiValue::Str(s) => crate::gui::GuiValue::Str(s),
    }
}

/// Engine → ABI GUI state value.
pub(super) fn gui_value_out(v: &crate::gui::GuiValue) -> api::GuiValue {
    match v {
        crate::gui::GuiValue::F32(x) => api::GuiValue::F32(*x),
        crate::gui::GuiValue::I32(x) => api::GuiValue::I32(*x),
        crate::gui::GuiValue::Str(s) => api::GuiValue::Str(s.clone()),
    }
}

fn damage_source(s: events::DamageSource) -> api::DamageSource {
    match s {
        events::DamageSource::Fall => api::DamageSource::Fall,
        events::DamageSource::PlayerAttack(id) => api::DamageSource::PlayerAttack {
            id: api::PlayerId(id.0),
        },
        events::DamageSource::MobAttack { kind, .. } => api::DamageSource::MobAttack {
            key: crate::mob::def(kind).key.to_owned(),
        },
        events::DamageSource::Mod(mod_id) => api::DamageSource::Mod {
            mod_id: mod_id.to_owned(),
        },
    }
}

pub(super) fn block_place_pre(ev: &BlockPlacePre) -> api::EventPayload {
    api::EventPayload::BlockPlacePre {
        pos: ivec(ev.pos),
        block: api::BlockId(ev.block.id()),
        facing: facing(ev.facing),
    }
}

pub(super) fn block_break_pre(ev: &BlockBreakPre) -> api::EventPayload {
    api::EventPayload::BlockBreakPre {
        pos: ivec(ev.pos),
        block: api::BlockId(ev.block.id()),
        harvested: ev.harvested,
    }
}

pub(super) fn block_interact(ev: &BlockInteract) -> api::EventPayload {
    api::EventPayload::BlockInteract {
        pos: ivec(ev.pos),
        block: api::BlockId(ev.block.id()),
    }
}

pub(super) fn item_use_pre(ev: &ItemUsePre) -> api::EventPayload {
    api::EventPayload::ItemUsePre {
        item: api::ItemId(ev.item.id()),
        target: ev.target.map(ivec),
    }
}

pub(super) fn mob_interact(ev: &MobInteract) -> api::EventPayload {
    api::EventPayload::MobInteract {
        id: ev.id,
        key: crate::mob::def(ev.kind).key.to_owned(),
        player_id: api::PlayerId(ev.player.0),
    }
}

pub(super) fn mob_damage_pre(ev: &MobDamagePre) -> api::EventPayload {
    api::EventPayload::MobDamagePre {
        mob_id: ev.mob_id,
        kind: api::MobId(ev.kind.id()),
        amount: ev.amount,
        source: damage_source(ev.source),
        origin: ev.origin.map(vec),
        feedback: api::MobDamageFeedback {
            components: ev
                .feedback
                .components
                .iter()
                .copied()
                .map(mob_damage_feedback_component)
                .collect(),
        },
    }
}

fn mob_damage_feedback_component(
    component: MobDamageFeedbackComponent,
) -> api::MobDamageFeedbackComponent {
    match component {
        MobDamageFeedbackComponent::DecreaseHealth => {
            api::MobDamageFeedbackComponent::DecreaseHealth
        }
        MobDamageFeedbackComponent::Immunity { ticks } => {
            api::MobDamageFeedbackComponent::Immunity { ticks }
        }
        MobDamageFeedbackComponent::Flash { duration } => {
            api::MobDamageFeedbackComponent::Flash { duration }
        }
        MobDamageFeedbackComponent::Knockback { scale, duration } => {
            api::MobDamageFeedbackComponent::Knockback { scale, duration }
        }
        MobDamageFeedbackComponent::Sound { category } => api::MobDamageFeedbackComponent::Sound {
            category: match category {
                MobDamageSound::Hurt => api::MobDamageSound::Hurt,
                MobDamageSound::Death => api::MobDamageSound::Death,
            },
        },
        MobDamageFeedbackComponent::Ragdoll => api::MobDamageFeedbackComponent::Ragdoll,
    }
}

pub(super) fn player_damage_pre(ev: &PlayerDamagePre) -> api::EventPayload {
    api::EventPayload::PlayerDamagePre {
        amount: ev.amount,
        source: damage_source(ev.source),
        origin: ev.origin.map(vec),
    }
}

pub(super) fn post_event(ev: &PostEvent) -> api::EventPayload {
    match *ev {
        PostEvent::BlockPlaced { pos, block } => api::EventPayload::BlockPlaced {
            pos: ivec(pos),
            block: api::BlockId(block.id()),
        },
        PostEvent::BlockBroken {
            pos,
            block,
            harvested,
            natural,
        } => api::EventPayload::BlockBroken {
            pos: ivec(pos),
            block: api::BlockId(block.id()),
            harvested,
            natural,
        },
        PostEvent::ItemUsed { item } => api::EventPayload::ItemUsed {
            item: api::ItemId(item.id()),
        },
        PostEvent::MobDied { id, kind, pos } => api::EventPayload::MobDied {
            id,
            kind: api::MobId(kind.id()),
            pos: vec(pos),
        },
        PostEvent::MobSpawned { id, kind, pos } => api::EventPayload::MobSpawned {
            id,
            kind: api::MobId(kind.id()),
            pos: vec(pos),
        },
        PostEvent::PlayerDamaged { amount, new_health } => {
            api::EventPayload::PlayerDamaged { amount, new_health }
        }
        PostEvent::PlayerDied => api::EventPayload::PlayerDied,
        PostEvent::ContainerOpened { kind, pos } => api::EventPayload::ContainerOpened {
            kind: container(kind),
            pos: pos.map(ivec),
        },
        PostEvent::ContainerClosed { kind, pos } => api::EventPayload::ContainerClosed {
            kind: container(kind),
            pos: pos.map(ivec),
        },
        PostEvent::SectionGenerated { pos } => {
            api::EventPayload::SectionGenerated { pos: section(pos) }
        }
        PostEvent::SectionLoaded { pos } => api::EventPayload::SectionLoaded { pos: section(pos) },
        PostEvent::PlayerDismounted { player, mob_id } => api::EventPayload::PlayerDismounted {
            player_id: api::PlayerId(player.0),
            mob_id,
        },
    }
}
