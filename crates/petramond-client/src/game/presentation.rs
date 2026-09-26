//! Neutral per-frame presentation data read from [`Game`].
//!
//! `Game` owns simulation state and transient client animation state. The app builds
//! this snapshot once per draw and passes it to presentation consumers, keeping render
//! wire structs out of `Game` while avoiding direct `Game` reads from those consumers.
//! Player bodies go through the client's animation stage ([`PlayerAnimation`]) on the
//! way: the gather reads each body, the stage poses it, and the snapshot carries only
//! the posed rows.

use glam::{IVec3, Vec3};

use petramond::mob::Mob;
use petramond::world::PlacedEmitter;
use petramond_math::math::Tilt;
use petramond_render::camera::ViewVolume;
use petramond_render::{AnimLayer, ArenaRange, GaitFade, MobArena};
use petramond_world::block::Block;

use super::remote_players;
use super::Game;
use crate::animation::{BodyFrame, BodyInput, BodyState, FootstepSource, PlayerAnimation, RemoteBody};

mod entity_emitters;
#[cfg(test)]
mod tests;
pub use entity_emitters::BodyEmitters;

pub use petramond_render::views::{
    BlockEntityPresentation, BreakOverlayView, CrackBox, CrackBoxes, DroppedItemPresentation,
    EntityShadow, GamePresentation, MobPresentation, ModelCrack, ParticleAtlas,
    ParticlePresentation, MAX_CRACK_BOXES,
};

/// One frame's presentation: the render snapshot, plus what goes to the
/// client audio instead of the renderer. Reads through to the render
/// snapshot.
pub struct FramePresentation<'a> {
    pub render: GamePresentation<'a>,
    /// Every body that could sound a footstep this frame (see
    /// [`FootstepSource`]) — INCLUDING bodies standing still, so the audio
    /// can retire the cadence state of players who left without a second
    /// list.
    pub footsteps: &'a [FootstepSource],
}

impl<'a> std::ops::Deref for FramePresentation<'a> {
    type Target = GamePresentation<'a>;

    fn deref(&self) -> &Self::Target {
        &self.render
    }
}

/// The local player's [`FootstepSource`] key. Remotes are `1 + PlayerId`, so
/// zero can never collide with one.
const LOCAL_FOOTSTEP_ID: u64 = 0;
/// Mob footstep sources are keyed apart from players' (`1 + player id`).
const MOB_FOOTSTEP_IDS: u64 = 1 << 62;
/// How far a body may rise or fall in one tick and still be walking, blocks.
const MOB_STEP_RISE: f64 = 0.2;

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

/// Where a body whose light is sampled at `cell` is lit from. Inside solid
/// ground every light level is zero — true, and no way to draw a body coming
/// up out of it (or sunk into it by anything else): it is lit by the open
/// air it stands up into, the first cell above that is not an opaque block.
fn lit_cell(world: &petramond::world::ReplicaWorld, cell: IVec3) -> IVec3 {
    const RISE: i32 = 4;
    (0..=RISE)
        .map(|dy| cell + IVec3::new(0, dy, 0))
        .find(|c| {
            world
                .data().block_if_loaded(c.x, c.y, c.z)
                .is_none_or(|block| !block.is_opaque())
        })
        .unwrap_or(cell)
}

/// The block a body at `pos` (feet centre, model y=0) steps on. A shape lying
/// flat in the feet's own cell answers first: a snow layer or carpet never
/// collides, so the body stands on the block beneath it, but the cover is what
/// the foot presses. A plant does not lie flat, so it is still walked through.
fn footstep_ground(
    world: &petramond::world::ReplicaWorld,
    pos: petramond_math::world_pos::WorldPos,
) -> Option<Block> {
    let feet = (pos + Vec3::new(0.0, 0.05, 0.0)).block();
    if let Some(cover) = world.data().block_if_loaded(feet.x, feet.y, feet.z) {
        if petramond_world::block::rests_flat_on_floor(world.data(), feet, cover) {
            return Some(cover);
        }
    }
    let below = (pos - Vec3::new(0.0, 0.1, 0.0)).block();
    world.data().block_if_loaded(below.x, below.y, below.z)
}

// Entity blob-shadow tuning. The gather owns all of it: the renderer just
// stamps quads.
/// How many cells below an entity's feet the ground probe searches before
/// giving up (no shadow when nothing is within reach — a body falling past a
/// cliff fades out long before this).
const SHADOW_PROBE_DEPTH: i32 = 6;
/// How far under a surface feet may be drawn and still stand on it (blocks):
/// settling error, and the step-up easing of a half-block ledge.
const SHADOW_SINK: f32 = 0.6;
/// Body height above its ground at which the shadow has fully faded.
const SHADOW_MAX_DROP: f32 = 4.0;
/// Peak darkening at a shadow's centre for a body resting on its ground.
const SHADOW_STRENGTH: f32 = 0.42;
/// Mob decal radius as a multiple of the species' largest collision half-extent.
const MOB_SHADOW_RADIUS_SCALE: f32 = 1.5;
/// Player bodies: half-width 0.3 × the mob scale, rounded to a tuned value.
const PLAYER_SHADOW_RADIUS: f32 = 0.45;
/// A dropped item is one small spinning cube.
const ITEM_SHADOW_RADIUS: f32 = 0.25;

#[derive(Default)]
pub struct GamePresentationScratch {
    /// Ambient (precipitation) volume drives — presentation-owned state,
    /// targets set from client mods each frame; see [`super::ambient`].
    pub ambient: super::ambient::AmbientDrives,
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
    particle_emitters: Vec<PlacedEmitter>,
    animated_rows: Vec<petramond::world::animated_block::AnimatedBlock>,
    block_draws: Vec<petramond::world::draw::BlockDrawInstance>,
    block_entities: Vec<BlockEntityPresentation>,
    mobs: Vec<MobPresentation>,
    /// The mob rows' fading gaits, animation layers and ragdoll bones, which
    /// each row addresses by range — reused, so a steady frame allocates
    /// nothing per mob.
    mob_arena: MobArena,
    /// The local body's emitters, refreshed from the replicated conditions
    /// each frame (a compare unless they changed).
    local_emitters: BodyEmitters,
    /// Scratch for one break overlay's resolved shape boxes, reused across
    /// overlays and frames.
    overlay_boxes: Vec<petramond_world::block::ShapeBox>,
    /// Every player animator and the frame's player bodies — the gather
    /// fills its [`BodyFrame`], the stage poses what the view sees.
    /// Presentation-owned state, like the ambient drives; the app opens
    /// each frame on it ([`PlayerAnimation::begin_frame`]).
    pub animation: PlayerAnimation,
    shadows: Vec<EntityShadow>,
    footsteps: Vec<FootstepSource>,
    break_overlays: Vec<BreakOverlayView>,
}

impl GamePresentationScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// `now` is the app render clock — the same seconds looping emitters
    /// animate on; the ambient volumes derive against it. `view` is the volume
    /// the frame will actually draw, so gathers that can be large cull against
    /// it instead of handing the renderer everything that is loaded — and the
    /// player bodies it sees are the ones the animation stage poses.
    pub fn snapshot<'a>(
        &'a mut self,
        game: &Game,
        now: f32,
        view: &ViewVolume,
    ) -> FramePresentation<'a> {
        let tick_alpha = game.tick_alpha();
        self.collect_item_entities(game);
        self.collect_particles(game);
        self.collect_ambient(game, now);
        self.collect_particle_emitters(game, view);
        self.collect_block_entities(game);
        self.collect_block_draws(game, view);
        self.collect_mob_draws(game, tick_alpha, view);
        self.collect_mobs(game, tick_alpha);
        if game.fx.particles.count_scale() > 0.0 {
            self.collect_mob_emitters(game, tick_alpha, view);
        }
        self.animation.frame.clear();
        self.collect_remote_players(game, tick_alpha, view);
        self.collect_footsteps(game, tick_alpha);
        self.collect_break_overlays(game);
        self.local_emitters
            .refresh(&[], &game.replica.self_view.conditions);
        let player = collect_player(game, &self.local_emitters, &mut self.animation.frame);
        self.animation.frame.local = player;
        if game.fx.particles.count_scale() > 0.0 {
            self.collect_local_player_emitters(game, player.as_ref(), view);
        }
        self.collect_entity_shadows(game, view, tick_alpha);
        self.animation.pose_bodies(view);

        FramePresentation {
            render: GamePresentation {
                tick_alpha,
                item_entities: &self.item_entities,
                particles: &self.particles,
                particle_emitters: &self.particle_emitters,
                block_entities: &self.block_entities,
                block_draws: &self.block_draws,
                mobs: &self.mobs,
                mob_arena: &self.mob_arena,
                anim_names: game.replica.entities.mobs().anim_names(),
                bodies: self.animation.bodies(),
                body_poses: self.animation.poses(),
                held_item_light: game.held_item_light(),
                break_overlays: &self.break_overlays,
                shadows: &self.shadows,
            },
            footsteps: &self.footsteps,
        }
    }

    fn collect_item_entities(&mut self, game: &Game) {
        self.item_entities.clear();
        // REPLICATED store: prev/curr batch rows are the interpolation pair.
        // Light is client-sampled at the item's cell, from the REPLICA world.
        let world = &game.replica.world;
        self.item_entities
            .extend(game.replica.entities.items().iter().map(|entry| {
                let c = entry.curr.pos.block();
                DroppedItemPresentation {
                    prev_pos: entry.prev.pos,
                    pos: entry.curr.pos,
                    item: petramond_world::item::ItemType(entry.curr.item_id),
                    // Re-intern the row's blob (idempotent hash probe once the
                    // variant exists); an unreadable blob renders plain.
                    variant: entry
                        .curr
                        .data
                        .as_deref()
                        .and_then(|b| petramond_world::item::variant::intern_blob(b).ok())
                        .unwrap_or_default(),
                    count: entry.curr.count,
                    prev_spin: entry.prev.spin,
                    spin: entry.curr.spin,
                    prev_flight: entry.prev.flight,
                    flight: entry.curr.flight,
                    skylight: world.data().skylight6_at_world(c.x, c.y, c.z),
                    blocklight: petramond_world::light::BlockLight6::from_x2(
                        world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                    ),
                }
            }));
    }

    fn collect_particles(&mut self, game: &Game) {
        self.particles.clear();
        self.particles
            .extend(game.fx.particles.particles().iter().map(|particle| {
                let (uv_min, uv_size) = particle.atlas_uv();
                ParticlePresentation {
                    quad_axes: None,
                    atlas: match particle.source {
                        crate::particle::ParticleSource::Solid => ParticleAtlas::Solid,
                        crate::particle::ParticleSource::Model(_) => ParticleAtlas::Model,
                        crate::particle::ParticleSource::Tile { .. } => ParticleAtlas::Block,
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
    /// this frame's derived ambient rows to `particles` (cubes ride the
    /// same Solid-atlas path as burst droplets). The particles graphics
    /// option governs the derive like every other particle producer.
    fn collect_ambient(&mut self, game: &Game, now: f32) {
        for (mod_id, bundle, intensity, wind) in game.client_mods.ambient_targets() {
            self.ambient.set(mod_id, bundle, intensity, wind);
        }
        self.ambient.collect(
            &game.replica.world,
            game.listener_position(),
            now,
            game.fx.particles.count_scale(),
            &mut self.particles,
        );
    }

    /// Looping emitters exist only to make particles, so the particles
    /// graphics option gates the gather itself — off costs nothing at all,
    /// like every other particle producer.
    fn collect_particle_emitters(&mut self, game: &Game, view: &ViewVolume) {
        if game.fx.particles.count_scale() <= 0.0 {
            self.particle_emitters.clear();
            return;
        }
        game.replica.world
            .collect_particle_emitters(view, &mut self.particle_emitters);
    }

    fn collect_block_draws(&mut self, game: &Game, view: &ViewVolume) {
        game.replica.world
            .collect_block_draws(view, &mut self.block_draws);
    }

    /// Draw sets live bodies wear, appended to the block sets: the same rows,
    /// framed at each body's interpolated feet instead of a cell.
    fn collect_mob_draws(&mut self, game: &Game, tick_alpha: f32, view: &ViewVolume) {
        let world = &game.replica.world;
        for entry in game.replica.entities.mobs().iter() {
            let Some(set) = &entry.draw else { continue };
            let Some((lo, hi)) = set.bounds else { continue };
            if entry.curr.dead {
                continue;
            }
            let (pos, yaw) = entry.interpolated_pose(tick_alpha);
            let anchor = pos.block();
            let turn = if entry.curr.draw.turns { yaw } else { 0.0 };
            let frame = petramond::world::draw::BlockLocalFrame {
                anchor,
                transform: petramond_math::math::Mat4::from_translation(pos.relative_to(anchor))
                    * petramond_math::math::Mat4::from_rotation_y(turn),
            };
            let (mn, mx) = petramond::world::draw::world_bounds(&frame, lo, hi);
            if !view.aabb_visible(mn, mx) {
                continue;
            }
            // Lit where the set is, not at the feet: a mark over a body
            // standing in a doorway's shadow hangs in the light above it.
            let c = ((mn.block() + mx.block()).as_vec3() * 0.5)
                .floor()
                .as_ivec3();
            self.block_draws
                .push(petramond::world::draw::BlockDrawInstance {
                    pos: anchor,
                    set: std::sync::Arc::clone(set),
                    frame,
                    skylight: world.data().skylight6_at_world(c.x, c.y, c.z),
                    blocklight: petramond_world::light::BlockLight6::from_x2(
                        world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                    ),
                });
        }
    }

    /// Every animated block (chests, doors, trapdoors, a pack's own) from the
    /// ONE world gather, each with its eased open progress.
    fn collect_block_entities(&mut self, game: &Game) {
        game.replica.world.collect_animated_blocks(&mut self.animated_rows);
        self.block_entities.clear();
        self.block_entities
            .extend(self.animated_rows.iter().map(|&block| BlockEntityPresentation {
                block,
                open_progress: game.fx.block_open_progress(block.pos, block.pose.open),
            }));
    }

    /// One row per replicated mob, in the store's order (the emitter gather
    /// pairs rows with entries by position). The variable-length parts —
    /// fading gaits, animation layers, the ragdoll pose — go into the frame's
    /// [`MobArena`], so a row is a plain value and a steady frame allocates
    /// nothing.
    fn collect_mobs(&mut self, game: &Game, tick_alpha: f32) {
        self.mobs.clear();
        self.mob_arena.clear();
        // REPLICATED store: prev/curr batch rows are the interpolation pair
        // (the same blend the renderer used to run over `Instance::prev_*`).
        // Light is client-sampled at the mob's body cell (the sim's sampling
        // point), from the REPLICA world.
        let world = &game.replica.world;
        let arena = &mut self.mob_arena;
        for entry in game.replica.entities.mobs().iter() {
            let (prev, curr) = (&entry.prev, &entry.curr);
            let c = lit_cell(world, (curr.pos + Vec3::new(0.0, 0.3, 0.0)).block());
            let gait = crate::game::replicated::gait_of(curr);
            let fades_start = arena.gait_fades.len();
            arena.gait_fades.extend(
                entry
                    .gait_blend
                    .iter()
                    .filter(|(clip, _, _)| Some(*clip) != gait)
                    .map(|&(clip, weight, phase)| GaitFade {
                        clip,
                        phase,
                        weight,
                    }),
            );
            // Each layer at its tick-interpolated phase (prev→curr by id;
            // fading-out layers hold the blend's last phase).
            let anims_start = arena.anims.len();
            arena.anims.extend(entry.anim_blend.iter().map(|&(anim, weight, held)| {
                let phase = match (
                    entry.prev_anims().iter().find(|(id, _)| *id == anim),
                    entry.curr_anims().iter().find(|(id, _)| *id == anim),
                ) {
                    (Some((_, a)), Some((_, b))) => a + (b - a) * tick_alpha,
                    (_, Some((_, b))) => *b,
                    _ => held,
                };
                AnimLayer {
                    anim,
                    phase,
                    weight,
                }
            }));
            let ragdoll_pose = curr.ragdoll.as_deref().map(|pose| {
                let start = arena.ragdoll.len();
                crate::game::replicated::lerp_ragdoll(
                    prev.ragdoll.as_deref(),
                    pose,
                    tick_alpha,
                    &mut arena.ragdoll,
                );
                ArenaRange::since(&arena.ragdoll, start)
            });
            let emitters = entry.emitters();
            self.mobs.push(MobPresentation {
                id: curr.id,
                kind: Mob(curr.kind_id),
                prev_pos: prev.pos,
                pos: curr.pos,
                prev_yaw: prev.yaw,
                yaw: curr.yaw,
                prev_tilt: prev.tilt,
                tilt: curr.tilt,
                prev_anim_time: prev.anim_time,
                anim_time: curr.anim_time,
                moving: curr.moving,
                idle_anim: curr.idle_anim,
                gait_weight: entry
                    .gait_blend
                    .iter()
                    .find(|(clip, _, _)| Some(*clip) == gait)
                    .map_or(1.0, |(_, weight, _)| *weight),
                gait_fades: ArenaRange::since(&arena.gait_fades, fades_start),
                prev_head_yaw: prev.head_yaw,
                head_yaw: curr.head_yaw,
                prev_head_pitch: prev.head_pitch,
                head_pitch: curr.head_pitch,
                skylight: world.data().skylight6_at_world(c.x, c.y, c.z),
                blocklight: petramond_world::light::BlockLight6::from_x2(
                    world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                ),
                hurt_flash: petramond::mob::hurt_flash01(
                    prev.hurt_timer,
                    curr.hurt_timer,
                    tick_alpha,
                ),
                dead: curr.dead,
                shorn: curr.shorn,
                emitter_tint: emitters.tint(),
                emitter_self_lit: emitters.self_lit(),
                anims: ArenaRange::since(&arena.anims, anims_start),
                ragdoll_pose,
                held: curr.held.map(|id| id.map(petramond_world::item::ItemType)),
            });
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
        let world = &game.replica.world;
        let ground = |pos: petramond_math::world_pos::WorldPos, walking: bool| {
            walking.then(|| footstep_ground(world, pos)).flatten()
        };
        let p = &game.local.player;
        let speed = Vec3::new(p.vel.x, 0.0, p.vel.z).length();
        self.footsteps.push(FootstepSource {
            id: LOCAL_FOOTSTEP_ID,
            pos: p.pos,
            ground: ground(
                p.pos,
                p.on_ground
                    && speed >= MIN_FOOTSTEP_SPEED
                    && !game.local.predicted_input.sneak
                    && game.replica.entities.own_mount().is_none(),
            ),
            sprinting: speed >= SPRINT_FOOTSTEP_SPEED,
        });
        for (id, rp) in game.replica.entities.players().iter_with_ids() {
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
        for entry in game.replica.entities.mobs().iter() {
            let (prev, curr) = (&entry.prev, &entry.curr);
            if curr.dead || !petramond::mob::def(Mob(curr.kind_id)).footsteps {
                continue;
            }
            // Airborne (a jump, a fall) takes no steps.
            let walking = curr.moving && (curr.pos.y - prev.pos.y).abs() < MOB_STEP_RISE;
            let pos = prev.pos.lerp(curr.pos, tick_alpha);
            self.footsteps.push(FootstepSource {
                id: MOB_FOOTSTEP_IDS | curr.id,
                pos,
                ground: ground(pos, walking),
                sprinting: false,
            });
        }
    }

    /// One body row per VISIBLE remote player, mirroring `collect_mobs`:
    /// transform interpolated between the prev/curr batch rows at
    /// `tick_alpha`, the shared body pose + per-remote held-item view read
    /// from the store (advanced once per frame in `Game::tick_receive`),
    /// light client-sampled from the replica at the interpolated body.
    fn collect_remote_players(&mut self, game: &Game, tick_alpha: f32, view: &ViewVolume) {
        let world = &game.replica.world;
        for p in game.replica.entities.players().iter() {
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
            // the seat along an arc the lerp would cut across), the BODY
            // sits square in the seat and leans with it — its yaw is the
            // mount's facing, only the clamped head follows the look. If the
            // referenced mount row is not available yet, keep the rider
            // row's own interpolation.
            let mut seat_tilt = Tilt::LEVEL;
            if let Some(mount) = p
                .curr
                .mount
                .and_then(|mount| game.replica.entities.mobs().mount_pose(mount, tick_alpha))
            {
                pos = mount.seat;
                body_yaw = mount.body_yaw;
                seat_tilt = mount.tilt;
                head_yaw = petramond_math::math::wrap_angle(yaw - body_yaw)
                    .clamp(-SEATED_HEAD_YAW_LIMIT, SEATED_HEAD_YAW_LIMIT);
            }
            if sleeping {
                // Mirror of `collect_player`'s sleeping branch: the sleeper
                // stands at the bed-group centre; the lying model's feet
                // anchor shifts back so the head lands on the pillow.
                pos -= Vec3::new(body_yaw.sin(), 0.0, body_yaw.cos()) * 0.925;
            }
            // Sample light at the body's torso cell (~mid-height).
            let c = (pos + Vec3::new(0.0, 0.9, 0.0)).block();
            let emitters = p.emitters();
            let frame = &mut self.animation.frame;
            let animator = frame.push_animator(&p.curr.animator, &p.plays, &p.events);
            let body = BodyInput {
                pos,
                emitter_tint: emitters.tint(),
                emitter_self_lit: emitters.self_lit(),
                skylight: world.skylight6_at_world(c.x, c.y, c.z),
                blocklight: petramond_world::light::BlockLight6::from_x2(
                    world.blocklight_rgb_at_world(c.x, c.y, c.z),
                ),
                state: BodyState {
                    body_yaw,
                    // Walking bodies: the follow rule keeps `yaw - body_yaw`
                    // within the head limit, no re-wrapping needed. Seated
                    // bodies clamped it against the seat facing above.
                    head_yaw,
                    head_pitch: pitch,
                    anim_time: p.pose.anim_time,
                    walk_weight: p.pose.walk_weight,
                    sneak_weight: p.pose.sneak_weight,
                    locomotion: p.pose.locomotion,
                    sleeping,
                    seated: p.curr.mount.is_some_and(mount_renders_seated),
                    seat_tilt,
                    hurt: p.hurt_flash01(),
                    skylight: world.data().skylight6_at_world(c.x, c.y, c.z),
                    blocklight: petramond_world::light::BlockLight6::from_x2(
                        world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                    ),
                    bones: push_bones(&mut self.bone_offsets, p.bones.current()),
                },
                bones: frame.push_bones(p.bones.current()),
            };
            frame.remotes.push(RemoteBody {
                key: p.curr.id.0 as u32,
                body,
                held: [p.view, p.off_view],
                frames: p.frames,
                animator,
            });
            if game.fx.particles.count_scale() > 0.0 {
                let body = entity_emitters::EmitterBody::player(
                    body.pos,
                    body.state.body_yaw,
                    body.skylight,
                    body.blocklight,
                    p.curr.id.0 as u64 + 1,
                );
                self.append_player_emitters(p.emitters(), &body, view);
            }
        }
    }

    /// One break (crack) overlay per active miner this frame: the local
    /// player's own target (from the replicated self view) plus every VISIBLE
    /// remote row's replicated target + stage, each shaped against the replica
    /// exactly like the own overlay always was. Capped at the
    /// [`MAX_BREAK_OVERLAYS`] nearest to the camera.
    fn collect_break_overlays(&mut self, game: &Game) {
        self.break_overlays.clear();
        let boxes = &mut self.overlay_boxes;
        if let Some((block, stage)) = game.replica.self_view.mining {
            self.break_overlays
                .push(break_overlay_at(game, block, stage, boxes));
        }
        for p in game.replica.entities.players().iter() {
            if !p.curr.visible {
                continue;
            }
            if let Some((block, stage)) = p.curr.mining {
                self.break_overlays
                    .push(break_overlay_at(game, block, stage, boxes));
            }
        }
        for mob in game.replica.entities.mobs().iter() {
            if let Some((block, stage)) = mob.curr.dig {
                self.break_overlays
                    .push(break_overlay_at(game, block, stage, boxes));
            }
        }
    }

    /// One blob-shadow row per shadowed entity this frame: every mob, dropped
    /// item, and body (local third-person + remotes) that has ground within
    /// [`SHADOW_PROBE_DEPTH`] of its feet.
    ///
    /// The gather owns the whole decision because only it has the world: the
    /// ground probe, the footprint-scaled radius, and the drop fade are all
    /// resolved here — the renderer stamps quads and nothing else. Rows cull
    /// against the view volume first so a full scene pays probes only for
    /// entities that will draw (the gather-scales-with-VISIBLE rule).
    fn collect_entity_shadows(&mut self, game: &Game, view: &ViewVolume, tick_alpha: f32) {
        self.shadows.clear();
        let world = &game.replica.world;
        // A generous box around the feet — the decal is at most a couple of
        // blocks across and sits below the body, never above it.
        let visible = |feet: petramond_math::world_pos::WorldPos| {
            view.aabb_visible(
                feet - Vec3::new(2.0, 1.0, 2.0),
                feet + Vec3::new(2.0, 2.0, 2.0),
            )
        };
        for m in &self.mobs {
            let feet = m.prev_pos.lerp(m.pos, tick_alpha);
            if !visible(feet) {
                continue;
            }
            let size = petramond::mob::def(m.kind).size;
            let half = size
                .half_length
                .unwrap_or(size.half_width)
                .max(size.half_width);
            push_entity_shadow(
                world,
                &mut self.shadows,
                feet,
                half * MOB_SHADOW_RADIUS_SCALE,
            );
        }
        for d in &self.item_entities {
            // An item in flight or lodged in a wall casts nothing: a blob
            // under a sliver reads as a second object, not a shadow.
            if d.flight.is_some() {
                continue;
            }
            let pos = d.prev_pos.lerp(d.pos, tick_alpha);
            if !visible(pos) {
                continue;
            }
            push_entity_shadow(world, &mut self.shadows, pos, ITEM_SHADOW_RADIUS);
        }
        let bodies = &self.animation.frame;
        let feet = bodies.local.iter().chain(bodies.remotes.iter().map(|r| &r.body));
        for body in feet {
            push_entity_shadow(world, &mut self.shadows, body.pos, PLAYER_SHADOW_RADIUS);
        }
    }
}

/// How far a SEATED body's head may swivel off the seat facing (radians).
/// The body itself sits square in the seat (its yaw is the mount's), so the
/// look must not drag it around — and an unclamped relative head yaw would
/// owl the neck when the rider looks backward.
const SEATED_HEAD_YAW_LIMIT: f32 = 1.2;

/// Resolve one entity's blob shadow into `out`: probe straight down from the
/// feet for the first collision top within [`SHADOW_PROBE_DEPTH`], then fade
/// the strength quadratically with the drop and widen the radius as the body
/// rises — a falling body's shadow spreads and pales before vanishing. An
/// entity with no reachable ground (falling past a cliff, over an unloaded
/// column) casts nothing.
fn push_entity_shadow(
    world: &petramond_world::world::WorldData,
    out: &mut Vec<EntityShadow>,
    feet: petramond_math::world_pos::WorldPos,
    radius: f32,
) {
    let x = feet.x.floor() as i32;
    let z = feet.z.floor() as i32;
    // Feet rest a hair under a surface as often as on it (a landing settles
    // at 69.99999; a step-up eases the drawn body through the ledge), so the
    // probe starts in the cell a slightly sunk body stands ON, not the one
    // `floor` of its feet names — that skipped the ground itself and stamped
    // the decal on the block beneath it, inside the terrain.
    let reach = feet.y + f64::from(SHADOW_SINK);
    let y0 = reach.floor() as i32;
    let (fx, fz) = (
        (feet.x - f64::from(x)) as f32,
        (feet.z - f64::from(z)) as f32,
    );
    for dy in 0..=SHADOW_PROBE_DEPTH {
        let y = y0 - dy;
        // The highest surface UNDER the feet: boxes beside them in the same
        // cell (a stair's upper step, a fence post) and ones above them are
        // not ground.
        let top = world
            .collision_boxes_at(x, y, z)
            .iter()
            .filter(|b| b.min[0] <= fx && fx <= b.max[0] && b.min[2] <= fz && fz <= b.max[2])
            .map(|b| b.max[1])
            .filter(|top| f64::from(y) + f64::from(*top) <= reach)
            .fold(f32::MIN, f32::max);
        if top == f32::MIN {
            continue;
        }
        let ground = y as f32 + top;
        let t = ((feet.y - f64::from(ground)) as f32 / SHADOW_MAX_DROP).clamp(0.0, 1.0);
        out.push(EntityShadow {
            center: petramond_math::world_pos::WorldPos::new(feet.x, f64::from(ground), feet.z),
            radius: radius * (1.0 + 0.5 * t),
            strength: SHADOW_STRENGTH * (1.0 - t * t),
        });
        return;
    }
}

/// Whether a wire mount renders the SEATED body pose: every mob seat, and a
/// pose anchor holding the `sitting` pose. An anchor pose outside the known
/// vocabulary renders the rest pose — like a disabled pack, never an error.
fn mount_renders_seated(mount: petramond::net::protocol::PlayerMount) -> bool {
    match mount {
        petramond::net::protocol::PlayerMount::Mob { .. } => true,
        petramond::net::protocol::PlayerMount::Anchor { pose, .. } => {
            pose == mod_api::pose::SITTING
        }
    }
}

/// The local body's facing as presented: square in its seat when mounted,
/// else the third-person follow pose.
fn local_body_yaw(game: &Game) -> f32 {
    game.replica.self_mount_pose()
        .map_or(game.local.third_person.pose.body_yaw, |mount| mount.body_yaw)
}

/// The local third-person body, its bone offsets appended to `frame`'s
/// arena; `None` in first person. `emitters` is the body's refreshed set.
fn collect_player(
    game: &Game,
    emitters: &BodyEmitters,
    frame: &mut BodyFrame,
) -> Option<BodyInput> {
    // The body draws only once the boom camera is actually placed — never on a
    // frame whose render camera is still the first-person eye (inside the head).
    if !game.third_person_enabled() || game.local.third_person.cam.is_none() {
        return None;
    }
    let (skylight, blocklight) = game.held_item_light();
    // The body shares the first-person camera's auto-step vertical easing (a
    // negative, settling lag) so stepping up a ledge glides instead of popping.
    let mut pos = game.local.player.pos + Vec3::new(0.0, game.local.camera_rig.step_y_offset(), 0.0);
    // Sleep state reads the replicated self view (the sim's SleepState stays
    // server-side).
    let sleeping = game.replica.self_view.sleeping.is_some();
    if sleeping {
        // The sleeper stands at the bed-group CENTRE; the lying model's feet
        // anchor shifts back toward the foot end so the head lands on the pillow
        // (bed length 2, model ~1.85 → feet ~0.925 behind centre).
        let head_yaw = game.local.third_person.pose.body_yaw;
        pos -= Vec3::new(head_yaw.sin(), 0.0, head_yaw.cos()) * 0.925;
    }
    // Seated: the body sits SQUARE in the seat and leans with it — its yaw
    // is the mount's facing, never the look-follow (which would spin the
    // whole body, legs through the hull); only the head follows the look,
    // clamped.
    let seated = game.replica.entities.own_mount().is_some_and(mount_renders_seated);
    let mount = game.replica.self_mount_pose();
    let body_yaw = local_body_yaw(game);
    let head_yaw = match mount {
        Some(_) => petramond_math::math::wrap_angle(game.local.player.yaw - body_yaw)
            .clamp(-SEATED_HEAD_YAW_LIMIT, SEATED_HEAD_YAW_LIMIT),
        None => game.local.player.yaw - body_yaw,
    };
    Some(BodyInput {
        emitter_tint: emitters.tint(),
        emitter_self_lit: emitters.self_lit(),
        pos,
        skylight,
        blocklight,
        state: BodyState {
            body_yaw,
            head_yaw,
            head_pitch: game.local.player.pitch,
            anim_time: game.local.third_person.pose.anim_time,
            seated,
            seat_tilt: mount.map_or(Tilt::LEVEL, |m| m.tilt),
            walk_weight: game.local.third_person.pose.walk_weight,
            sneak_weight: game.local.third_person.pose.sneak_weight,
            locomotion: game.local.third_person.pose.locomotion,
            sleeping,
            // The hurt flash is the app's envelope, handed to the animation
            // with the local frame (`LocalInput::hurt_flash`).
            hurt: 0.0,
        },
        // The LOCAL body's eased offsets (advanced in `tick_send`): a client
        // mod posing bones owns them here for the same reason it owns a held
        // pose — the release has to present now, not a round trip later.
        bones: frame.push_bones(game.fx.local_bones()),
    })
}

/// The crack overlay for a miner's `(block, stage)` — the target + stage come
/// from replicated state (the own `SelfState::mining` or a remote row's); the
/// shape details are derived from the REPLICA world at that cell. `resolved`
/// is the caller's reused scratch for the cell's shape boxes.
fn break_overlay_at(
    game: &Game,
    block: IVec3,
    stage: u8,
    resolved: &mut Vec<petramond_world::block::ShapeBox>,
) -> BreakOverlayView {
    // A model block's crack is a decal over the model's own drawn triangles, so
    // the view carries only the outline box the decal is masked to — from the
    // one producer that answers for a placed model's world extent.
    let model = game
        .replica.world
        .model_outline_box(block)
        .map(|(base, min, max)| ModelCrack { base, min, max });
    // The ONE box producer answers for every box family at once; nothing here
    // asks which family it is.
    game.replica.world.shape_draw_boxes(block, resolved);
    let shape_boxes = (!resolved.is_empty()).then(|| {
        let mut boxes = [CrackBox {
            min: [0.0; 3],
            max: [0.0; 3],
            faces: [false; 6],
            pose: None,
        }; MAX_CRACK_BOXES];
        let len = resolved.len().min(MAX_CRACK_BOXES);
        for (dst, b) in boxes.iter_mut().zip(resolved.iter()).take(len) {
            *dst = CrackBox {
                min: b.aabb.min,
                max: b.aabb.max,
                faces: std::array::from_fn(|fi| b.faces[fi].is_some()),
                pose: b.pose,
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
            game.replica.world.data().selection_box_at(block.x, block.y, block.z)
        },
        shape_boxes,
        model,
        stage,
    }
}
