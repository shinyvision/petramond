//! Per-frame view-state setters + terrain sync for [`Renderer`].
//!
//! Cheap mutators the app calls each frame to hand the renderer the camera
//! uniforms, selection/break overlay, held item, world instance lists, UI
//! snapshot, and the terrain mesh sync (scheduled by [`UploadQueue`]).

use super::upload_queue::{FrameDrain, UploadPriority};
use super::*;

/// Soft render-thread budget for packing/writing terrain columns. One upload is always
/// allowed so terrain keeps making progress; after that, leave time for the actual frame.
/// (The upload queue's count cap is the backstop behind it.)
const MESH_COLUMN_UPLOAD_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(1_750);
const RENDER_ORIGIN_GRID: i32 = 16;

/// Tilt of the sun/moon arc out of the east–west vertical plane. Mirror of
/// `ARC_TILT` in `assets/shaders/daynight_sky.wgsl` — keep in sync, or the
/// terrain haze's sun-glow drifts off the drawn sun sprite.
const SUN_ARC_TILT: f32 = 0.15;

/// The atmosphere's sun lane: unit sun direction (xyz) + daylight (w), derived
/// from the engine-owned `petramond:time` shader param (`[fraction, daylight,
/// moon_phase, 0]`) with the same arc formula as
/// `daynight_sky.wgsl`. Without a day/night cycle the sun holds late morning at
/// full daylight.
pub(super) fn sun_uniform(
    shader_params: Option<&petramond::world::environment::ShaderParamMap>,
) -> [f32; 4] {
    let (fraction, daylight) = shader_params
        .and_then(|params| params.get("petramond:time"))
        .map(|time| (time[0].fract(), time[1].clamp(0.0, 1.0)))
        .unwrap_or((0.25, 1.0));
    let angle = std::f32::consts::TAU * fraction;
    let dir = glam::Vec3::new(angle.cos(), angle.sin(), SUN_ARC_TILT).normalize();
    [dir.x, dir.y, dir.z, daylight]
}

/// Fill a 16-slot params block from a shader's declared key list.
fn fill_shader_params(
    keys: &[String],
    shader_params: Option<&petramond::world::environment::ShaderParamMap>,
) -> super::super::uniforms::ShaderParams {
    let mut values = [[0.0f32; 4]; super::super::uniforms::SHADER_PARAM_SLOTS];
    if let Some(shader_params) = shader_params {
        for (i, key) in keys.iter().enumerate() {
            if i >= values.len() {
                break;
            }
            if let Some(value) = shader_params.get(key) {
                values[i] = *value;
            }
        }
    }
    super::super::uniforms::ShaderParams { values }
}

#[inline]
fn render_origin_for_camera(pos: petramond_math::world_pos::WorldPos) -> glam::IVec3 {
    let cell = pos.block();
    let snap = |v: i32| v.div_euclid(RENDER_ORIGIN_GRID) * RENDER_ORIGIN_GRID;
    glam::IVec3::new(snap(cell.x), snap(cell.y), snap(cell.z))
}

/// `view_offset` is a view-space correction applied after the look (the
/// first-person camera bone), identity otherwise.
#[inline]
fn relative_view_proj(
    cam: &Camera,
    render_origin: glam::IVec3,
    view_offset: glam::Mat4,
) -> glam::Mat4 {
    let local_pos = cam.pos.relative_to(render_origin);
    cam.proj()
        * view_offset
        * glam::Mat4::look_at_rh(local_pos, local_pos + cam.forward(), glam::Vec3::Y)
}

impl Renderer {
    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        eye_fluid: Option<petramond_world::block::Block>,
        shader_params: Option<&petramond::world::environment::ShaderParamMap>,
    ) {
        let eye_medium = eye_fluid
            .and_then(petramond_world::block::Block::fluid_def)
            .map(|def| &def.medium);
        let render_origin = render_origin_for_camera(cam.pos);
        let local_cam = cam.pos.relative_to(render_origin);
        let view_offset = match &self.hand.first_person {
            Some(first_person) if self.hand.visible && self.hand.screen_shake => {
                first_person.view_offset()
            }
            _ => glam::Mat4::IDENTITY,
        };
        let view_proj = relative_view_proj(cam, render_origin, view_offset);
        let inv_view_proj = view_proj.inverse();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.view.frustum = Frustum::from_view_proj(view_proj);
        self.view.proj_y_scale = cam.proj().y_axis.y;
        self.view.cam_pos = cam.pos;
        self.view.render_origin = render_origin;
        self.view.visual_time = time;
        self.update_shader_params(shader_params);
        let mut effective_sky_scale = 1.0;
        let mut effective_sky_color = [1.0, 1.0, 1.0];
        let mut shader_light_overrode_identity = false;
        if let (Some(params), Some(key)) = (shader_params, self.sky.light_param_key.as_deref()) {
            if let Some(value) = params.get(key) {
                effective_sky_scale = value[0].clamp(0.0, 1.0);
                effective_sky_color = [
                    value[1].clamp(0.0, 1.0),
                    value[2].clamp(0.0, 1.0),
                    value[3].clamp(0.0, 1.0),
                ];
                shader_light_overrode_identity = true;
            }
        }
        let effective_fog_color = if shader_light_overrode_identity && eye_medium.is_none() {
            [
                fog_color[0] * effective_sky_scale * effective_sky_color[0],
                fog_color[1] * effective_sky_scale * effective_sky_color[1],
                fog_color[2] * effective_sky_scale * effective_sky_color[2],
            ]
        } else {
            fog_color
        };
        self.sky.clear_color = effective_fog_color;
        self.sky.scale = effective_sky_scale;
        self.sky.color = effective_sky_color;
        let (fog_start, fog_end, in_fluid, volume_tint) = match eye_medium {
            Some(m) => (m.fog_start, m.fog_end, 1.0, m.volume_tint),
            None => (self.sky.fog_start, self.sky.fog_end, 0.0, [1.0; 3]),
        };
        self.terrain.view_key = TerrainViewKey {
            view_proj: view_proj.to_cols_array().map(f32::to_bits),
            cam: cam.pos.to_array().map(f64::to_bits),
            fog: self.terrain_cull_dist().to_bits(),
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [local_cam.x, local_cam.y, local_cam.z, 0.0],
            fog: [fog_start, fog_end, time, in_fluid],
            // fog_color.w = the sim's sky scale (1.0 = identity/noon).
            fog_color: [
                effective_fog_color[0],
                effective_fog_color[1],
                effective_fog_color[2],
                effective_sky_scale,
            ],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            render_origin: [render_origin.x, render_origin.y, render_origin.z, 0],
            atlas_layout: crate::atlas::atlas_layout_uniform(),
            sky_color: [
                effective_sky_color[0],
                effective_sky_color[1],
                effective_sky_color[2],
                0.0,
            ],
            sun_dir: sun_uniform(shader_params),
            volume_tint: [volume_tint[0], volume_tint[1], volume_tint[2], 0.0],
        };
        self.ghosts.camera(&self.queue, &u);
        self.selection.camera(&self.queue, &u);
        self.queue
            .write_buffer(&self.binds.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    fn update_shader_params(
        &mut self,
        shader_params: Option<&petramond::world::environment::ShaderParamMap>,
    ) {
        self.queue.write_buffer(
            &self.binds.shader_params_buf,
            0,
            bytemuck::cast_slice(&[fill_shader_params(
                &self.sky.shader_param_keys,
                shader_params,
            )]),
        );
        // Each environment pass declares its own key list over its own
        // buffer. A pass whose declared params are ALL absent goes dormant
        // (skipped in encode) — the title screen and servers without the
        // owning mod pay nothing for it.
        for pass in &mut self.sky.env_passes {
            let any_present = shader_params.is_some_and(|params| {
                pass.res
                    .param_keys
                    .iter()
                    .any(|key| params.contains_key(key))
            });
            pass.dormant = !pass.res.param_keys.is_empty() && !any_present;
            if pass.dormant {
                continue;
            }
            self.queue.write_buffer(
                &pass.res.params_buf,
                0,
                bytemuck::cast_slice(&[fill_shader_params(&pass.res.param_keys, shader_params)]),
            );
        }
    }

    /// Ease the post mood toward the mods' combined target and upload it for
    /// the grade pass. `[0, 0]` = the untouched image; the ease (~2 s) makes
    /// weather moods breathe in and out instead of popping.
    pub fn set_mood(&mut self, target: [f32; 2], dt: f32) {
        const MOOD_EASE_SECONDS: f32 = 2.0;
        let target = target.map(|v| v.clamp(0.0, 0.5));
        if self.targets.mood == target {
            return;
        }
        let previous = self.targets.mood;
        let ease = 1.0 - (-dt.clamp(0.0, 0.25) / MOOD_EASE_SECONDS).exp();
        self.targets.mood[0] += (target[0] - self.targets.mood[0]) * ease;
        self.targets.mood[1] += (target[1] - self.targets.mood[1]) * ease;
        if self.targets.mood != previous {
            self.upload_post_process();
        }
    }

    /// Set (or clear) the target highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, shape: Option<SelectionShape>) {
        self.chrome.selection = shape;
    }

    /// Store the block-break overlays to draw this frame (empty clears). A
    /// small bounded slice — the local miner's own crack plus the capped
    /// nearest remotes; each bakes exactly like the single overlay always did.
    pub fn set_break_overlays(&mut self, v: &[BreakOverlayView]) {
        self.hand.break_overlays.clear();
        self.hand.break_overlays.extend_from_slice(v);
    }

    /// Store the local player's frame, as the client's animation built it:
    /// each hand's eased held view for the seats and attaches (an empty
    /// off-hand view draws nothing in the left hand), and the viewmodel's
    /// posed bones, whose camera bone [`update_uniforms`](Self::update_uniforms)
    /// applies to the world view.
    pub fn set_local_frame(&mut self, frame: LocalFrame<'_>) {
        let hand = &mut self.hand;
        [hand.held_item, hand.off_item] = frame.held;
        if let Some(first_person) = &mut hand.first_person {
            first_person.set_bones(frame.first_person);
        }
    }

    pub fn set_hand_visible(&mut self, visible: bool) {
        self.hand.visible = visible;
    }

    /// Store this frame's hurt-shake screen offset for the hand/held item, in
    /// NDC units (tiny values — the shake is subtle).
    pub fn set_hand_shake(&mut self, shake: [f32; 2]) {
        self.hand.shake = shake;
    }

    pub fn set_crosshair_visible(&mut self, visible: bool) {
        self.chrome.crosshair_visible = visible;
    }

    /// Store the two-channel light to apply to the first-person hand / held item
    /// (so it brightens AND takes the colour of nearby block light, and block
    /// light keeps it lit at night).
    pub fn set_held_item_light(
        &mut self,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) {
        self.hand.held_item_skylight = skylight.min(crate::lighting::FULL_SKYLIGHT);
        self.hand.held_item_blocklight = blocklight;
    }

    // The per-frame row lists below are handed over by SWAP, not copied: the
    // caller's buffer becomes this frame's rows and last frame's come back
    // for the caller to clear and refill, so a row is written once by the
    // scene bake and never copied again on its way to the GPU.

    /// This frame's mod draw sets. They ride the ITEM-ENTITY opaque stream:
    /// same block atlas, same double-sided CPU-lit pipeline, and no chunk
    /// re-mesh — which is the whole reason a mod may submit a new set every
    /// tick.
    pub fn swap_block_draws(&mut self, v: &mut Vec<crate::BlockDrawInstance>) {
        std::mem::swap(&mut self.item_entity.block_draws, v);
    }

    /// Take the dropped item-entities to draw this frame.
    pub fn swap_item_entities(&mut self, v: &mut Vec<ItemEntityInstance>) {
        std::mem::swap(&mut self.item_entity.instances, v);
    }

    /// Take the animated blocks to draw this frame.
    pub(crate) fn swap_block_entities(&mut self, v: &mut Vec<BlockEntityInstance>) {
        std::mem::swap(&mut self.block_entity.instances, v);
    }

    /// Take the mobs to draw this frame (already interpolated by the scene
    /// adapter) with the arena their ranges address, and adopt the session's
    /// animation-name table their layer ids index — a pointer compare unless
    /// the table grew.
    pub fn swap_mobs(
        &mut self,
        mobs: &mut Vec<MobRenderInstance>,
        arena: &mut crate::MobArena,
        names: &crate::AnimNames,
    ) {
        std::mem::swap(&mut self.actor.mobs, mobs);
        std::mem::swap(&mut self.actor.mob_arena, arena);
        self.actor.anim_names.adopt(names);
    }

    /// Take the player bodies to draw this frame — the local third-person
    /// body and every remote, each already posed by the client's animation —
    /// with the pose arena their `PlayerRenderInstance::pose` ranges index
    /// into.
    pub fn swap_player_bodies(
        &mut self,
        bodies: &mut Vec<PlayerBodyRender>,
        poses: &mut Vec<glam::Mat4>,
    ) {
        std::mem::swap(&mut self.actor.bodies, bodies);
        std::mem::swap(&mut self.actor.body_poses, poses);
    }

    /// Take the block-atlas particle cubes to draw this frame.
    pub fn swap_particles(&mut self, v: &mut Vec<ParticleInstance>) {
        std::mem::swap(&mut self.particle.instances, v);
    }

    /// Take the model-atlas particle cubes (bbmodel-block flecks) for this frame; they
    /// bake into the same particle vbuf after the block cubes and draw with the model
    /// atlas bound.
    pub fn swap_model_particles(&mut self, v: &mut Vec<ParticleInstance>) {
        std::mem::swap(&mut self.particle.model_instances, v);
    }

    /// Take the visible particle emitters for this frame. The renderer derives
    /// transient translucent cubes from these in `bake_world_instances`.
    pub fn swap_particle_emitters(&mut self, v: &mut Vec<ParticleEmitterInstance>) {
        std::mem::swap(&mut self.particle.emitters, v);
    }

    /// Take the solid-color simulated particles (emitter-burst droplets) for
    /// this frame; they join the emitter cubes' alpha-blended bake.
    pub fn swap_solid_particles(&mut self, v: &mut Vec<SolidParticleInstance>) {
        std::mem::swap(&mut self.particle.solid_instances, v);
    }

    /// Take this frame's entity blob-shadow rows (ground-resolved by the
    /// gather).
    pub fn swap_shadows(&mut self, v: &mut Vec<EntityShadow>) {
        std::mem::swap(&mut self.shadow.instances, v);
    }

    pub fn clear_world_state(&mut self) {
        self.ghosts.clear_world();
        self.selection.clear_world();
        self.terrain.clear_world();
        self.chrome.clear_world();
        // Each pass drops its own world-scoped state, so leaving a world
        // cannot forget one the way the hand-written reset did (it had lost
        // the solid particles, the held item, and its animator).
        self.hand.clear_world();
        self.particle.clear_world();
        self.item_entity.clear_world();
        self.block_entity.clear_world();
        self.actor.clear_world();
        self.shadow.clear_world();
    }

    /// True while terrain columns are still queued for GPU upload. Uploads are
    /// spread over frames to protect frame time, so a caller that must draw the
    /// COMPLETE terrain in one shot pumps [`Renderer::sync_meshes`] until this
    /// clears.
    pub fn terrain_uploads_pending(&self) -> bool {
        !self.terrain.uploads.is_empty()
    }

    /// Synchronize GPU meshes with the terrain CPU meshes: drop columns whose
    /// meshes are gone, queue the dirty ones, and upload what the upload
    /// queue releases this frame within the time budget.
    pub fn sync_meshes(&mut self, terrain: &mut TerrainRenderHandoff<'_>) {
        self.terrain.uploads.begin_frame();
        // Drop packed GPU columns whose CPU meshes are gone.
        let before_columns = self.terrain.columns.len();
        self.terrain.columns.retain(|p| terrain.has_column_mesh(p));
        if self.terrain.columns.len() != before_columns {
            self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
        }

        let frustum = self.view.frustum;
        let render_origin = self.view.render_origin;
        // Render-local, like the frustum.
        let cam = self.view.cam_pos.relative_to(render_origin);
        let fog = self.terrain_cull_dist();
        // Columns about to be in view first, then nearest first.
        let priority = |column: ChunkPos| -> UploadPriority {
            let (lo_y, hi_y) = (
                petramond_world::chunk::WORLD_MIN_Y,
                petramond_world::chunk::WORLD_MAX_Y,
            );
            let corner = glam::IVec3::new(column.cx * 16, lo_y, column.cz * 16) - render_origin;
            let min = corner.as_vec3();
            let max = min + glam::Vec3::new(16.0, (hi_y - lo_y) as f32, 16.0);
            let visible_soon =
                frustum.aabb_visible(min, max) && aabb_distance_sq(cam, min, max) <= fog * fog;
            let center = glam::Vec3::new(min.x + 8.0, cam.y, min.z + 8.0);
            (
                u8::from(!visible_soon),
                (cam - center).length_squared().to_bits(),
            )
        };
        let uploads = &mut self.terrain.uploads;
        terrain.for_dirty_columns(&mut |column, revision| {
            uploads.mark_dirty(column, revision, || priority(column));
        });
        // Columns a synchronous click presentation installed into skip the
        // quiet gate: the player is pointing at them.
        for column in terrain.take_urgent_columns() {
            uploads.mark_urgent(column, || priority(column));
        }

        let device = &self.device;
        let queue = &self.queue;
        let TerrainPass {
            columns,
            column_origins: origins,
            geometry: arenas,
            quad_index,
            uploads,
            gpu_revision,
            ..
        } = &mut self.terrain;
        let start = std::time::Instant::now();
        let mut upload_batch = crate::resources::TerrainUploadBatch::default();
        let mut drain = FrameDrain::default();
        loop {
            if drain.uploads() > 0 && start.elapsed() >= MESH_COLUMN_UPLOAD_TIME_BUDGET {
                break;
            }
            let Some((column, revision)) = uploads.next_ready(&mut drain) else {
                break;
            };
            uploads.take(column);
            if !terrain.has_column_mesh(column) {
                let removed = columns.remove(&column).is_some();
                terrain.mark_column_uploaded(column);
                if removed {
                    *gpu_revision = gpu_revision.wrapping_add(1);
                }
                continue;
            }
            // A recreated renderer may lack the GPU copy of a released sibling.
            // Keep drawing the old column while that exceptional remesh completes.
            let reusable = columns.get(&column).is_some_and(|gpu| {
                terrain.column_meshes(column).iter().all(|(sp, mesh)| {
                    !mesh.is_released() || gpu.sections.iter().any(|(old, _)| old == sp)
                })
            });
            if !reusable && terrain.needs_repack_remeshes(column) {
                uploads.restart(column, revision, &mut drain);
                continue;
            }
            let meshes = terrain.column_meshes(column);
            if meshes.is_empty() {
                let removed = columns.remove(&column).is_some();
                terrain.mark_column_uploaded(column);
                if removed {
                    *gpu_revision = gpu_revision.wrapping_add(1);
                }
                continue;
            }
            let prev = columns.remove(&column);
            let gpu = upload_column_mesh(
                device,
                queue,
                &meshes,
                prev,
                origins,
                arenas,
                quad_index,
                &mut upload_batch,
            );
            columns.insert(column, gpu);
            drop(meshes);
            terrain.mark_column_uploaded(column);
            drain.record_upload();
            *gpu_revision = gpu_revision.wrapping_add(1);
        }
        upload_batch.submit(queue);
        uploads.finish(drain, priority);
    }
}
