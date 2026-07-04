//! The smoke-test mod: the minimal proof of the whole Phase 2b loop
//! (init → register → tick dispatch → event dispatch → host calls), used by
//! the engine's integration tests and kept as the reference example. It also
//! answers the Phase 3b probe (world-KV read/write + a mob spawn) when the
//! engine plants `smoke:probe` — inert in normal play — and registers the
//! Phase 4 reference worldgen feature (a rare smoke-block pillar on open
//! land), the SDK's model of a seam-correct, deterministic gen hook.
//!
//! Its pack (`pack/`) also registers one namespaced block, `smoke:smoke_block`
//! — pure data, no wasm involvement — proving a single mod ships content and
//! logic together. Phase 5: the block's `open_gui` interaction opens the
//! `smoke:panel` GUI (a hand-authored manifest in the pack: one button + one
//! label bound to `smoke:count_text`); every `bump` click increments a counter
//! into that state key, proving the click→dispatch→state→label loop.

use mod_sdk::*;

const HEARTBEAT_SYSTEM: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const PILLAR_FEATURE: u32 = 1;

/// Log every this-many ticks (10 s at 20 TPS).
const HEARTBEAT_TICKS: u64 = 200;

/// Positional-RNG salt unique to this feature — decorrelates its stream from
/// the engine's and from other mods' (any constant of your own works).
const PILLAR_SALT: u64 = 0x0000_540C_E017_1A17;
/// Roughly one pillar per three chunks (per-column chance).
const PILLAR_CHANCE: f32 = 1.0 / 768.0;
/// Pillar cells above the surface (surface+1 ..= surface+PILLAR_HEIGHT).
const PILLAR_HEIGHT: i32 = 3;

#[derive(Default)]
struct Smoke {
    /// Blocks seen placed this session — state persisting across dispatches.
    placed: u64,
    /// The Phase 3b probe already answered (one-shot).
    probed: bool,
    /// Resolved at init; session-scoped (never persist numeric ids).
    smoke_block: Option<BlockId>,
}

impl Mod for Smoke {
    fn init(&mut self) {
        register_tick_system(Stage::Spawning, AttachSide::After, 0, HEARTBEAT_SYSTEM);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_worldgen_feature(WorldgenStage::Trees, PILLAR_FEATURE);
        // Init also runs on each per-thread worldgen instance, so it must stay
        // pure: registrations + resolves only (no sim-scoped calls here).
        self.smoke_block = resolve_block("smoke:smoke_block");
        log("initialized: heartbeat, block_placed observer, pillar worldgen feature");
    }

    fn tick_system(&mut self, _system_id: u32) {
        let tick = current_tick();
        if tick % HEARTBEAT_TICKS == 0 {
            log(&format!(
                "heartbeat at tick {tick} (roll {})",
                rng_u64("heartbeat") % 100
            ));
        }
        // Phase 3b probe (drives the integration test; inert in normal play):
        // when the engine side plants `smoke:probe` in the world KV, echo it to
        // `smoke:pong` and spawn one owl — a guest-side world-KV read+write and
        // an entity spawn in one round trip.
        if !self.probed {
            if let Some(probe) = world_kv_get("smoke:probe") {
                self.probed = true;
                world_kv_set("smoke:pong", probe);
                let spawned = spawn_mob("owl", [8.5, 80.5, 8.5], 0.0);
                log(&format!("probe answered (owl spawned: {spawned})"));
            }
        }
    }

    fn handle_event(&mut self, _handler_id: u32, payload: &mut EventPayload) -> Outcome {
        if let EventPayload::BlockPlaced { pos, block } = payload {
            self.placed += 1;
            log(&format!(
                "block #{} placed at {:?} ({} this session)",
                block.0, pos, self.placed
            ));
        }
        Outcome::Continue
    }

    /// Phase 4 reference feature: a rare 3-block smoke pillar on dry land.
    ///
    /// Every decision follows the GenCtx seam contract, so a pillar crossing a
    /// vertical section border comes out identical from both sections' calls:
    /// - the origin roll is positional RNG over (seed, column) — pure;
    /// - the anchor is the column surface height — identical column data in
    ///   every section of the column;
    /// - the per-cell "only over air" predicate uses `ctx.block`, which is
    ///   `Some` exactly for the cells THIS call may emit (out-of-section cells
    ///   are the neighbouring section's to check and emit).
    ///
    /// "On grass surface" is approximated as dry land (surface ≥ sea level):
    /// the hook inputs carry no per-column surface material, and checking the
    /// surface BLOCK from the snapshot would be impossible for a section that
    /// only contains the pillar's upper cells — an inconsistent decision would
    /// cut pillars at section seams. Margin is 0: a column-anchored feature
    /// writes only in its own column (see the GenCtx docs).
    fn gen_feature(&mut self, _feature_id: u32, ctx: &GenCtx) -> Vec<GenWrite> {
        let Some(block) = self.smoke_block else {
            return Vec::new(); // pack content missing — degrade, don't trap
        };
        let mut writes = Vec::new();
        ctx.for_each_origin(0, |wx, wz| {
            let Some(surface) = ctx.surface_y(wx, wz) else {
                return;
            };
            if surface < ctx.sea_level() {
                return; // submerged or floorless column
            }
            let mut rng = GenRng::positional(ctx.seed(), PILLAR_SALT, wx, 0, wz);
            if !rng.chance(PILLAR_CHANCE) {
                return;
            }
            for dy in 1..=PILLAR_HEIGHT {
                let p = [wx, surface + dy, wz];
                if ctx.block(p) == Some(BlockId::AIR) {
                    writes.push((p, block));
                }
            }
        });
        writes
    }

    /// Phase 5 reference GUI loop: every click of the `bump` button counts up
    /// through the session state map — a `GuiStateGet` + `GuiStateSet` round
    /// trip whose result the manifest's label (bound to `smoke:count_text`)
    /// draws next frame. The count lives in the SESSION map on purpose:
    /// closing and reopening the GUI resets it (clear-on-open contract).
    fn gui_click(&mut self, kind_key: &str, widget_id: &str, _pos: Option<[i32; 3]>) {
        if kind_key != "smoke:panel" || widget_id != "bump" {
            return;
        }
        let clicks = match gui_state_get("smoke:count") {
            Some(GuiValue::I32(n)) => n + 1,
            _ => 1,
        };
        gui_state_set("smoke:count", GuiValue::I32(clicks));
        gui_state_set("smoke:count_text", GuiValue::Str(format!("clicks: {clicks}")));
    }
}

register_mod!(Smoke);
