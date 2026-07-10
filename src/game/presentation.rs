//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, ParticleEmitter, RenderShape};
use crate::block_model::BlockModelKind;
use crate::door::DoorState;
use crate::facing::Facing;
use crate::item::ItemType;
use crate::mob::Mob;
use crate::render::{PlayerRenderInstance, RemotePlayerRender};
use crate::stair::StairShape;

use super::remote_players;
use super::Game;

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    /// A stair's corner-resolved shape: the crack rebuilds the exact
    /// quads the chunk mesher emitted for it (`mesh::stair::plane_quads`).
    pub stair_shape: Option<StairShape>,
    /// A slab cell's layer state: the crack rebuilds the exact per-layer quads
    /// the chunk mesher emitted (`mesh::slab::layer_quads`), so the decal is
    /// cropped to the occupied halves rather than stretched over them.
    pub slab_state: Option<crate::block_state::SlabState>,
    /// A pane's resolved connection mask: the crack rebuilds the exact post/arm
    /// faces the chunk mesher emitted (`mesh::pane::shape_faces`), so the decal
    /// hugs the connected shape rather than a box around it.
    pub pane_mask: Option<u8>,
    /// A model block cracks over its cell's actual model cubes, including the targeted
    /// cell's authored footprint offset and placed facing.
    pub model: Option<(BlockModelKind, [u8; 3], Facing)>,
    /// 0..=9 crack stage.
    pub stage: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ChestPresentation {
    pub(crate) pos: IVec3,
    pub(crate) facing: Facing,
    pub(crate) lid_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DoorPresentation {
    pub(crate) pos: IVec3,
    pub(crate) state: DoorState,
    pub(crate) tiles: [Tile; 3],
    pub(crate) swing_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DroppedItemPresentation {
    pub(crate) prev_pos: Vec3,
    pub(crate) pos: Vec3,
    pub(crate) item: ItemType,
    pub(crate) count: u8,
    pub(crate) prev_spin: f32,
    pub(crate) spin: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParticleAtlas {
    Block,
    Model,
    /// No atlas: a solid-color cube (an emitter-burst particle — water
    /// splash). `tint` IS the color; drawn alpha-blended with the looping
    /// emitter cubes instead of through the cutout fleck pipeline.
    Solid,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ParticlePresentation {
    pub(crate) atlas: ParticleAtlas,
    pub(crate) pos: Vec3,
    pub(crate) uv_min: [f32; 2],
    pub(crate) uv_size: f32,
    pub(crate) tint: [f32; 3],
    pub(crate) warm: u8,
    pub(crate) alpha: f32,
    pub(crate) size: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ParticleEmitterPresentation {
    pub(crate) origin: Vec3,
    pub(crate) emitter: ParticleEmitter,
    pub(crate) seed: u64,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MobPresentation {
    pub(crate) id: u64,
    pub(crate) kind: Mob,
    pub(crate) prev_pos: Vec3,
    pub(crate) pos: Vec3,
    pub(crate) prev_yaw: f32,
    pub(crate) yaw: f32,
    pub(crate) prev_anim_time: f32,
    pub(crate) anim_time: f32,
    pub(crate) moving: bool,
    pub(crate) idle_anim: Option<u8>,
    pub(crate) prev_head_yaw: f32,
    pub(crate) head_yaw: f32,
    pub(crate) prev_head_pitch: f32,
    pub(crate) head_pitch: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
    pub(crate) hurt_flash: f32,
    pub(crate) dead: bool,
    pub(crate) shorn: bool,
    /// Replicated active particle-emitter bundle ids (client-local
    /// `particle_emitters.json` catalog ids).
    pub(crate) emitters: Vec<u8>,
    /// Body tint composed from the active bundles' `tint` values (white when
    /// none) — multiplied into the render tint like the hurt flash.
    pub(crate) emitter_tint: [f32; 3],
    pub(crate) ragdoll_pose: Option<Arc<[(Vec3, Quat)]>>,
}

/// The local player's third-person body for this frame, or absent in first person.
/// Player movement/look are per-frame (already smooth), so unlike mobs there are
/// no prev/current pairs to interpolate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PlayerPresentation {
    /// Feet centre (model `y=0`).
    pub(crate) pos: Vec3,
    /// Body facing yaw (engine yaw space).
    pub(crate) body_yaw: f32,
    /// Head yaw relative to the body (radians) and look pitch.
    pub(crate) head_yaw: f32,
    pub(crate) head_pitch: f32,
    /// Seconds into the walk animation.
    pub(crate) anim_time: f32,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle).
    pub(crate) walk_weight: f32,
    /// Asleep in a bed: the body renders lying on its back, feet at `pos`,
    /// head toward `body_yaw`.
    pub(crate) sleeping: bool,
    pub(crate) skylight: u8,
    pub(crate) blocklight: u8,
}

pub(crate) struct GamePresentation<'a> {
    pub(crate) tick_alpha: f32,
    pub(crate) item_entities: &'a [DroppedItemPresentation],
    pub(crate) particles: &'a [ParticlePresentation],
    pub(crate) particle_emitters: &'a [ParticleEmitterPresentation],
    pub(crate) chests: &'a [ChestPresentation],
    pub(crate) doors: &'a [DoorPresentation],
    pub(crate) mobs: &'a [MobPresentation],
    /// Every OTHER connected player's body + held item for this frame,
    /// already interpolated and posed — the render input rows themselves
    /// (`build_player_body` consumes `PlayerRenderInstance` directly, so no
    /// second translation buys anything).
    pub(crate) remote_players: &'a [RemotePlayerRender],
    pub(crate) player: Option<PlayerPresentation>,
    pub(crate) held_item_light: (u8, u8, u8),
    /// Every break (crack) overlay to draw this frame: the LOCAL player's own
    /// mining target plus each visible remote's replicated one, capped at the
    /// [`MAX_BREAK_OVERLAYS`] nearest to the camera.
    pub(crate) break_overlays: &'a [BreakOverlayView],
}

/// Break overlays drawn per frame (own + remotes), nearest-to-camera first
/// under contention — a bound so a crowd of miners can't grow the bake.
const MAX_BREAK_OVERLAYS: usize = 8;

#[derive(Default)]
pub(crate) struct GamePresentationScratch {
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
    particle_emitter_rows: Vec<(Vec3, ParticleEmitter, u64, u8, u8)>,
    particle_emitters: Vec<ParticleEmitterPresentation>,
    chest_rows: Vec<(IVec3, Facing, u8, u8)>,
    door_rows: Vec<(IVec3, DoorState, [Tile; 3], u8, u8)>,
    chests: Vec<ChestPresentation>,
    doors: Vec<DoorPresentation>,
    mobs: Vec<MobPresentation>,
    remote_players: Vec<RemotePlayerRender>,
    break_overlays: Vec<BreakOverlayView>,
}

impl GamePresentationScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot<'a>(&'a mut self, game: &Game) -> GamePresentation<'a> {
        let tick_alpha = game.tick_alpha();
        self.collect_item_entities(game);
        self.collect_particles(game);
        self.collect_particle_emitters(game);
        self.collect_chests(game);
        self.collect_doors(game);
        self.collect_mobs(game, tick_alpha);
        self.collect_mob_emitters(tick_alpha);
        self.collect_remote_players(game, tick_alpha);
        self.collect_break_overlays(game);

        GamePresentation {
            tick_alpha,
            item_entities: &self.item_entities,
            particles: &self.particles,
            particle_emitters: &self.particle_emitters,
            chests: &self.chests,
            doors: &self.doors,
            mobs: &self.mobs,
            remote_players: &self.remote_players,
            player: collect_player(game),
            held_item_light: game.held_item_light(),
            break_overlays: &self.break_overlays,
        }
    }

    fn collect_item_entities(&mut self, game: &Game) {
        self.item_entities.clear();
        // REPLICATED store: prev/curr batch rows are the interpolation pair.
        // Light is client-sampled at the item's cell, from the REPLICA world.
        let world = &game.replica;
        self.item_entities
            .extend(game.replicated_items.iter().map(|entry| {
                let c = crate::mathh::voxel_at(entry.curr.pos);
                DroppedItemPresentation {
                    prev_pos: entry.prev.pos,
                    pos: entry.curr.pos,
                    item: crate::item::ItemType(entry.curr.item_id),
                    count: entry.curr.count,
                    prev_spin: entry.prev.spin,
                    spin: entry.curr.spin,
                    skylight: world.skylight6_at_world(c.x, c.y, c.z),
                    blocklight: world.blocklight6_at_world(c.x, c.y, c.z),
                }
            }));
    }

    fn collect_particles(&mut self, game: &Game) {
        self.particles.clear();
        self.particles
            .extend(game.particles.particles().iter().map(|particle| {
                let (uv_min, uv_size) = particle.atlas_uv();
                ParticlePresentation {
                    atlas: if particle.solid {
                        ParticleAtlas::Solid
                    } else if particle.model.is_some() {
                        ParticleAtlas::Model
                    } else {
                        ParticleAtlas::Block
                    },
                    pos: particle.pos,
                    uv_min,
                    uv_size,
                    tint: particle.tint,
                    warm: particle.warm,
                    alpha: particle.alpha(),
                    size: particle.render_size(),
                    skylight: particle.skylight,
                    blocklight: particle.blocklight,
                }
            }));
    }

    fn collect_particle_emitters(&mut self, game: &Game) {
        game.replica
            .collect_particle_emitters(&mut self.particle_emitter_rows);
        self.particle_emitters.clear();
        self.particle_emitters
            .extend(self.particle_emitter_rows.iter().map(
                |&(origin, emitter, seed, skylight, blocklight)| ParticleEmitterPresentation {
                    origin,
                    emitter,
                    seed,
                    skylight,
                    blocklight,
                },
            ));
    }

    fn collect_chests(&mut self, game: &Game) {
        game.replica.collect_chests(&mut self.chest_rows);
        self.chests.clear();
        self.chests.extend(
            self.chest_rows
                .iter()
                .map(|&(pos, facing, skylight, blocklight)| ChestPresentation {
                    pos,
                    facing,
                    lid_progress: game.chest_lid_angle(pos),
                    skylight,
                    blocklight,
                }),
        );
    }

    fn collect_doors(&mut self, game: &Game) {
        game.replica.collect_doors(&mut self.door_rows);
        self.doors.clear();
        self.doors.extend(self.door_rows.iter().map(
            |&(pos, state, tiles, skylight, blocklight)| DoorPresentation {
                pos,
                state,
                tiles,
                swing_progress: game.door_swing_angle(pos),
                skylight,
                blocklight,
            },
        ));
    }

    fn collect_mobs(&mut self, game: &Game, tick_alpha: f32) {
        self.mobs.clear();
        // REPLICATED store: prev/curr batch rows are the interpolation pair
        // (the same blend the renderer used to run over `Instance::prev_*`).
        // Light is client-sampled at the mob's body cell (the sim's sampling
        // point), from the REPLICA world.
        let world = &game.replica;
        self.mobs.extend(game.replicated_mobs.iter().map(|entry| {
            let (prev, curr) = (&entry.prev, &entry.curr);
            let c = crate::mathh::voxel_at(curr.pos + Vec3::new(0.0, 0.3, 0.0));
            MobPresentation {
                id: curr.id,
                kind: Mob(curr.kind_id),
                prev_pos: prev.pos,
                pos: curr.pos,
                prev_yaw: prev.yaw,
                yaw: curr.yaw,
                prev_anim_time: prev.anim_time,
                anim_time: curr.anim_time,
                moving: curr.moving,
                idle_anim: curr.idle_anim,
                prev_head_yaw: prev.head_yaw,
                head_yaw: curr.head_yaw,
                prev_head_pitch: prev.head_pitch,
                head_pitch: curr.head_pitch,
                skylight: world.skylight6_at_world(c.x, c.y, c.z),
                blocklight: world.blocklight6_at_world(c.x, c.y, c.z),
                hurt_flash: crate::mob::hurt_flash01(prev.hurt_timer, curr.hurt_timer, tick_alpha),
                dead: curr.dead,
                shorn: curr.shorn,
                emitters: curr.emitters.clone(),
                emitter_tint: emitter_tint(&curr.emitters),
                ragdoll_pose: curr.ragdoll.as_ref().map(|pose| {
                    crate::game::replicated::lerp_ragdoll(prev.ragdoll.as_ref(), pose, tick_alpha)
                        .into()
                }),
            }
        }));
    }

    /// Mobs emit particles exactly like emitter blocks: each ACTIVE bundle id
    /// resolves to its `particle_emitters.json` rows and feeds the same
    /// transient-particle pipeline, anchored to the mob's interpolated feet each
    /// frame (the whole particle column rides along — the effect stays ON the
    /// mob; a row's `offset` raises it into the body). Appends to the
    /// block-emitter list collected earlier this frame, after `collect_mobs` so
    /// it reads the replicated ids. A ragdolling corpse keeps its ids, so a mob
    /// that burned to death keeps burning through its ragdoll.
    fn collect_mob_emitters(&mut self, tick_alpha: f32) {
        for m in &self.mobs {
            if m.emitters.is_empty() {
                continue;
            }
            let feet = m.prev_pos.lerp(m.pos, tick_alpha);
            let mut stream = 0u64;
            for &id in &m.emitters {
                let Some(bundle) = crate::particle_emitters::def(id) else {
                    continue;
                };
                for emitter in bundle.rows {
                    stream += 1;
                    self.particle_emitters.push(ParticleEmitterPresentation {
                        origin: feet + Vec3::from_array(emitter.offset),
                        emitter: *emitter,
                        // Distinct deterministic stream per mob and per row, so
                        // sibling rows' schedules don't pulse in lockstep.
                        seed: m.id ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                        skylight: m.skylight,
                        blocklight: m.blocklight,
                    });
                }
            }
        }
    }

    /// One render row per VISIBLE remote player, mirroring `collect_mobs`:
    /// transform interpolated between the prev/curr batch rows at
    /// `tick_alpha`, the shared body pose + per-remote held-item view read
    /// from the store (advanced once per frame in `Game::tick_receive`),
    /// light client-sampled from the replica at the interpolated body.
    fn collect_remote_players(&mut self, game: &Game, tick_alpha: f32) {
        self.remote_players.clear();
        let world = &game.replica;
        for p in game.remote_players.iter() {
            // Spectators and the dead ship rows (flags/actions keep flowing)
            // but draw no body.
            if !p.curr.visible {
                continue;
            }
            let (mut pos, yaw, pitch) = remote_players::interpolate(&p.prev, &p.curr, tick_alpha);
            let sleeping = p.curr.sleeping;
            let body_yaw = p.pose.body_yaw;
            if sleeping {
                // Mirror of `collect_player`'s sleeping branch: the sleeper
                // stands at the bed-group centre; the lying model's feet
                // anchor shifts back so the head lands on the pillow.
                pos.x -= body_yaw.sin() * 0.925;
                pos.z -= body_yaw.cos() * 0.925;
            }
            // Sample light at the body's torso cell (~mid-height).
            let c = crate::mathh::voxel_at(pos + Vec3::new(0.0, 0.9, 0.0));
            self.remote_players.push(RemotePlayerRender {
                body: PlayerRenderInstance {
                    pos,
                    body_yaw,
                    // The follow rule keeps `yaw - body_yaw` within the head
                    // limit, so the relative head yaw needs no re-wrapping
                    // (same contract as the local body).
                    head_yaw: yaw - body_yaw,
                    head_pitch: pitch,
                    anim_time: p.pose.anim_time,
                    walk_weight: p.pose.walk_weight,
                    sleeping,
                    hurt: p.hurt_flash01(),
                    skylight: world.skylight6_at_world(c.x, c.y, c.z),
                    blocklight: world.blocklight6_at_world(c.x, c.y, c.z),
                },
                held: p.view,
            });
        }
    }

    /// One break (crack) overlay per active miner this frame: the local
    /// player's own target (from the replicated self view) plus every VISIBLE
    /// remote row's replicated target + stage, each shaped against the replica
    /// exactly like the own overlay always was. Capped at the
    /// [`MAX_BREAK_OVERLAYS`] nearest to the camera.
    fn collect_break_overlays(&mut self, game: &Game) {
        self.break_overlays.clear();
        if let Some((block, stage)) = game.self_view.mining {
            self.break_overlays
                .push(break_overlay_at(game, block, stage));
        }
        for p in game.remote_players.iter() {
            if !p.curr.visible {
                continue;
            }
            if let Some((block, stage)) = p.curr.mining {
                self.break_overlays
                    .push(break_overlay_at(game, block, stage));
            }
        }
        if self.break_overlays.len() > MAX_BREAK_OVERLAYS {
            let cam = game.render_camera().pos;
            let dist = |v: &BreakOverlayView| {
                (Vec3::new(
                    v.block.x as f32 + 0.5,
                    v.block.y as f32 + 0.5,
                    v.block.z as f32 + 0.5,
                ) - cam)
                    .length_squared()
            };
            self.break_overlays
                .sort_by(|a, b| dist(a).total_cmp(&dist(b)));
            self.break_overlays.truncate(MAX_BREAK_OVERLAYS);
        }
    }
}

/// The third-person body row, when the view is active. The body-yaw follow rule
/// keeps `yaw - body_yaw` within the head limit, so the relative head yaw needs
/// no re-wrapping here.
/// The multiply body tint for a mob's active emitter-bundle ids: the product of
/// every active bundle's declared `tint` (white when none declare one).
fn emitter_tint(ids: &[u8]) -> [f32; 3] {
    let mut tint = [1.0, 1.0, 1.0];
    for &id in ids {
        if let Some(t) = crate::particle_emitters::def(id).and_then(|b| b.tint) {
            tint = [tint[0] * t[0], tint[1] * t[1], tint[2] * t[2]];
        }
    }
    tint
}

fn collect_player(game: &Game) -> Option<PlayerPresentation> {
    // The body draws only once the boom camera is actually placed — never on a
    // frame whose render camera is still the first-person eye (inside the head).
    if !game.third_person_enabled() || game.third_person.cam.is_none() {
        return None;
    }
    let (skylight, blocklight, _warm) = game.held_item_light();
    // The body shares the first-person camera's auto-step vertical easing (a
    // negative, settling lag) so stepping up a ledge glides instead of popping.
    let mut pos = game.player.pos;
    pos.y += game.camera_step_y_offset;
    // Sleep state reads the replicated self view (the sim's SleepState stays
    // server-side).
    let sleeping = game.self_view.sleeping.is_some();
    if sleeping {
        // The sleeper stands at the bed-group CENTRE; the lying model's feet
        // anchor shifts back toward the foot end so the head lands on the pillow
        // (bed length 2, model ~1.85 → feet ~0.925 behind centre).
        let head_yaw = game.third_person.pose.body_yaw;
        pos.x -= head_yaw.sin() * 0.925;
        pos.z -= head_yaw.cos() * 0.925;
    }
    Some(PlayerPresentation {
        pos,
        body_yaw: game.third_person.pose.body_yaw,
        head_yaw: game.player.yaw - game.third_person.pose.body_yaw,
        head_pitch: game.player.pitch,
        anim_time: game.third_person.pose.anim_time,
        walk_weight: game.third_person.pose.walk_weight,
        sleeping,
        skylight,
        blocklight,
    })
}

/// The crack overlay for a miner's `(block, stage)` — the target + stage come
/// from replicated state (the own `SelfState::mining` or a remote row's); the
/// shape details are derived from the REPLICA world at that cell.
fn break_overlay_at(game: &Game, block: IVec3, stage: u8) -> BreakOverlayView {
    let model =
        match Block::from_id(game.replica.chunk_block(block.x, block.y, block.z)).render_shape() {
            RenderShape::Model(kind) => Some((
                kind,
                game.replica.model_offset_at(block.x, block.y, block.z),
                game.replica.model_facing_at(block.x, block.y, block.z),
            )),
            _ => None,
        };
    let block_type = Block::from_id(game.replica.chunk_block(block.x, block.y, block.z));
    let stair_shape = (block_type.render_shape() == RenderShape::Stair)
        .then(|| game.replica.stair_shape_at(block.x, block.y, block.z));
    let slab_state = game.replica.slab_state_if_slab(block);
    let pane_mask =
        (block_type.render_shape() == RenderShape::Pane).then(|| game.replica.pane_mask_at(block));
    BreakOverlayView {
        block,
        visual_box: if model.is_some()
            || stair_shape.is_some()
            || slab_state.is_some()
            || pane_mask.is_some()
        {
            None
        } else {
            game.replica.selection_box_at(block.x, block.y, block.z)
        },
        stair_shape,
        slab_state,
        pane_mask,
        model,
        stage,
    }
}
