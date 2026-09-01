//! Engine ↔ ABI type conversions (`crate::events` types to `mod_api` mirrors).
//!
//! One total function per direction actually used: engine payloads
//! flow OUT to guests; only `Outcome` and the taxonomy's mutable fields flow
//! back (handled at the wiring site, not here). Every match is exhaustive on
//! purpose — adding an engine event/stage without its ABI mirror must not
//! compile.

use crate::events::{
    self, AttackAttempt, BlockBreakPre, BlockPlacePre, InteractAttempt, ItemUsePre,
    MobDamageFeedbackComponent, MobDamagePre, MobDamageSound, PlayerDamagePre, PostEvent,
    PostEventKind,
};
use mod_api as api;
use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::chunk::SectionPos;

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
        | K::InteractAttempt
        | K::UseUnclaimed
        | K::AttackAttempt
        | K::ItemUsePre
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
        K::MobTagAdded => PostEventKind::MobTagAdded,
        K::MobTagRemoved => PostEventKind::MobTagRemoved,
        K::ItemPickedUp => PostEventKind::ItemPickedUp,
        K::ItemObtained => PostEventKind::ItemObtained,
        K::MobDamaged => PostEventKind::MobDamaged,
        K::Interacted => PostEventKind::Interacted,
        K::ModEvent => PostEventKind::ModEvent,
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

/// Engine container sessions speak `GuiKind` end-to-end; the ABI names the
/// same kinds by their REGISTRY KEY, so engine and pack containers convert
/// through one line and no engine identity is baked into the wire enum.
fn container(kind: petramond_world::gui_state::GuiKind) -> api::ContainerKind {
    api::ContainerKind::new(petramond_world::gui_state::kind_key(kind).unwrap_or("?"))
}

/// ABI → engine GUI state value.
pub(super) fn gui_value(v: api::GuiValue) -> petramond_world::gui_state::GuiValue {
    match v {
        api::GuiValue::F32(x) => petramond_world::gui_state::GuiValue::F32(x),
        api::GuiValue::I32(x) => petramond_world::gui_state::GuiValue::I32(x),
        api::GuiValue::Str(s) => petramond_world::gui_state::GuiValue::Str(s),
    }
}

/// Engine → ABI GUI state value.
pub(super) fn gui_value_out(v: &petramond_world::gui_state::GuiValue) -> api::GuiValue {
    match v {
        petramond_world::gui_state::GuiValue::F32(x) => api::GuiValue::F32(*x),
        petramond_world::gui_state::GuiValue::I32(x) => api::GuiValue::I32(*x),
        petramond_world::gui_state::GuiValue::Str(s) => api::GuiValue::Str(s.clone()),
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
        player: api::PlayerId(ev.player.0),
        // An earlier handler's override is part of the live event — later
        // handlers in the chain must see it to leave or replace it.
        drops: ev
            .drops
            .as_ref()
            .map(|stacks| stacks.iter().map(item_stack_out).collect()),
    }
}

/// Engine stack → its ABI crossing (registry name + count + instance data).
pub(super) fn item_stack_out(stack: &petramond_world::item::ItemStack) -> api::ItemStackData {
    let data = petramond_world::item::variant::get(stack.variant)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    api::ItemStackData {
        item: petramond_world::registry::names()
            .items
            .name(stack.item.id())
            .unwrap_or("?")
            .to_owned(),
        count: stack.count,
        data,
    }
}

/// ABI stack → engine stack, LENIENT: this is runtime data from a mod (an
/// event-payload drops override), so an unknown item name drops the stack
/// with a warning and malformed instance data degrades to a plain stack —
/// never an error, the instance-data rule.
pub(super) fn item_stack_in(data: &api::ItemStackData) -> Option<petramond_world::item::ItemStack> {
    use petramond_world::item::{variant, ItemStack, ItemType};
    let Some(item) = ItemType::by_name(&data.item) else {
        log::warn!("event drops override names unknown item '{}'", data.item);
        return None;
    };
    let variant = if data.data.is_empty() {
        variant::VariantId::NONE
    } else {
        let mut map = variant::VariantMap::new();
        for (k, v) in &data.data {
            map.insert(k.clone(), v.clone());
        }
        if variant::valid(&map) {
            variant::intern(&map).unwrap_or(variant::VariantId::NONE)
        } else {
            log::warn!(
                "event drops override for '{}': invalid instance data",
                data.item
            );
            variant::VariantId::NONE
        }
    };
    Some(ItemStack::with_variant(item, data.count, variant))
}

pub(super) fn interact_attempt(ev: &InteractAttempt) -> api::EventPayload {
    api::EventPayload::InteractAttempt {
        block: ev.block.map(ivec),
        face: ev.face.map(ivec),
        mob: ev.mob,
        player: api::PlayerId(ev.player.0),
    }
}

pub(super) fn use_unclaimed(ev: &InteractAttempt) -> api::EventPayload {
    api::EventPayload::UseUnclaimed {
        block: ev.block.map(ivec),
        face: ev.face.map(ivec),
        mob: ev.mob,
        player: api::PlayerId(ev.player.0),
    }
}

pub(super) fn attack_attempt(ev: &AttackAttempt) -> api::EventPayload {
    api::EventPayload::AttackAttempt {
        block: ev.block.map(ivec),
        face: ev.face.map(ivec),
        mob: ev.mob,
        target: ev.target.map(|p| api::PlayerId(p.0)),
        player: api::PlayerId(ev.player.0),
    }
}

pub(super) fn item_use_pre(ev: &ItemUsePre) -> api::EventPayload {
    api::EventPayload::ItemUsePre {
        item: api::ItemId(ev.item.id()),
        target: ev.target.map(ivec),
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
        PostEvent::ItemUsed { player, item, kind } => api::EventPayload::ItemUsed {
            player: api::PlayerId(player.0),
            item: api::ItemId(item.id()),
            kind: match kind {
                crate::events::ItemUseEvent::Eaten => api::ItemUseEvent::Eaten,
                crate::events::ItemUseEvent::Handler => api::ItemUseEvent::Handler,
                crate::events::ItemUseEvent::Claimed => api::ItemUseEvent::Claimed,
            },
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
        PostEvent::PlayerDismounted { player, mount } => api::EventPayload::PlayerDismounted {
            player_id: api::PlayerId(player.0),
            mount: match mount.target {
                crate::mob::riding::MountTarget::Mob(id) => api::MountTarget::Mob(id),
                crate::mob::riding::MountTarget::Anchor(a) => {
                    api::MountTarget::Anchor(a.pos.to_array())
                }
            },
        },
        PostEvent::MobTagAdded {
            id,
            kind,
            ref key,
            ref value,
        } => api::EventPayload::MobTagAdded {
            mob_id: id,
            kind: api::MobId(kind.id()),
            key: key.clone(),
            value: super::host::tags::to_api(value),
        },
        PostEvent::MobTagRemoved {
            id,
            kind,
            ref key,
            ref value,
        } => api::EventPayload::MobTagRemoved {
            mob_id: id,
            kind: api::MobId(kind.id()),
            key: key.clone(),
            value: super::host::tags::to_api(value),
        },
        PostEvent::ItemPickedUp {
            player,
            item,
            count,
            pos,
        } => api::EventPayload::ItemPickedUp {
            player: api::PlayerId(player.0),
            item: api::ItemId(item.id()),
            count,
            pos: vec(pos),
        },
        PostEvent::ItemObtained { player, item } => api::EventPayload::ItemObtained {
            player: api::PlayerId(player.0),
            item: api::ItemId(item.id()),
        },
        PostEvent::MobDamaged {
            mob_id,
            kind,
            amount,
            source,
            killed,
        } => api::EventPayload::MobDamaged {
            mob_id,
            kind: api::MobId(kind.id()),
            amount,
            source: damage_source(source),
            killed,
        },
        PostEvent::Interacted {
            block,
            face,
            mob,
            player,
            consumed,
        } => api::EventPayload::Interacted {
            block: block.map(ivec),
            face: face.map(ivec),
            mob,
            player: api::PlayerId(player.0),
            consumed,
        },
        PostEvent::ModEvent { ref key, ref data } => api::EventPayload::ModEvent {
            key: key.clone(),
            data: data.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drops override is runtime data from a mod, so its ingestion is
    /// LENIENT like every instance-data read: an unknown item name drops
    /// that stack (never errors the break), malformed instance data degrades
    /// to a plain stack, and well-formed data survives the round trip.
    #[test]
    fn a_drops_override_ingests_leniently_and_round_trips() {
        let unknown = api::ItemStackData {
            item: "nomod:nothing".into(),
            count: 1,
            data: Vec::new(),
        };
        assert!(item_stack_in(&unknown).is_none());

        let bare_key = api::ItemStackData {
            item: "petramond:coal".into(),
            count: 2,
            data: vec![("barekey".into(), vec![1])],
        };
        let stack = item_stack_in(&bare_key).expect("known item ingests");
        assert_eq!(
            stack.variant,
            petramond_world::item::variant::VariantId::NONE,
            "malformed instance data degrades to a plain stack"
        );

        let good = api::ItemStackData {
            item: "petramond:coal".into(),
            count: 2,
            data: vec![("m:k".into(), vec![7])],
        };
        let stack = item_stack_in(&good).expect("known item ingests");
        let back = item_stack_out(&stack);
        assert_eq!(back, good, "well-formed data survives the round trip");
    }
}
