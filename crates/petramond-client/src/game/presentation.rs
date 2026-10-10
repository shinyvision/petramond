use glam::{IVec3, Vec3};

use petramond::mob::Mob;
use petramond::world::PlacedEmitter;
use petramond_math::math::Tilt;
use petramond_render::camera::ViewVolume;
use petramond_render::{AnimLayer, ArenaRange, GaitFade, MobArena};
use petramond_world::block::Block;

use super::remote_players;
use super::Game;
use crate::animation::{
    BodyFrame, BodyInput, BodyState, FootstepSource, PlayerAnimation, RemoteBody,
};

mod entity_emitters;
#[cfg(test)]
mod tests;
pub use entity_emitters::BodyEmitters;

pub use petramond_render::views::{
    BlockEntityPresentation, BreakOverlayView, CrackBox, CrackBoxes, DroppedItemPresentation,
    EntityShadow, GamePresentation, MobPresentation, ModelCrack, ParticleAtlas,
    ParticlePresentation, MAX_CRACK_BOXES,
};

pub struct FramePresentation<'a> {
    pub render: GamePresentation<'a>,
    pub footsteps: &'a [FootstepSource],
}

impl<'a> std::ops::Deref for FramePresentation<'a> {
    type Target = GamePresentation<'a>;

    fn deref(&self) -> &Self::Target {
        &self.render
    }
}

const LOCAL_FOOTSTEP_ID: u64 = 0;
const MOB_FOOTSTEP_IDS: u64 = 1 << 62;
const MOB_STEP_RISE: f64 = 0.2;

const MIN_FOOTSTEP_SPEED: f32 = 0.5;

const SPRINT_FOOTSTEP_SPEED: f32 = 4.95;

const MIN_FOOTSTEP_WALK_WEIGHT: f32 = 0.35;

fn lit_cell(world: &petramond::world::ReplicaWorld, cell: IVec3) -> IVec3 {
    const RISE: i32 = 4;
    (0..=RISE)
        .map(|dy| cell + IVec3::new(0, dy, 0))
        .find(|c| {
            world
                .data()
                .block_if_loaded(c.x, c.y, c.z)
                .is_none_or(|block| !block.is_opaque())
        })
        .unwrap_or(cell)
}

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

const SHADOW_PROBE_DEPTH: i32 = 6;
const SHADOW_SINK: f32 = 0.6;
const SHADOW_MAX_DROP: f32 = 4.0;
/// No flag is posed farther than this, whatever the view distance.
const FAR_CLOTH_LIMIT: f32 = 512.0;

/// The coat the cloth owned by `cell` wears this frame, read from the cell's own data.
/// A painted coat's per-texel dyes go into the frame's shared `texels` list.
fn cloth_coat(
    data: &petramond_world::world::WorldData,
    cell: IVec3,
    texels: &mut Vec<Option<[f32; 3]>>,
) -> petramond_render::views::ClothCoat {
    use petramond_render::views::ClothCoat;
    use petramond_world::paint::{Coat, GRID};
    let coat = Coat::read(|key| data.cell_kv_get(cell.x, cell.y, cell.z, key));
    match (coat.paint, coat.tint) {
        (Some(_), _) => {
            let first = texels.len() as u32;
            texels.extend((0..GRID).flat_map(|ty| (0..GRID).map(move |tx| coat.texel(tx, ty))));
            ClothCoat::Painted { first }
        }
        (None, Some(tint)) => ClothCoat::Tint(tint),
        (None, None) => ClothCoat::None,
    }
}

const SHADOW_STRENGTH: f32 = 0.42;
const MOB_SHADOW_RADIUS_SCALE: f32 = 1.5;
const PLAYER_SHADOW_RADIUS: f32 = 0.45;
const ITEM_SHADOW_RADIUS: f32 = 0.25;

#[derive(Default)]
pub struct GamePresentationScratch {
    pub ambient: super::ambient::AmbientDrives,
    item_entities: Vec<DroppedItemPresentation>,
    particles: Vec<ParticlePresentation>,
    particle_emitters: Vec<PlacedEmitter>,
    animated_rows: Vec<petramond::world::animated_block::AnimatedBlock>,
    block_draws: Vec<petramond::world::draw::BlockDrawInstance>,
    cloths: Vec<petramond_render::views::ClothPresentation>,
    cloth_points: Vec<petramond_render::views::ClothPoint>,
    cloth_texels: Vec<Option<[f32; 3]>>,
    far_cloths: Vec<petramond::world::PlacedCloth>,
    far_pose: Vec<Vec3>,
    block_entities: Vec<BlockEntityPresentation>,
    mobs: Vec<MobPresentation>,
    mob_arena: MobArena,
    local_emitters: BodyEmitters,
    overlay_boxes: Vec<petramond_world::block::ShapeBox>,
    pub animation: PlayerAnimation,
    shadows: Vec<EntityShadow>,
    footsteps: Vec<FootstepSource>,
    break_overlays: Vec<BreakOverlayView>,
}

impl GamePresentationScratch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot<'a>(
        &'a mut self,
        game: &'a Game,
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
        self.collect_cloths(game, view);
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
                cloths: &self.cloths,
                cloth_points: &self.cloth_points,
                cloth_texels: &self.cloth_texels,
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
        let world = &game.replica.world;
        self.item_entities
            .extend(game.replica.entities.items().iter().map(|entry| {
                let c = entry.curr.pos.block();
                DroppedItemPresentation {
                    prev_pos: entry.prev.pos,
                    pos: entry.curr.pos,
                    item: petramond_world::item::ItemType(entry.curr.item_id),
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

    fn collect_ambient(&mut self, game: &Game, now: f32) {
        for (mod_id, bundle, intensity, wind) in game.client_mods.ambient_targets() {
            self.ambient.set(mod_id, bundle, intensity, wind);
        }
        self.ambient.collect(
            &game.replica.world,
            game.render_camera().pos,
            now,
            game.fx.particles.count_scale(),
            &mut self.particles,
        );
    }

    fn collect_particle_emitters(&mut self, game: &Game, view: &ViewVolume) {
        if game.fx.particles.count_scale() <= 0.0 {
            self.particle_emitters.clear();
            return;
        }
        game.replica
            .world
            .collect_particle_emitters(view, &mut self.particle_emitters);
    }

    fn collect_cloths(&mut self, game: &Game, view: &ViewVolume) {
        self.cloths.clear();
        self.cloth_points.clear();
        self.cloth_texels.clear();
        let data = game.replica.world.data();
        let cloth = &game.fx.cloth;
        let alpha = cloth.alpha();
        let (lo, hi) = super::cloth::ClothSim::reach();
        for sim in cloth.sims() {
            let min = petramond_math::world_pos::WorldPos::block_min(sim.cell);
            if !view.aabb_visible(min + lo, min + hi) {
                continue;
            }
            let def = sim.def;
            self.cloths
                .push(petramond_render::views::ClothPresentation {
                    cell: sim.cell,
                    tile: def.tile,
                    uv: def.uv,
                    cols: u16::from(def.segments[0]) + 1,
                    rows: u16::from(def.segments[1]) + 1,
                    first: self.cloth_points.len() as u32,
                    coat: cloth_coat(data, sim.cell, &mut self.cloth_texels),
                });
            self.cloth_points.extend(sim.points(alpha).map(|pos| {
                let c = sim.cell + petramond_math::math::voxel_at(pos);
                let (skylight, blocklight) = data.dynamic_light_at_world(c.x, c.y, c.z);
                petramond_render::views::ClothPoint {
                    pos,
                    skylight,
                    blocklight,
                }
            }));
        }

        // Beyond simulation range a flag is still drawn: posed from the wind on a
        // coarser grid the farther it is.
        let eye = view.eye();
        let radius = view.cull_distance().min(FAR_CLOTH_LIMIT) as i32;
        game.replica
            .world
            .collect_cloths(eye.block(), radius, &mut self.far_cloths);
        let (time, wind) = cloth.far_clock();
        for placed in &self.far_cloths {
            if cloth.simulates(placed.cell) {
                continue;
            }
            let Some(def) = petramond_world::cloth::def(placed.cloth) else {
                continue;
            };
            let min = petramond_math::world_pos::WorldPos::block_min(placed.cell);
            if !view.aabb_visible(min + lo, min + hi) {
                continue;
            }
            let distance = (min - eye).length();
            let segments = super::cloth::ClothSystem::far_segments(def, distance);
            super::cloth::far_pose(def, placed.cell, wind, time, segments, &mut self.far_pose);
            self.cloths
                .push(petramond_render::views::ClothPresentation {
                    cell: placed.cell,
                    tile: def.tile,
                    uv: def.uv,
                    cols: segments[0] as u16 + 1,
                    rows: segments[1] as u16 + 1,
                    first: self.cloth_points.len() as u32,
                    coat: cloth_coat(data, placed.cell, &mut self.cloth_texels),
                });
            self.cloth_points.extend(self.far_pose.iter().map(|&pos| {
                let c = placed.cell + petramond_math::math::voxel_at(pos);
                let (skylight, blocklight) = data.dynamic_light_at_world(c.x, c.y, c.z);
                petramond_render::views::ClothPoint {
                    pos,
                    skylight,
                    blocklight,
                }
            }));
        }
    }

    fn collect_block_draws(&mut self, game: &Game, view: &ViewVolume) {
        game.replica
            .world
            .collect_block_draws(view, &mut self.block_draws);
    }

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

    fn collect_block_entities(&mut self, game: &Game) {
        game.replica
            .world
            .collect_animated_blocks(&mut self.animated_rows);
        self.block_entities.clear();
        self.block_entities
            .extend(
                self.animated_rows
                    .iter()
                    .map(|&block| BlockEntityPresentation {
                        block,
                        open_progress: game.fx.block_open_progress(block.pos, block.pose.open),
                    }),
            );
    }

    fn collect_mobs(&mut self, game: &Game, tick_alpha: f32) {
        self.mobs.clear();
        self.mob_arena.clear();
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
            let anims_start = arena.anims.len();
            arena
                .anims
                .extend(entry.anim_blend.iter().map(|&(anim, weight, held)| {
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
        let hidden = game.hidden_remote_body();
        for (id, p) in game.replica.entities.players().iter_with_ids() {
            if !p.curr.visible || hidden == Some(id) {
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
                pos -= Vec3::new(body_yaw.sin(), 0.0, body_yaw.cos()) * 0.925;
            }
            let c = (pos + Vec3::new(0.0, 0.9, 0.0)).block();
            let emitters = p.emitters();
            let frame = &mut self.animation.frame;
            let animator = frame.push_animator(&p.curr.animator, &p.plays, &p.events);
            let body = BodyInput {
                pos,
                emitter_tint: emitters.tint(),
                emitter_self_lit: emitters.self_lit(),
                skylight: world.data().skylight6_at_world(c.x, c.y, c.z),
                blocklight: petramond_world::light::BlockLight6::from_x2(
                    world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                ),
                state: BodyState {
                    body_yaw,
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

    fn collect_entity_shadows(&mut self, game: &Game, view: &ViewVolume, tick_alpha: f32) {
        self.shadows.clear();
        let world = &game.replica.world;
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
                world.data(),
                &mut self.shadows,
                feet,
                half * MOB_SHADOW_RADIUS_SCALE,
            );
        }
        for d in &self.item_entities {
            if d.flight.is_some() {
                continue;
            }
            let pos = d.prev_pos.lerp(d.pos, tick_alpha);
            if !visible(pos) {
                continue;
            }
            push_entity_shadow(world.data(), &mut self.shadows, pos, ITEM_SHADOW_RADIUS);
        }
        let bodies = &self.animation.frame;
        let feet = bodies
            .local
            .iter()
            .chain(bodies.remotes.iter().map(|r| &r.body));
        for body in feet {
            push_entity_shadow(
                world.data(),
                &mut self.shadows,
                body.pos,
                PLAYER_SHADOW_RADIUS,
            );
        }
    }
}

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

fn mount_renders_seated(mount: petramond::net::protocol::PlayerMount) -> bool {
    match mount {
        petramond::net::protocol::PlayerMount::Mob { .. } => true,
        petramond::net::protocol::PlayerMount::Anchor { pose, .. } => {
            pose == mod_api::pose::SITTING
        }
    }
}

fn local_body_yaw(game: &Game) -> f32 {
    game.replica
        .self_mount_pose()
        .map_or(game.local.third_person.pose.body_yaw, |mount| {
            mount.body_yaw
        })
}

fn collect_player(
    game: &Game,
    emitters: &BodyEmitters,
    frame: &mut BodyFrame,
) -> Option<BodyInput> {
    if !game.presents_local_body() {
        return None;
    }
    let (skylight, blocklight) = game.held_item_light();
    let mut pos =
        game.local.player.pos + Vec3::new(0.0, game.local.camera_rig.step_y_offset(), 0.0);
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
    let seated = game
        .replica
        .entities
        .own_mount()
        .is_some_and(mount_renders_seated);
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
            hurt: 0.0,
        },
        bones: frame.push_bones(game.fx.local_bones()),
    })
}

fn break_overlay_at(
    game: &Game,
    block: IVec3,
    stage: u8,
    resolved: &mut Vec<petramond_world::block::ShapeBox>,
) -> BreakOverlayView {
    let model = game
        .replica
        .world
        .data()
        .model_outline_box(block)
        .map(|(base, min, max)| ModelCrack { base, min, max });
    game.replica.world.data().shape_draw_boxes(block, resolved);
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
            game.replica
                .world
                .data()
                .selection_box_at(block.x, block.y, block.z)
        },
        shape_boxes,
        model,
        stage,
    }
}
