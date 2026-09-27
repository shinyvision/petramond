use super::{MobSoundEvent, SoundEvent, SpatialSoundCommand};
use petramond::net::protocol::SelfEvents;
use petramond::player::one_shot::OneShot;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::inventory::Hand;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WorldEvent {
    BlockBroken {
        pos: IVec3,
        block: Block,
        normal: Option<IVec3>,
        tint: Option<[u8; 3]>,
    },
    BlockPlaced {
        pos: IVec3,
        block: Block,
    },
    PanelToggled {
        anchor: IVec3,
        open: bool,
    },
    ChestOpened {
        pos: IVec3,
    },
    ChestClosed {
        pos: IVec3,
    },
    ItemPickedUp {
        pos: petramond_math::world_pos::WorldPos,
        by_self: bool,
    },
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
    pub placed_block: Option<Block>,
    pub placed_off_hand: bool,
    pub broke_block: Option<Block>,
    pub swung_hand: bool,
    pub threw_item: bool,
    pub picked_up_item: bool,
    pub open_gui: Option<(
        petramond_world::gui_state::GuiKind,
        Option<petramond::menu::MenuAnchor>,
    )>,
    pub close_document_gui: bool,
    pub toggled_panel: Option<bool>,
    pub bed_interacted: bool,
    pub interacted: bool,
    pub interacted_off_hand: bool,
    pub interacted_presents_itself: bool,
    pub interacted_places: bool,
    pub player_damaged: bool,
    pub player_died: bool,
    pub open_sleep: bool,
    pub sleep_ended: bool,
    pub respawned: bool,
    pub sounds: Vec<SoundEvent>,
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    pub mob_sounds: Vec<MobSoundEvent>,
    pub world_events: Vec<WorldEvent>,
    pub animator_events: Vec<(petramond::player::RigId, u16)>,
    pub connection_lost: Option<String>,
    pub presented_world_replaced: bool,
    pub presented_time_jumped: bool,
}

impl GameEvents {
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

#[derive(Default)]
pub struct ClientEvents {
    pub world: Vec<WorldEvent>,
    pub self_events: SelfEvents,
    pub sounds: Vec<SoundEvent>,
    pub spatial_sounds: Vec<SpatialSoundCommand>,
    pub mob_sounds: Vec<MobSoundEvent>,
}
