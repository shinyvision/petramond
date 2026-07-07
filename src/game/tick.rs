use super::Game;
use crate::block::Block;
use crate::events::{Attach, PostEvent, PostEventKind, SimCtx, Stage};
use crate::mathh::IVec3;
use crate::player;
use crate::world::StreamEvent;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
pub(super) const TICK_DT: f32 = 0.05;

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time.
const MAX_TICKS_PER_FRAME: u32 = 4;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct GameInput {
    /// False while an app screen such as inventory owns input focus.
    pub gameplay_enabled: bool,
    pub movement: MovementInput,
    pub look_delta: (f32, f32),
    /// Whole wheel notches scrolled this frame (signed): negative selects
    /// previous slots, positive selects next, 0 for none. Wraps within the hotbar.
    pub hotbar_scroll: i32,
    /// Level state: primary button held for mining.
    pub break_held: bool,
    /// Edge state: primary button *pressed* this frame.
    pub attack_clicked: bool,
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
    /// Level state: secondary button held — sustains an in-progress eat.
    pub use_held: bool,
}

/// One sound a mod emitted on the tick (`EmitSound` HostCall): resolved to a
/// runtime [`Sound`](crate::audio::Sound) id at call time, carried through the
/// tick→presentation channel, and played by the app layer each frame — the sim
/// never touches audio. `pos` is where it happened (`None` = non-spatial);
/// positional reach comes from the sound row's `attenuation_distance`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ModSound {
    pub sound: crate::audio::Sound,
    pub pos: Option<crate::mathh::Vec3>,
}

/// A semantic mob sound event produced by gameplay. The app resolves the
/// species' `mobs.json` sound hook and owns actual playback.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundEvent {
    pub mob_id: u64,
    pub kind: crate::mob::Mob,
    pub category: crate::mob::MobSoundCategory,
    pub pos: crate::mathh::Vec3,
}

/// A deterministic presentation command produced by the spatial sound HostCalls.
/// The app/audio side owns actual playback and active sinks; the sim only carries
/// resolved sound ids, stable handles, and positions through the tick event queue.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ModSpatialSoundCommand {
    PlayAt {
        handle: u64,
        sound: crate::audio::Sound,
        pos: crate::mathh::Vec3,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound: crate::audio::Sound,
        mob_id: u64,
        volume: f32,
        pitch: f32,
        /// The mob position when the command was emitted. If the mob despawns
        /// before the app sees a frame snapshot, playback starts and finishes here.
        last_pos: crate::mathh::Vec3,
    },
    Stop {
        handle: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameEvents {
    /// The block placed this frame, if any.
    pub placed_block: Option<Block>,
    /// The block broken (player-mined) this frame, if any.
    pub broke_block: Option<Block>,
    /// The hand swung this frame for an attack.
    pub swung_hand: bool,
    /// An item/stack left the hand for the world this frame.
    pub threw_item: bool,
    /// At least one dropped item was collected into the inventory this frame.
    pub picked_up_item: bool,
    /// The player right-clicked a placed crafting table this frame.
    pub open_crafting_table: bool,
    /// The player right-clicked a placed furnace this frame.
    pub open_furnace: Option<IVec3>,
    /// The player right-clicked a placed chest this frame.
    pub open_chest: Option<IVec3>,
    /// The player right-clicked a placed furniture workbench this frame.
    pub open_furniture_workbench: Option<IVec3>,
    /// A mod GUI should open this frame: from a block's `open_gui` interaction
    /// (`pos = Some`) or a mod's programmatic `GuiOpen` (`pos = None`).
    pub open_mod_gui: Option<(crate::gui::GuiKind, Option<IVec3>)>,
    /// A mod asked to close the open mod GUI this frame (`GuiClose`); the app
    /// honours it only while a mod GUI screen is actually up.
    pub close_mod_gui: bool,
    /// The player right-clicked a door this frame. Carries the door's NEW open
    /// state (after the toggle applied), so the presentation picks the open vs
    /// close sound. `None` = no door toggle this frame.
    pub toggled_door: Option<bool>,
    /// The player right-clicked a bed this frame. This fires even in daytime,
    /// when the click sets the spawn point but does not start sleep.
    pub bed_interacted: bool,
    /// The player's right-click was CONSUMED by a block interaction this frame
    /// (any `try_open_interactable` capability: container screens, mod GUIs,
    /// doors, beds, a mod cancelling `block_interact`…). Set at the sim's one
    /// decision point, so the interact hand jab is the DEFAULT for every
    /// interaction — present and future — with no per-kind enumeration.
    pub interacted: bool,
    /// The held item's own right-click use fired this frame (a bucket scooping
    /// or pouring water) — plays the same hand jab as placing.
    pub used_item: bool,
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
    pub mod_sounds: Vec<ModSound>,
    /// Spatial sound start/stop commands emitted by mods across this frame's
    /// fixed ticks. NON-lossy; the app/audio side owns active playback state.
    pub mod_spatial_sounds: Vec<ModSpatialSoundCommand>,
    /// Semantic mob sound events emitted by gameplay across this frame's fixed
    /// ticks. NON-lossy; the app resolves species data and plays them.
    pub mob_sounds: Vec<MobSoundEvent>,
}

/// What the world-mutating actions did across the fixed tick(s) that ran this frame.
/// The tick→presentation channel: the event bus feeds it (via `SimCtx::feed`),
/// never the other way around. Crate-visible so event handlers can write it.
/// The latched fields are lossy by design; `sounds` is the non-lossy per-tick
/// queue alongside them (every mod `EmitSound` plays exactly once).
#[derive(Clone, Debug)]
pub(crate) struct TickEvents {
    pub(crate) broke_block: Option<Block>,
    pub(crate) placed_block: Option<Block>,
    pub(crate) swung_hand: bool,
    pub(crate) picked_up_item: bool,
    pub(crate) threw_item: bool,
    pub(crate) used_item: bool,
    pub(crate) bed_interacted: bool,
    pub(crate) interacted: bool,
    pub(crate) player_damaged: bool,
    pub(crate) player_died: bool,
    pub(crate) sleep_ended: bool,
    pub(crate) respawned: bool,
    pub(crate) sounds: Vec<ModSound>,
    pub(crate) spatial_sounds: Vec<ModSpatialSoundCommand>,
    pub(crate) mob_sounds: Vec<MobSoundEvent>,
    next_spatial_sound_handle: u64,
}

impl Default for TickEvents {
    fn default() -> Self {
        Self::with_next_spatial_sound_handle(1)
    }
}

impl TickEvents {
    pub(crate) fn with_next_spatial_sound_handle(next_spatial_sound_handle: u64) -> Self {
        Self {
            broke_block: None,
            placed_block: None,
            swung_hand: false,
            picked_up_item: false,
            threw_item: false,
            used_item: false,
            bed_interacted: false,
            interacted: false,
            player_damaged: false,
            player_died: false,
            sleep_ended: false,
            respawned: false,
            sounds: Vec::new(),
            spatial_sounds: Vec::new(),
            mob_sounds: Vec::new(),
            next_spatial_sound_handle: next_spatial_sound_handle.max(1),
        }
    }

    pub(crate) fn next_spatial_sound_handle(&self) -> u64 {
        self.next_spatial_sound_handle
    }

    pub(crate) fn alloc_spatial_sound_handle(&mut self) -> u64 {
        let handle = self.next_spatial_sound_handle.max(1);
        self.next_spatial_sound_handle = handle.wrapping_add(1).max(1);
        handle
    }
}

impl Game {
    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        // Per-frame exceptions kept for local feel: look, hotbar, local player, mob push.
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.apply_mob_push(dt);
        self.tick_world();
        self.refresh_target();
        self.update_third_person(dt);

        self.capture_intent(input);
        let mut events = self.run_fixed_ticks(dt);

        // Presentation/infra after fixed simulation; no gameplay mutation here.
        self.tick_entities(dt);
        self.advance_chest_lids(dt);
        self.advance_door_swings(dt);
        self.tick_mesh_budget();
        self.refresh_dropped_item_lights_after_world_light_update();

        self.maybe_autosave(dt);

        GameEvents {
            placed_block: events.placed_block,
            broke_block: events.broke_block,
            swung_hand: events.swung_hand,
            picked_up_item: events.picked_up_item,
            threw_item: events.threw_item,
            open_crafting_table: std::mem::take(&mut self.request_open_table),
            open_furnace: std::mem::take(&mut self.request_open_furnace),
            open_chest: std::mem::take(&mut self.request_open_chest),
            open_furniture_workbench: std::mem::take(&mut self.request_open_workbench),
            open_mod_gui: std::mem::take(&mut self.request_open_mod_gui),
            close_mod_gui: std::mem::take(&mut self.request_close_mod_gui),
            toggled_door: self.toggled_door.take(),
            bed_interacted: events.bed_interacted,
            interacted: events.interacted,
            used_item: events.used_item,
            player_damaged: events.player_damaged,
            player_died: events.player_died,
            open_sleep: std::mem::take(&mut self.request_open_sleep),
            sleep_ended: events.sleep_ended,
            respawned: events.respawned,
            mod_sounds: std::mem::take(&mut events.sounds),
            mod_spatial_sounds: std::mem::take(&mut events.spatial_sounds),
            mob_sounds: std::mem::take(&mut events.mob_sounds),
        }
    }

    /// Latch this frame's input into the action-intent fields the fixed tick consumes.
    pub(super) fn capture_intent(&mut self, input: &GameInput) {
        self.intent_gameplay = input.gameplay_enabled;
        self.intent_sneak = input.movement.sneak;
        if !input.gameplay_enabled {
            // Menu focus drops queued action edges so clicks cannot fire behind screens.
            self.intent_break_held = false;
            self.intent_use_held = false;
            self.pending_attack = false;
            self.pending_place = false;
            return;
        }
        self.intent_break_held = input.break_held;
        self.intent_use_held = input.use_held;
        if input.attack_clicked {
            self.pending_attack = true;
        }
        if input.place_clicked {
            self.pending_place = true;
        }
    }

    fn run_fixed_ticks(&mut self, dt: f32) -> TickEvents {
        // Clamp long stalls and cap catch-up so fixed ticks never spiral.
        self.tick_accumulator += dt.clamp(0.0, 1.0);
        let mut ran = 0;
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        while self.tick_accumulator >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.game_tick_step(&mut events);
            self.tick_accumulator -= TICK_DT;
            ran += 1;
        }
        if self.tick_accumulator > TICK_DT {
            self.tick_accumulator = TICK_DT;
        }
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        events
    }

    /// One fixed game tick: world and entity mutation only. The hardwired engine
    /// steps run in [`Stage`] order; between them the scheduler runs attached
    /// systems and the post-event queue drains (see [`end_stage`](Self::end_stage)).
    /// `pub(super)` so tests can drive exactly one tick.
    pub(super) fn game_tick_step(&mut self, events: &mut TickEvents) {
        // Post events queued from per-frame code since the last tick (section
        // stream installs, container screens) dispatch first, before any stage:
        // per-frame code only ever queues; handlers run on the tick. Mod
        // actions still queued from the previous tick's final drain (or from
        // mod_init) apply here first.
        self.pump_stream_events();
        self.apply_mod_actions(events);
        self.drain_post_events(events);

        // Keep action intent before world/entity simulation so inputs resolve on the tick.
        self.begin_stage(Stage::Mining, events);
        self.tick_mining(events);
        self.end_stage(Stage::Mining, events);

        self.begin_stage(Stage::Placement, events);
        self.tick_place(events);
        self.end_stage(Stage::Placement, events);

        self.begin_stage(Stage::Attack, events);
        self.tick_attack(events);
        self.end_stage(Stage::Attack, events);

        self.begin_stage(Stage::Drops, events);
        self.tick_drops(events);
        self.end_stage(Stage::Drops, events);

        self.begin_stage(Stage::Menu, events);
        self.tick_menu(events);
        self.end_stage(Stage::Menu, events);

        self.begin_stage(Stage::PlayerDamage, events);
        self.tick_fall_damage(events);
        // Status effects ride the same stage: they are pure player-state
        // steps (regen heals, durations count down) on the tick, after damage
        // so a same-tick hit lands before the heal.
        self.tick_effects();
        // Sleeping and respawn ride the same stage: both are pure player-state
        // transitions (teleport, health restore, time skip) on the tick.
        self.tick_bed_and_respawn(events);
        self.end_stage(Stage::PlayerDamage, events);

        // World::game_tick's internal order (scheduled → block updates → furnaces
        // → random ticks) is its own sealed contract; the stage wraps it whole.
        self.begin_stage(Stage::WorldScheduled, events);
        self.world.game_tick(&self.recipes);
        self.dispatch_mod_block_hooks(events);
        self.end_stage(Stage::WorldScheduled, events);

        self.begin_stage(Stage::NaturalBreaks, events);
        self.process_natural_breaks();
        self.end_stage(Stage::NaturalBreaks, events);

        self.begin_stage(Stage::Pickup, events);
        if self.item_pickup_tick() {
            events.picked_up_item = true;
        }
        self.end_stage(Stage::Pickup, events);

        let player_pos = self.player.body_center();
        let player_body = (!self.player.is_spectator())
            .then(|| crate::mob::Body::new(self.player.pos, player::HALF_W, player::HEIGHT));

        self.begin_stage(Stage::Mobs, events);
        let attacks = self.world.tick_mobs(TICK_DT, player_pos, player_body);
        // Mob→player combat resolves right after the mobs moved: each strike runs
        // through the `player_damage_pre` pipeline (i-frame mods cancel there) and
        // an applied strike knocks the player back.
        self.apply_mob_attacks(attacks, events);
        self.end_stage(Stage::Mobs, events);

        self.begin_stage(Stage::ItemPhysics, events);
        self.world.tick_item_physics(TICK_DT, player_pos);
        self.end_stage(Stage::ItemPhysics, events);

        self.begin_stage(Stage::Spawning, events);
        for (kind, pos) in self.world.spawn_mobs_tick(player_pos) {
            self.bus.emit(PostEvent::MobSpawned { kind, pos });
        }
        self.tick_mod_hostile_mob_spawns(events);
        self.end_stage(Stage::Spawning, events);
    }

    /// Forward the behavior hooks the world tick queued on mod-behavior blocks
    /// (see `block::behavior::wasm`) to their owning mods, inside the same
    /// stage window as the world tick that fired them. The queue is drained
    /// unconditionally so it never carries over between ticks.
    fn dispatch_mod_block_hooks(&mut self, events: &mut TickEvents) {
        let hooks = self.world.take_mod_block_hooks();
        if hooks.is_empty() || !self.mods.has_block_behaviors() {
            return;
        }
        let Self {
            world,
            player,
            mods,
            bus,
            ..
        } = self;
        let mut ctx = SimCtx {
            world,
            player,
            feed: events,
            queue: bus.queue_mut(),
        };
        mods.dispatch_block_hooks(&mut ctx, &hooks);
    }

    fn tick_mod_hostile_mob_spawns(&mut self, events: &mut TickEvents) {
        if !self.mods.has_hostile_spawners() || crate::mob::hostile_cap_full(&self.world) {
            return;
        }

        let player_pos = self.player.pos;
        'attempts: for attempt in 0..crate::mob::HOSTILE_SPAWN_ATTEMPTS {
            let sites = crate::mob::hostile_attempt_sites(&self.world, player_pos, attempt);
            for site in sites {
                let kind = {
                    let Self {
                        world,
                        player,
                        mods,
                        bus,
                        ..
                    } = self;
                    let mut ctx = SimCtx {
                        world,
                        player,
                        feed: events,
                        queue: bus.queue_mut(),
                    };
                    mods.hostile_spawn_kind(&mut ctx, &site.candidate)
                };
                let Some(kind) = kind else {
                    continue;
                };
                if self.world.spawn_mob(kind, site.pos, site.yaw) {
                    self.bus.emit(PostEvent::MobSpawned {
                        kind,
                        pos: site.pos,
                    });
                }
                break 'attempts;
            }
        }
    }

    /// Run the systems attached at `at` — the mod seam. Nothing is attached in
    /// Phase 1, so this is a bounds-checked array read per stage edge.
    fn run_systems(&mut self, at: Attach, events: &mut TickEvents) {
        if self.systems.is_empty_at(at) {
            return;
        }
        self.systems.run(
            at,
            &mut self.world,
            &mut self.player,
            events,
            self.bus.queue_mut(),
        );
    }

    /// Open a stage: run its `Before` systems, then apply any mod actions they
    /// queued (`DamagePlayer`/`HurtMob`/... — see `apply_mod_actions`) BEFORE
    /// the engine step runs, so mob indices captured by those systems cannot be
    /// shifted by the step in between.
    fn begin_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::Before(stage), events);
        self.apply_mod_actions(events);
    }

    /// Close a stage: run its `After` systems, apply the mod actions they (or
    /// the stage's inline pre-event handlers) queued, then drain the post
    /// queue — so post events emitted by those actions (`player_damaged`,
    /// `mob_died`) dispatch within the same tick, at the earliest defined
    /// point. Actions queued by post handlers during the drain roll to the
    /// next action point (next stage or next tick's start) — no recursion.
    fn end_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::After(stage), events);
        self.apply_mod_actions(events);
        self.drain_post_events(events);
    }

    fn drain_post_events(&mut self, events: &mut TickEvents) {
        self.bus
            .drain_post(&mut self.world, &mut self.player, events);
    }

    /// Hand the section stream events buffered by the per-frame `World::poll` to
    /// the bus. The capture gate mirrors listener presence so an idle bus costs
    /// the streamer nothing.
    fn pump_stream_events(&mut self) {
        let wants = self.bus.wants(PostEventKind::SectionGenerated)
            || self.bus.wants(PostEventKind::SectionLoaded);
        self.world.set_stream_event_capture(wants);
        if !wants {
            return;
        }
        for ev in self.world.take_stream_events() {
            self.bus.emit(match ev {
                StreamEvent::Generated(pos) => PostEvent::SectionGenerated { pos },
                StreamEvent::Loaded(pos) => PostEvent::SectionLoaded { pos },
            });
        }
    }
}
