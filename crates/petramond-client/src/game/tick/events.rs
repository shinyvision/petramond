//! What a frame's ticks report to the app: world-anchored events, the
//! frame's gathered [`GameEvents`], and the per-frame accumulation they are
//! assembled from.

use super::{MobSoundEvent, SoundEvent, SpatialSoundCommand};
use petramond::net::protocol::SelfEvents;
use petramond::player::one_shot::OneShot;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::inventory::Hand;

/// One world-anchored event this frame's tick batch carried, in local types —
/// the client-side twin of [`petramond::net::protocol::WorldEventMsg`]. Every
/// observer presents these (break bursts, door swings, POSITIONAL sounds);
/// the app maps each to its sound at the event's position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WorldEvent {
    BlockBroken {
        pos: IVec3,
        block: Block,
        normal: Option<IVec3>,
        /// The cell's `petramond:tint` KV at break time (the KV is wiped with
        /// the block, so the burst tint must ride the event).
        tint: Option<[u8; 3]>,
    },
    BlockPlaced {
        pos: IVec3,
        block: Block,
    },
    /// A door toggled: the LOWER cell + its NEW open state.
    DoorToggled {
        lower: IVec3,
        open: bool,
    },
    ChestOpened {
        pos: IVec3,
    },
    ChestClosed {
        pos: IVec3,
    },
    /// A player collected a drop at `pos`. `by_self` = the LOCAL player did
    /// (the app keeps its non-positional self pickup sound for that).
    ItemPickedUp {
        pos: petramond_math::world_pos::WorldPos,
        by_self: bool,
    },
    /// A one-shot particle burst (a `particle_emitters.json` burst bundle by
    /// client-local catalog id) — e.g. the water splash when something falls
    /// in. Every client spawns the burst into its own particle system.
    EmitterBurst {
        emitter: u8,
        pos: petramond_math::world_pos::WorldPos,
        intensity: f32,
        direction: Option<[f32; 3]>,
        look: crate::particle::BurstLook,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameEvents {
    // The hand one-shots below are CLIENT-PREDICTED (latched at click/finish
    // time) — the server never echoes self-initiated actions back, so each
    // fires exactly once. World-visible confirmation (sounds, bursts) comes
    // from the replicated world events instead.
    /// The place ghost predicted a block this frame, if any.
    pub placed_block: Option<Block>,
    /// The predicted place above committed from the OFF hand (the use-click
    /// ladder's second pass) — the LEFT hand animates the pop.
    pub placed_off_hand: bool,
    /// The local mining timer finished a block this frame, if any.
    pub broke_block: Option<Block>,
    /// The hand swung this frame for an attack.
    pub swung_hand: bool,
    /// An item/stack left the hand for the world this frame.
    pub threw_item: bool,
    /// At least one dropped item was collected into the inventory this frame.
    pub picked_up_item: bool,
    /// A GUI screen should open this frame — engine containers and mod GUIs
    /// alike, with the block or mob the session is anchored on (`None` for
    /// an unanchored `GuiOpen`). The inventory's E-key open is
    /// client-initiated and never rides here.
    pub open_gui: Option<(
        petramond_world::gui_state::GuiKind,
        Option<petramond::menu::MenuAnchor>,
    )>,
    /// A mod asked to close the open mod GUI this frame (`GuiClose`); the app
    /// honours it only while a mod GUI screen is actually up.
    pub close_document_gui: bool,
    /// The player right-clicked a door this frame. Carries the door's NEW open
    /// state (after the toggle applied). The open/close SOUND is driven by the
    /// positional [`WorldEvent::DoorToggled`] every observer receives; this
    /// one-shot remains for the toggler's own presentation. `None` = no door
    /// toggle this frame.
    pub toggled_door: Option<bool>,
    /// The player right-clicked a bed this frame. This fires even in daytime,
    /// when the click sets the spawn point but does not start sleep.
    pub bed_interacted: bool,
    /// The player's use click PREDICTABLY does something this frame (an
    /// interactable target, a usable/edible held item, a plausible placement)
    /// — the P0 hand jab, latched at click time, unless the consumer
    /// presents itself ([`interacted_presents_itself`](Self::interacted_presents_itself)).
    /// Covers what the removed `used_item` echo used to animate.
    pub interacted: bool,
    /// The jab above belongs to the OFF hand (the click's predicted effect —
    /// or its `used_unpredicted` echo — came from the ladder's second pass):
    /// the LEFT hand jabs instead of the right.
    pub interacted_off_hand: bool,
    /// The consumed click's consumer PRESENTS ITSELF (the eat's raise is its
    /// whole presentation), so no hand jabs. Still an interaction for
    /// everything else that reads one.
    pub interacted_presents_itself: bool,
    /// The consumed click above is a PLACEMENT (predicted to place, ghost or
    /// not): it plays the place jab, not the interact one.
    pub interacted_places: bool,
    /// The player took damage this frame (post `player_damage_pre`, amount
    /// > 0) — plays the hurt sound and kicks the screen/hand shake.
    pub player_damaged: bool,
    /// The player's health hit 0 this frame — the app opens the death screen.
    pub player_died: bool,
    /// The player right-clicked a bed this frame — the app opens the sleep
    /// overlay.
    pub open_sleep: bool,
    /// The sleep ended this frame (completed, cancelled, or died) — the app
    /// closes the sleep overlay if it is up.
    pub sleep_ended: bool,
    /// The player respawned this frame — the app closes the death screen.
    pub respawned: bool,
    /// Every sound mods emitted across this frame's fixed ticks, in emission
    /// order. NON-lossy (unlike the latched booleans above): each entry plays
    /// exactly once.
    pub sounds: Vec<SoundEvent>,
    /// Spatial sound start/stop commands emitted by mods across this frame's
    /// fixed ticks. NON-lossy; the app/audio side owns active playback state.
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    /// Semantic mob sound events emitted by gameplay across this frame's fixed
    /// ticks. NON-lossy; the app resolves species data and plays them.
    pub mob_sounds: Vec<MobSoundEvent>,
    /// World-anchored events every observer presents (positional sounds,
    /// break bursts, door swings), in emission order. NON-lossy.
    pub world_events: Vec<WorldEvent>,
    /// Graph events fired on the local player's rig animators this batch:
    /// client mods' own (a round trip early) and the server's echoes of the
    /// rest, `(rig, event id)` in order.
    pub animator_events: Vec<(petramond::player::RigId, u16)>,
    /// The server became unreachable (thread crashed / channel closed) —
    /// reported EXACTLY ONCE, on the frame the loss is detected. Until the app
    /// grows a proper "world stopped" screen for it, it
    /// is logged and the (frozen) world keeps rendering.
    pub connection_lost: Option<String>,
}

impl GameEvents {
    /// The hand gestures this batch fired, `(hand, gesture)` in the order
    /// the client-mod swing facts rank them: the swing, the break, the
    /// place (a placed block or a click predicted to place — from the hand
    /// that acted), the throw, then the interact — every other consumed use
    /// click, unless its consumer presents itself.
    pub fn one_shots(&self) -> impl Iterator<Item = (Hand, OneShot)> + '_ {
        let click_hand = if self.interacted_off_hand {
            Hand::Off
        } else {
            Hand::Main
        };
        let place_hand = if self.placed_off_hand {
            Hand::Off
        } else {
            Hand::Main
        };
        let placed = self.placed_block.is_some();
        let predicted_place = self.interacted && self.interacted_places;
        let interact =
            self.interacted && !self.interacted_places && !self.interacted_presents_itself;
        [
            (self.swung_hand, Hand::Main, OneShot::Swing),
            (self.broke_block.is_some(), Hand::Main, OneShot::Break),
            (placed, place_hand, OneShot::Place),
            (predicted_place && !placed, click_hand, OneShot::Place),
            (self.threw_item, Hand::Main, OneShot::Throw),
            (interact, click_hand, OneShot::Interact),
        ]
        .into_iter()
        .filter_map(|(fired, hand, kind)| fired.then_some((hand, kind)))
    }
}

/// The client-side accumulation of one frame's `TickUpdate` event payloads,
/// already translated to LOCAL types (ids remapped at the transport for a
/// remote client; identity in-process). Filled by `apply_tick_update`, drained
/// once per frame by [`Game::tick`] into `GameEvents`.
#[derive(Default)]
pub struct ClientEvents {
    pub world: Vec<WorldEvent>,
    pub self_events: SelfEvents,
    pub sounds: Vec<SoundEvent>,
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    pub mob_sounds: Vec<MobSoundEvent>,
}
