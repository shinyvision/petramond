use crate::menu::ContainerMenu;
use crate::net::protocol::TargetRef;
use crate::player::Player;
use crate::server::bed::SleepState;
use crate::server::drops::DropQueue;
use crate::server::item_use::EatingState;
use petramond_math::math::IVec3;
use petramond_world::item::ItemType;
use petramond_world::mining::MiningState;

mod latches;

pub use crate::player::PlayerId;
pub use latches::{
    AttackClick, InputLatches, PendingBreakFinished, PendingMenuAction, PendingUseClick,
    BREAK_QUEUE_DEPTH, MENU_ACTIONS_PER_TICK, MENU_QUEUE_DEPTH, MOD_EVENT_BURST,
};

pub use crate::world::placement_types::HeldRotation;

#[derive(Clone, Debug)]
pub struct FallTracker {
    peak_y: f64,
    airborne: bool,
}

impl FallTracker {
    pub fn new(y: f64) -> Self {
        Self {
            peak_y: y,
            airborne: false,
        }
    }

    pub fn reset(&mut self, y: f64) {
        self.peak_y = y;
        self.airborne = false;
    }

    pub fn observe(&mut self, y: f64, on_ground: bool, in_water: bool) -> Option<FallOutcome> {
        if in_water {
            let was_airborne = self.airborne;
            let dist = (self.peak_y - y) as f32;
            self.peak_y = y;
            self.airborne = !on_ground;
            return (was_airborne && dist > 0.0).then_some(FallOutcome::Splashed(dist));
        }
        if on_ground {
            let landed = self.airborne;
            let dist = (self.peak_y - y) as f32;
            self.peak_y = y;
            self.airborne = false;
            return (landed && dist > 0.0).then_some(FallOutcome::Landed(dist));
        }
        self.peak_y = self.peak_y.max(y);
        self.airborne = true;
        None
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FallOutcome {
    Landed(f32),
    Splashed(f32),
}

pub struct ConnectedPlayer {
    pub(in crate::server) id: PlayerId,
    pub(in crate::server) key: crate::net::identity::PlayerKey,
    pub(in crate::server) name: String,
    pub(in crate::server) player: Player,
    pub(in crate::server) sim: SessionSim,
    pub(in crate::server) input: InputLatches,
    pub(in crate::server) replication: SessionReplication,
    pub(in crate::server) transport: SessionTransport,
}

pub struct SessionSim {
    pub mining: MiningState,
    pub last_instant_break: Option<u64>,
    pub creative: super::creative::CreativeSession,
    pub edits: super::creative::EditHistory,
    pub schematic: super::schematics::SchematicSession,
    pub attack_cooldown: u32,
    pub fall: FallTracker,
    pub pending_fall: f32,
    pub pending_splash: f32,
    pub eating: Option<EatingState>,
    pub drop_queue: DropQueue,
    pub mount: Option<crate::mob::riding::Mount>,
    pub menu: ContainerMenu,
    pub sleep: Option<SleepState>,
    pub gui_state: std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
}

pub struct SessionReplication {
    pub pending_corrective_cells: Vec<IVec3>,
    pub presented_places: Vec<IVec3>,
    pub presented_breaks: Vec<IVec3>,
    pub pending_action_outcomes: Vec<crate::net::protocol::ActionOutcome>,
    pub swing_events: mod_api::HandSwing,
    pub request_open_gui: Option<(
        petramond_world::gui_state::GuiKind,
        Option<crate::menu::MenuAnchor>,
    )>,
    pub request_close_gui: bool,
    pub request_open_sleep: bool,
    pub pos_before_ticks: petramond_math::world_pos::WorldPos,
    pub tick_teleported: bool,
    pub last_sent_inventory_revision: Option<u64>,
    pub last_obtained_scan: Option<u64>,
    pub sent_unlock_count: usize,
    pub sent_disabled_mods: usize,
    pub last_menu_sync: Option<crate::net::protocol::MenuSyncMsg>,
    pub last_sent_gui_state: Option<std::sync::Arc<petramond_world::gui_state::GuiStateMap>>,
    pub last_reported_transform: Option<crate::net::protocol::SelfTransform>,
}

pub struct SessionTransport {
    pub terrain: crate::server::streaming::TerrainSync,
    pub interest: crate::server::game::EntityInterest,
    pub view_radius: i32,
}

impl SessionReplication {
    pub fn force_inventory_resync(&mut self) {
        self.last_sent_inventory_revision = None;
    }

    pub fn force_menu_resync(&mut self) {
        self.last_menu_sync = None;
    }

    pub fn force_gui_state_resync(&mut self) {
        self.last_sent_gui_state = None;
    }

    pub fn push_outcome(&mut self, outcome: crate::net::protocol::ActionOutcome) {
        self.pending_action_outcomes.push(outcome);
    }
}

impl ConnectedPlayer {
    pub fn new(
        id: PlayerId,
        key: crate::net::identity::PlayerKey,
        name: String,
        player: Player,
        view_radius: i32,
    ) -> Self {
        let fall = FallTracker::new(player.pos.y);
        let pos_before_ticks = player.pos;
        Self {
            id,
            key,
            name,
            sim: SessionSim {
                mining: MiningState::new(),
                last_instant_break: None,
                creative: Default::default(),
                edits: Default::default(),
                schematic: Default::default(),
                attack_cooldown: 0,
                fall,
                pending_fall: 0.0,
                pending_splash: 0.0,
                eating: None,
                drop_queue: DropQueue::default(),
                mount: None,
                menu: ContainerMenu::new(),
                sleep: None,
                gui_state: petramond_world::gui_state::empty_gui_state(),
            },
            input: InputLatches::new(pos_before_ticks),
            replication: SessionReplication {
                pending_corrective_cells: Vec::new(),
                presented_places: Vec::new(),
                presented_breaks: Vec::new(),
                pending_action_outcomes: Vec::new(),
                swing_events: Default::default(),
                request_open_gui: None,
                request_close_gui: false,
                request_open_sleep: false,
                pos_before_ticks,
                tick_teleported: false,
                last_sent_inventory_revision: None,
                last_obtained_scan: None,
                sent_unlock_count: 0,
                sent_disabled_mods: 0,
                last_menu_sync: None,
                last_sent_gui_state: None,
                last_reported_transform: None,
            },
            transport: SessionTransport {
                terrain: Default::default(),
                interest: Default::default(),
                view_radius: view_radius.clamp(4, 64),
            },
            player,
        }
    }

    #[inline]
    pub fn id(&self) -> PlayerId {
        self.id
    }

    #[inline]
    pub fn key(&self) -> crate::net::identity::PlayerKey {
        self.key
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn player(&self) -> &Player {
        &self.player
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    #[inline]
    pub fn mount(&self) -> Option<crate::mob::riding::Mount> {
        self.sim.mount
    }

    #[inline]
    pub fn menu(&self) -> &ContainerMenu {
        &self.sim.menu
    }

    #[inline]
    pub fn look(&self) -> Option<TargetRef> {
        self.input.look
    }

    #[inline]
    pub fn gui_state(&self) -> &std::sync::Arc<petramond_world::gui_state::GuiStateMap> {
        &self.sim.gui_state
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn sim(&self) -> &SessionSim {
        &self.sim
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn sim_mut(&mut self) -> &mut SessionSim {
        &mut self.sim
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn input(&self) -> &InputLatches {
        &self.input
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn input_mut(&mut self) -> &mut InputLatches {
        &mut self.input
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn replication(&self) -> &SessionReplication {
        &self.replication
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn replication_mut(&mut self) -> &mut SessionReplication {
        &mut self.replication
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn transport(&self) -> &SessionTransport {
        &self.transport
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn transport_mut(&mut self) -> &mut SessionTransport {
        &mut self.transport
    }

    #[inline]
    pub fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    pub fn latch_swing(
        &mut self,
        hand: petramond_world::inventory::Hand,
        kind: mod_api::SwingKind,
    ) {
        let swing = &mut self.replication.swing_events;
        match hand {
            petramond_world::inventory::Hand::Main => swing.main = Some(kind),
            petramond_world::inventory::Hand::Off => swing.off = Some(kind),
        }
    }

    #[inline]
    pub fn sneaking(&self) -> bool {
        self.input.intent_sneak && self.input.intent_gameplay
    }

    #[inline]
    pub fn using(&self) -> bool {
        self.input.intent_use_held && self.input.intent_gameplay
    }

    pub fn open_gui(&self) -> Option<crate::events::OpenGui> {
        match self.sim.menu.target() {
            crate::menu::ContainerTarget::None => None,
            crate::menu::ContainerTarget::Gui { kind, anchor } => {
                Some(crate::events::OpenGui { kind, anchor })
            }
        }
    }

    #[inline]
    pub fn held_rotation_snapshot(&self) -> HeldRotation {
        self.input.held_rotation.clone()
    }

    #[inline]
    pub fn held_slab_rotation(&self) -> petramond_world::slab::SlabRotation {
        self.input
            .held_rotation
            .slab_rotation(self.player.held().map(|st| st.item))
    }

    pub fn tick_effects(&mut self) {
        for behavior in self.player.tick_effects() {
            match behavior {
                petramond_world::effect::EffectBehavior::None
                | petramond_world::effect::EffectBehavior::Speed { .. } => {}
                petramond_world::effect::EffectBehavior::Regen { amount, .. } => {
                    self.player.heal(amount);
                }
            }
        }
    }

    pub fn eating_progress(&self) -> Option<f32> {
        let eat = self.sim.eating?;
        let ticks = eat.item.food()?.eat_ticks.max(1);
        Some(eat.progress as f32 / ticks as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_tracker_reports_water_entries_only_for_real_falls() {
        let mut fall = FallTracker::new(68.0);
        assert_eq!(fall.observe(68.0, false, false), None, "left the ledge");
        assert_eq!(fall.observe(66.0, false, false), None, "mid-air");
        assert_eq!(
            fall.observe(64.0, false, true),
            Some(FallOutcome::Splashed(4.0)),
            "the water entry reports the whole drop"
        );
        match fall.observe(63.8, false, true) {
            None => {}
            Some(FallOutcome::Splashed(d)) => {
                assert!(d < 0.5, "swimming reports only bob-sized drops: {d}")
            }
            other => panic!("unexpected {other:?}"),
        }

        let mut walk = FallTracker::new(64.0);
        assert_eq!(walk.observe(64.0, true, false), None);
        assert_eq!(
            walk.observe(64.0, true, true),
            None,
            "a grounded (walked-in) water entry reports nothing"
        );

        let mut dry = FallTracker::new(70.0);
        assert_eq!(dry.observe(70.0, false, false), None);
        assert_eq!(
            dry.observe(64.0, true, false),
            Some(FallOutcome::Landed(6.0)),
            "dry landings keep their distance"
        );
    }
}
