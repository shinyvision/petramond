//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.

use std::sync::Arc;

use glam::{IVec3, Quat, Vec3};

use crate::atlas::Tile;
use crate::block::Block;
use crate::block_model::BlockModelKind;
use crate::camera::ViewVolume;
use crate::door::DoorState;
use crate::facing::Facing;
use crate::item::ItemType;
use crate::mob::Mob;
use crate::render::{PlayerRenderInstance, RemotePlayerRender};
use crate::world::PlacedEmitter;

use super::remote_players;
use super::Game;

/// The block-break overlay to draw this frame: a cracked-texture overlay over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The cell-local visual box the crack hugs. `None` means an ordinary full cube.
    pub visual_box: Option<([f32; 3], [f32; 3])>,
    /// The cell's RESOLVED shape boxes, when its family has a box form: the
    /// crack traces THEM with cell-local UVs, so the decal hugs the real
    /// geometry (a stair's steps, a slab's occupied halves, a fence's post and
    /// rails, a chair's legs) instead of a box hanging in the cell's empty air.
    ///
    /// One field for every box family, because they all answer through the one
    /// box producer — a family is never named here.
    pub shape_boxes: Option<CrackBoxes>,
    /// A model block cracks over its cell's actual model cubes, including the targeted
    /// cell's authored footprint offset and placed facing.
    pub model: Option<(BlockModelKind, [u8; 3], Facing)>,
    /// 0..=9 crack stage.
    pub stage: u8,
}

/// The most cell-local boxes a crack traces (a chair is 7). A shape with more
/// truncates — the crack just covers fewer parts.
pub const MAX_CRACK_BOXES: usize = 16;

/// One cell-local box of a resolved shape, reduced to what a crack decal
/// needs: the box, and which of its faces the family actually emits. A face
/// the family never emits takes no destroy texture — that is what keeps a
/// ladder's crack off the wall behind it and a fence rail's end cap clean.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Canonical face order (`+X, -X, +Y, -Y, +Z, -Z`) — `mesh::face::Face::ALL`.
    pub faces: [bool; 6],
}

/// A bounded, `Copy` snapshot of a cell's resolved boxes for its break crack
/// (the view stays `Copy`, so no per-frame allocation).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CrackBoxes {
    pub boxes: [CrackBox; MAX_CRACK_BOXES],
    pub len: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ChestPresentation {
    pub(crate) pos: IVec3,
    pub(crate) facing: Facing,
    pub(crate) lid_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: crate::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DoorPresentation {
    pub(crate) pos: IVec3,
    pub(crate) state: DoorState,
    pub(crate) tiles: [Tile; 3],
    pub(crate) swing_progress: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: crate::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DroppedItemPresentation {
    pub(crate) prev_pos: Vec3,
    pub(crate) pos: Vec3,
    pub(crate) item: ItemType,
    pub(crate) variant: crate::item::VariantId,
    pub(crate) count: u8,
    pub(crate) prev_spin: f32,
    pub(crate) spin: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: crate::light::BlockLight6,
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
    pub(crate) uv_size: [f32; 2],
    pub(crate) tint: [f32; 3],
    pub(crate) alpha: f32,
    pub(crate) size: f32,
    /// Vertical cube elongation (1 = a cube; ambient rain streaks stretch).
    /// Only the Solid atlas path honors it.
    pub(crate) stretch: f32,
    pub(crate) skylight: u8,
    pub(crate) blocklight: crate::light::BlockLight6,
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
    pub(crate) blocklight: crate::light::BlockLight6,
    pub(crate) hurt_flash: f32,
    pub(crate) dead: bool,
    pub(crate) shorn: bool,
    /// Replicated active particle-emitter bundle ids (client-local
    /// `particle_emitters.json` catalog ids).
    pub(crate) emitters: Vec<u8>,
    /// Named model animations as `(name, phase, weight)` — each layered by
    /// the renderer over the walk/idle/rest base pose at its own
    /// tick-interpolated PHASE (seconds into the clip; a paused oar's phase
    /// holds) and CLIENT-side blend weight (fading in toward 1, out toward 0).
    pub(crate) anims: Vec<(String, f32, f32)>,
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
    /// The body renders seated (legs forward): mounted on a mob seat, or
    /// pinned at a pose anchor whose pose is `sitting`. Anchor poses outside
    /// the known vocabulary render the rest pose (see [`mount_renders_seated`]).
    pub(crate) seated: bool,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle).
    pub(crate) walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` fully crouched).
    pub(crate) sneak_weight: f32,
    /// Asleep in a bed: the body renders lying on its back, feet at `pos`,
    /// head toward `body_yaw`.
    pub(crate) sleeping: bool,
    pub(crate) skylight: u8,
    pub(crate) blocklight: crate::light::BlockLight6,
}

/// One body whose FOOTSTEPS the client may sound this frame — the local player
/// and every visible remote, together, so a step is heard at whoever took it.
///
/// Built here rather than in `App` because deciding it needs the world: the
/// sound is the block UNDER the feet, and only presentation has the replica.
/// The cadence itself is `App`'s (see `tick_footstep_sounds`), like the mob
/// idle schedule.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct FootstepSource {
    /// Stable cadence key: `0` is the local player, a remote is `1 + its
    /// PlayerId`. Ids are only compared, never sent.
    pub(crate) id: u64,
    /// Feet centre — where the sound plays, so a remote's steps arrive from
    /// their body and attenuate with distance like any other world sound.
    pub(crate) pos: Vec3,
    /// The block being walked on, or `None` when this body is not making
    /// footsteps at all: standing still, SNEAKING, airborne (the cell below is
    /// air), seated, asleep, or over an unloaded cell. `App` never re-decides
    /// this.
    pub(crate) ground: Option<crate::block::Block>,
    /// Moving at a sprint — the gait `App` picks the step interval from.
    ///
    /// Derived from the body's ACTUAL horizontal speed on both sides rather
    /// than from a sprint key: a key held while the body is blocked, wading, or
    /// climbing must not quicken the cadence, and speed needs no new
    /// replication (a remote's velocity already ships for its walk blend).
    pub(crate) sprinting: bool,
}

pub(crate) struct GamePresentation<'a> {
    pub(crate) tick_alpha: f32,
    pub(crate) item_entities: &'a [DroppedItemPresentation],
    pub(crate) particles: &'a [ParticlePresentation],
    /// Every emitter — block rows and mobs alike — whose particles are inside
    /// this frame's view volume, already culled by the gather.
    pub(crate) particle_emitters: &'a [PlacedEmitter],
    pub(crate) chests: &'a [ChestPresentation],
    /// Mod-submitted per-block draw sets with the light at their cell — the
    /// gather's own rows, handed on without a re-spelling copy.
    pub(crate) block_draws: &'a [crate::world::draw::BlockDrawInstance],
    pub(crate) doors: &'a [DoorPresentation],
    pub(crate) mobs: &'a [MobPresentation],
    /// Every OTHER connected player's body + held item for this frame,
    /// already interpolated and posed — the render input rows themselves
    /// (`build_player_body` consumes `PlayerRenderInstance` directly, so no
    /// second translation buys anything).
    pub(crate) remote_players: &'a [RemotePlayerRender],
    /// Every body that could sound a footstep this frame (see
    /// [`FootstepSource`]) — INCLUDING bodies standing still, so `App` can
    /// retire the cadence state of players who left without a second list.
    pub(crate) footsteps: &'a [FootstepSource],
    pub(crate) player: Option<PlayerPresentation>,
    pub(crate) held_item_light: (u8, crate::light::BlockLight6),
    /// Every break (crack) overlay to draw this frame: the LOCAL player's own
    /// mining target plus each visible remote's replicated one, capped at the
    /// [`MAX_BREAK_OVERLAYS`] nearest to the camera.
    pub(crate) break_overlays: &'a [BreakOverlayView],
}

/// The local player's [`FootstepSource`] key. Remotes are `1 + PlayerId`, so
/// zero can never collide with one.
const LOCAL_FOOTSTEP_ID: u64 = 0;

/// Horizontal speed (blocks/s) below which a body is not walking. Well
/// under a sneak (half walk) and well over the drift a current or a
/// conveyor-ish push imparts to a standing body.
const MIN_FOOTSTEP_SPEED: f32 = 0.5;

/// Horizontal speed (blocks/s) at or above which a body is SPRINTING —
/// midway between `player::movement::WALK` (4.3) and `SPRINT` (5.6), so
/// either gait clears it by a wide margin and a slowed sprint honestly
/// reads as a walk.
const SPRINT_FOOTSTEP_SPEED: f32 = 4.95;

/// Walk-blend weight above which a REMOTE body is walking. The blend is eased,
/// so this is a hysteresis-free threshold on an already-smoothed signal.
const MIN_FOOTSTEP_WALK_WEIGHT: f32 = 0.35;

/// Break overlays drawn per frame (own + remotes), nearest-to-camera first
/// under contention — a bound so a crowd of miners can't grow the bake.
const MAX_BREAK_OVERLAYS: usize = 8;

#[derive(Default)]
pub(crate) struct GamePresentationScratch {
    /// Ambient (precipitation) volume drives — presentation-owned state,
    /// targets set from client mods each frame; see [`super::ambient`].
    pub(crate) ambient: super::ambient::AmbientDrives,
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
    particle_emitters: Vec<PlacedEmitter>,
    chest_rows: Vec<(IVec3, Facing, u8, crate::light::BlockLight6)>,
    block_draws: Vec<crate::world::draw::BlockDrawInstance>,
    door_rows: Vec<(IVec3, DoorState, [Tile; 3], u8, crate::light::BlockLight6)>,
    chests: Vec<ChestPresentation>,
    doors: Vec<DoorPresentation>,
    mobs: Vec<MobPresentation>,
    remote_players: Vec<RemotePlayerRender>,
    footsteps: Vec<FootstepSource>,
    break_overlays: Vec<BreakOverlayView>,
}

impl GamePresentationScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `now` is the app render clock — the same seconds looping emitters
    /// animate on; the ambient volumes derive against it. `view` is the volume
    /// the frame will actually draw, so gathers that can be large cull against
    /// it instead of handing the renderer everything that is loaded.
    pub(crate) fn snapshot<'a>(
        &'a mut self,
        game: &Game,
        now: f32,
        view: &ViewVolume,
    ) -> GamePresentation<'a> {
        let tick_alpha = game.tick_alpha();
        self.collect_item_entities(game);
        self.collect_particles(game);
        self.collect_ambient(game, now);
        self.collect_particle_emitters(game, view);
        self.collect_chests(game);
        self.collect_block_draws(game, view);
        self.collect_doors(game);
        self.collect_mobs(game, tick_alpha);
        self.collect_mob_emitters(tick_alpha, view);
        self.collect_remote_players(game, tick_alpha);
        self.collect_footsteps(game, tick_alpha);
        self.collect_break_overlays(game);

        GamePresentation {
            tick_alpha,
            item_entities: &self.item_entities,
            particles: &self.particles,
            particle_emitters: &self.particle_emitters,
            chests: &self.chests,
            block_draws: &self.block_draws,
            doors: &self.doors,
            mobs: &self.mobs,
            remote_players: &self.remote_players,
            footsteps: &self.footsteps,
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
                    // Re-intern the row's blob (idempotent hash probe once the
                    // variant exists); an unreadable blob renders plain.
                    variant: entry
                        .curr
                        .data
                        .as_deref()
                        .and_then(crate::item::variant::intern_blob)
                        .unwrap_or_default(),
                    count: entry.curr.count,
                    prev_spin: entry.prev.spin,
                    spin: entry.curr.spin,
                    skylight: world.skylight6_at_world(c.x, c.y, c.z),
                    blocklight: crate::light::BlockLight6::from_x2(
                        world.blocklight_rgb_at_world(c.x, c.y, c.z),
                    ),
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
                    alpha: particle.alpha(),
                    size: particle.render_size(),
                    stretch: 1.0,
                    skylight: particle.skylight,
                    blocklight: particle.blocklight,
                }
            }));
    }

    /// Sync ambient drive targets from the client-mod runtime, then append
    /// this frame's derived precipitation rows to `particles` (they ride the
    /// same Solid-atlas path as burst droplets). The particles graphics
    /// option governs the derive like every other particle producer.
    fn collect_ambient(&mut self, game: &Game, now: f32) {
        game.client_mods.sync_ambient_targets(&mut self.ambient);
        self.ambient.collect(
            &game.replica,
            game.listener_position(),
            now,
            game.particles.count_scale(),
            &mut self.particles,
        );
    }

    /// Looping emitters exist only to make particles, so the particles
    /// graphics option gates the gather itself — off costs nothing at all,
    /// like every other particle producer.
    fn collect_particle_emitters(&mut self, game: &Game, view: &ViewVolume) {
        if game.particles.count_scale() <= 0.0 {
            self.particle_emitters.clear();
            return;
        }
        game.replica
            .collect_particle_emitters(view, &mut self.particle_emitters);
    }

    fn collect_block_draws(&mut self, game: &Game, view: &ViewVolume) {
        game.replica
            .collect_block_draws(view, &mut self.block_draws);
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
                blocklight: crate::light::BlockLight6::from_x2(
                    world.blocklight_rgb_at_world(c.x, c.y, c.z),
                ),
                hurt_flash: crate::mob::hurt_flash01(prev.hurt_timer, curr.hurt_timer, tick_alpha),
                dead: curr.dead,
                shorn: curr.shorn,
                emitters: curr.emitters.clone(),
                emitter_tint: emitter_tint(&curr.emitters),
                // Each layer at its tick-interpolated phase (prev→curr by
                // name; fading-out layers hold the blend's last phase).
                anims: entry
                    .anim_blend
                    .iter()
                    .map(|(name, weight, held)| {
                        let phase = match (
                            prev.anims.iter().find(|(n, _)| n == name),
                            curr.anims.iter().find(|(n, _)| n == name),
                        ) {
                            (Some((_, a)), Some((_, b))) => a + (b - a) * tick_alpha,
                            (_, Some((_, b))) => *b,
                            _ => *held,
                        };
                        (name.clone(), phase, *weight)
                    })
                    .collect(),
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
    fn collect_mob_emitters(&mut self, tick_alpha: f32, view: &ViewVolume) {
        if self.mobs.is_empty() {
            return;
        }
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
                    let origin = feet + Vec3::from_array(emitter.offset);
                    let envelope = crate::world::emitter_envelope(emitter);
                    if !view.aabb_visible(origin - envelope, origin + envelope) {
                        continue;
                    }
                    self.particle_emitters.push(PlacedEmitter {
                        origin,
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

    /// One footstep row per body — the local player plus every visible remote
    /// — with the block under its feet resolved from the replica.
    ///
    /// "Is this body walking" is answered from the BEST signal each side has,
    /// and they are deliberately different: the local player owns real physics
    /// (`on_ground` + velocity), while a remote is only ever seen through the
    /// replicated transform, whose movement the shared `BodyPose` has already
    /// distilled into the `walk_weight` that drives its legs. Footsteps
    /// agreeing with the animation is the point.
    ///
    /// SNEAKING is the exception that reads a FLAG on both sides, not a speed:
    /// it already ships (observers need it to render the crouch), and a sneak
    /// is only ~2.15 blocks/s, close enough to a laboured walk that inferring
    /// it would silence ordinary movement.
    ///
    /// AIRBORNE NEEDS NO TEST: the cell under the feet is air, air is
    /// `BlockMaterial::None`, and the silent set answers no step sound. The
    /// same fall-through covers an unloaded cell and any block whose material
    /// has no sounds yet.
    fn collect_footsteps(&mut self, game: &Game, tick_alpha: f32) {
        self.footsteps.clear();
        let world = &game.replica;
        // The block a body at `pos` (feet centre, model y=0) stands on.
        let ground = |pos: Vec3, walking: bool| -> Option<crate::block::Block> {
            if !walking {
                return None;
            }
            let c = crate::mathh::voxel_at(pos - Vec3::new(0.0, 0.1, 0.0));
            world.block_if_loaded(c.x, c.y, c.z)
        };
        let p = &game.player;
        let speed = Vec3::new(p.vel.x, 0.0, p.vel.z).length();
        self.footsteps.push(FootstepSource {
            id: LOCAL_FOOTSTEP_ID,
            pos: p.pos,
            ground: ground(
                p.pos,
                p.on_ground
                    && speed >= MIN_FOOTSTEP_SPEED
                    && !game.predicted_input.sneak
                    && game.self_mount.is_none(),
            ),
            sprinting: speed >= SPRINT_FOOTSTEP_SPEED,
        });
        for (id, rp) in game.remote_players.iter_with_ids() {
            if !rp.curr.visible {
                continue;
            }
            let (pos, _, _) = remote_players::interpolate(&rp.prev, &rp.curr, tick_alpha);
            let vel = rp.curr.transform.vel;
            let speed = Vec3::new(vel.x, 0.0, vel.z).length();
            let walking = rp.pose.walk_weight >= MIN_FOOTSTEP_WALK_WEIGHT
                && !rp.curr.sneaking
                && !rp.curr.sleeping
                && rp.curr.mount.is_none();
            self.footsteps.push(FootstepSource {
                id: 1 + id.0 as u64,
                pos,
                ground: ground(pos, walking),
                sprinting: speed >= SPRINT_FOOTSTEP_SPEED,
            });
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
            let mut body_yaw = p.pose.body_yaw;
            let mut head_yaw = yaw - body_yaw;
            // A mounted body GLUES to the interpolated mount: position at the
            // seat offset instead of the row lerp (a turning mount rotates
            // the seat along an arc the lerp would cut across), and the BODY
            // sits square in the seat — its yaw is the mount's facing, only
            // the clamped head follows the look. If the referenced mount row
            // is not available yet, keep the rider row's own interpolation.
            if let Some((seat_pos, mount_yaw)) = p.curr.mount.and_then(|mount| {
                // Mob yaw is mount convention (0 faces `-Z`), π from player
                // body yaw; a pose anchor already carries player-convention
                // yaw.
                let (seat_pos, body_yaw) = match mount {
                    crate::net::protocol::PlayerMount::Mob { id, seat } => {
                        let (seat_pos, mob_yaw) = game
                            .replicated_mobs
                            .interpolated_seat_pose(id, seat, tick_alpha)?;
                        (seat_pos, mob_yaw + std::f32::consts::PI)
                    }
                    crate::net::protocol::PlayerMount::Anchor { pos, yaw, .. } => (pos, yaw),
                };
                Some((seat_pos, crate::game::body_pose::wrap_angle(body_yaw)))
            }) {
                pos = seat_pos;
                body_yaw = mount_yaw;
                head_yaw = crate::game::body_pose::wrap_angle(yaw - body_yaw)
                    .clamp(-SEATED_HEAD_YAW_LIMIT, SEATED_HEAD_YAW_LIMIT);
            }
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
                    // Walking bodies: the follow rule keeps `yaw - body_yaw`
                    // within the head limit, no re-wrapping needed. Seated
                    // bodies clamped it against the seat facing above.
                    head_yaw,
                    head_pitch: pitch,
                    anim_time: p.pose.anim_time,
                    walk_weight: p.pose.walk_weight,
                    sneak_weight: p.pose.sneak_weight,
                    sleeping,
                    seated: p.curr.mount.is_some_and(mount_renders_seated),
                    hurt: p.hurt_flash01(),
                    skylight: world.skylight6_at_world(c.x, c.y, c.z),
                    blocklight: crate::light::BlockLight6::from_x2(
                        world.blocklight_rgb_at_world(c.x, c.y, c.z),
                    ),
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

/// How far a SEATED body's head may swivel off the seat facing (radians).
/// The body itself sits square in the seat (its yaw is the mount's), so the
/// look must not drag it around — and an unclamped relative head yaw would
/// owl the neck when the rider looks backward.
const SEATED_HEAD_YAW_LIMIT: f32 = 1.2;

/// Whether a wire mount renders the SEATED body pose: every mob seat, and a
/// pose anchor holding the `sitting` pose. An anchor pose outside the known
/// vocabulary renders the rest pose — like a disabled pack, never an error.
fn mount_renders_seated(mount: crate::net::protocol::PlayerMount) -> bool {
    match mount {
        crate::net::protocol::PlayerMount::Mob { .. } => true,
        crate::net::protocol::PlayerMount::Anchor { pose, .. } => pose == mod_api::pose::SITTING,
    }
}

fn collect_player(game: &Game) -> Option<PlayerPresentation> {
    // The body draws only once the boom camera is actually placed — never on a
    // frame whose render camera is still the first-person eye (inside the head).
    if !game.third_person_enabled() || game.third_person.cam.is_none() {
        return None;
    }
    let (skylight, blocklight) = game.held_item_light();
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
    // Seated: the body sits SQUARE in the seat — its yaw is the mount's
    // facing, never the look-follow (which would spin the whole body, legs
    // through the hull); only the head follows the look, clamped.
    let seated = game.self_mount.is_some_and(mount_renders_seated);
    let (body_yaw, head_yaw) = match game.mount_body_yaw() {
        Some(mount_yaw) => (
            mount_yaw,
            crate::game::body_pose::wrap_angle(game.player.yaw - mount_yaw)
                .clamp(-SEATED_HEAD_YAW_LIMIT, SEATED_HEAD_YAW_LIMIT),
        ),
        None => (
            game.third_person.pose.body_yaw,
            game.player.yaw - game.third_person.pose.body_yaw,
        ),
    };
    Some(PlayerPresentation {
        pos,
        body_yaw,
        head_yaw,
        head_pitch: game.player.pitch,
        anim_time: game.third_person.pose.anim_time,
        seated,
        walk_weight: game.third_person.pose.walk_weight,
        sneak_weight: game.third_person.pose.sneak_weight,
        sleeping,
        skylight,
        blocklight,
    })
}

/// The crack overlay for a miner's `(block, stage)` — the target + stage come
/// from replicated state (the own `SelfState::mining` or a remote row's); the
/// shape details are derived from the REPLICA world at that cell.
fn break_overlay_at(game: &Game, block: IVec3, stage: u8) -> BreakOverlayView {
    let block_type = Block::from_id(game.replica.chunk_block(block.x, block.y, block.z));
    let model = block_type.model_kind().map(|kind| {
        (
            kind,
            game.replica.model_offset_at(block.x, block.y, block.z),
            game.replica.model_facing_at(block.x, block.y, block.z),
        )
    });
    // The ONE box producer answers for every box family at once; nothing here
    // asks which family it is.
    let mut resolved = Vec::new();
    game.replica.shape_draw_boxes(block, &mut resolved);
    let shape_boxes = (!resolved.is_empty()).then(|| {
        let mut boxes = [CrackBox {
            min: [0.0; 3],
            max: [0.0; 3],
            faces: [false; 6],
        }; MAX_CRACK_BOXES];
        let len = resolved.len().min(MAX_CRACK_BOXES);
        for (dst, b) in boxes.iter_mut().zip(resolved.iter()).take(len) {
            *dst = CrackBox {
                min: b.aabb.min,
                max: b.aabb.max,
                faces: std::array::from_fn(|fi| b.faces[fi].is_some()),
            };
        }
        CrackBoxes {
            boxes,
            len: len as u8,
        }
    });
    BreakOverlayView {
        block,
        visual_box: if model.is_some() || shape_boxes.is_some() {
            None
        } else {
            game.replica.selection_box_at(block.x, block.y, block.z)
        },
        shape_boxes,
        model,
        stage,
    }
}
