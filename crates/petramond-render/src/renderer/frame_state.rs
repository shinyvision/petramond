//! Per-frame view-state setters + terrain sync for [`Renderer`].
//!
//! Cheap mutators the app calls each frame to hand the renderer the camera
//! uniforms, selection/break overlay, held item, world instance lists, UI
//! snapshot, and the terrain mesh sync. Split out of the
//! renderer god-file; behavior is byte-for-byte identical.

use super::*;

/// Max terrain columns uploaded to the GPU per frame. CPU meshes stay section-owned, but
/// render-side buffers are packed per XZ column, so one upload can refresh many vertical
/// section ranges. Excess stays dirty and rolls onto later frames.
///
/// The TIME budget below is the real frame guard; this count is a backstop against a
/// burst of individually-cheap uploads. The old cap of 6 (~360 columns/s) admission-
/// limited fresh-terrain visibility during RD32 flight (~200 fresh columns/s plus 2–3
/// re-uploads each while filling) with most of the time budget unspent.
const MESH_COLUMN_UPLOADS_PER_FRAME: usize = 24;
/// Soft render-thread budget for packing/writing terrain columns. One upload is always
/// allowed so terrain keeps making progress; after that, leave time for the actual frame.
const MESH_COLUMN_UPLOAD_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(1_750);
const MESH_UPLOAD_QUIET_FRAMES: u64 = 1;
const MESH_UPLOAD_MAX_WAIT_FRAMES: u64 = 4;
const RENDER_ORIGIN_GRID: f32 = 16.0;

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
fn render_origin_for_camera(pos: glam::Vec3) -> glam::Vec3 {
    (pos / RENDER_ORIGIN_GRID).floor() * RENDER_ORIGIN_GRID
}

#[inline]
fn relative_view_proj(cam: &Camera, render_origin: glam::Vec3) -> glam::Mat4 {
    let local_pos = cam.pos - render_origin;
    cam.proj() * glam::Mat4::look_at_rh(local_pos, local_pos + cam.forward(), glam::Vec3::Y)
}

impl Renderer {
    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        underwater: bool,
        shader_params: Option<&petramond::world::environment::ShaderParamMap>,
    ) {
        let render_origin = render_origin_for_camera(cam.pos);
        let local_cam = cam.pos - render_origin;
        let view_proj = relative_view_proj(cam, render_origin);
        let inv_view_proj = view_proj.inverse();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.view.frustum = Frustum::from_view_proj(view_proj);
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
        let effective_fog_color = if shader_light_overrode_identity && !underwater {
            [
                fog_color[0] * effective_sky_scale * effective_sky_color[0],
                fog_color[1] * effective_sky_scale * effective_sky_color[1],
                fog_color[2] * effective_sky_scale * effective_sky_color[2],
            ]
        } else {
            fog_color
        };
        self.sky.clear_color = effective_fog_color;
        self.sky.underwater = underwater;
        self.sky.scale = effective_sky_scale;
        self.sky.color = effective_sky_color;
        let (fog_start, fog_end) = if underwater {
            (UNDERWATER_FOG_START, UNDERWATER_FOG_END)
        } else {
            (self.sky.fog_start, self.sky.fog_end)
        };
        self.terrain.view_key = TerrainViewKey {
            view_proj: view_proj.to_cols_array().map(f32::to_bits),
            cam: cam.pos.to_array().map(f32::to_bits),
            fog: self.terrain_cull_dist().to_bits(),
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [local_cam.x, local_cam.y, local_cam.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            // fog_color.w = the sim's sky scale (1.0 = identity/noon).
            fog_color: [
                effective_fog_color[0],
                effective_fog_color[1],
                effective_fog_color[2],
                effective_sky_scale,
            ],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            render_origin: [render_origin.x, render_origin.y, render_origin.z, 0.0],
            atlas_anim: crate::atlas::atlas_anim_uniform(),
            sky_color: [
                effective_sky_color[0],
                effective_sky_color[1],
                effective_sky_color[2],
                0.0,
            ],
            sun_dir: sun_uniform(shader_params),
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    fn update_shader_params(
        &mut self,
        shader_params: Option<&petramond::world::environment::ShaderParamMap>,
    ) {
        self.queue.write_buffer(
            &self.shader_params_buf,
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
        let ease = 1.0 - (-dt.clamp(0.0, 0.25) / MOOD_EASE_SECONDS).exp();
        self.targets.mood[0] += (target[0].clamp(0.0, 0.5) - self.targets.mood[0]) * ease;
        self.targets.mood[1] += (target[1].clamp(0.0, 0.5) - self.targets.mood[1]) * ease;
        self.queue.write_buffer(
            &self.targets.mood_buf,
            0,
            bytemuck::cast_slice(&[self.targets.mood[0], self.targets.mood[1], 0.0, 0.0]),
        );
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

    /// Advance and store the first-person held-item / hand state for this frame.
    pub fn set_held_item(&mut self, v: HeldItemFrame) {
        self.hand.held_item = self.hand.held_item_anim.update(v);
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
    pub fn set_held_item_light(&mut self, skylight: u8, blocklight: petramond::light::BlockLight6) {
        self.hand.held_item_skylight = skylight.min(crate::lighting::FULL_SKYLIGHT);
        self.hand.held_item_blocklight = blocklight;
    }

    /// This frame's mod draw sets. They ride the ITEM-ENTITY opaque stream:
    /// same block atlas, same double-sided CPU-lit pipeline, and no chunk
    /// re-mesh — which is the whole reason a mod may submit a new set every
    /// tick.
    pub fn set_block_draws(&mut self, v: &[crate::BlockDrawInstance]) {
        self.item_entity.block_draws.clear();
        self.item_entity.block_draws.extend_from_slice(v);
    }

    /// Store the dropped item-entities to draw this frame. Reuses the existing
    /// `Vec` capacity (clear + extend) to avoid per-frame reallocation.
    pub fn set_item_entities(&mut self, v: &[ItemEntityInstance]) {
        self.item_entity.instances.clear();
        self.item_entity.instances.extend_from_slice(v);
    }

    /// Store the placed chests to draw this frame. Reuses the existing `Vec`
    /// capacity (clear + extend) to avoid per-frame reallocation.
    pub(crate) fn set_chests(&mut self, v: &[ChestInstance]) {
        self.block_entity.chests.clear();
        self.block_entity.chests.extend_from_slice(v);
    }

    /// Store the placed doors to draw this frame. Reuses the existing `Vec` capacity
    /// (clear + extend) to avoid per-frame reallocation.
    pub(crate) fn set_doors(&mut self, v: &[DoorInstance]) {
        self.block_entity.doors.clear();
        self.block_entity.doors.extend_from_slice(v);
    }

    /// Store the mobs to draw this frame (already interpolated by the scene adapter).
    /// Reuses the existing `Vec` capacity.
    pub fn set_mobs(&mut self, v: &[MobRenderInstance]) {
        self.actor.mobs.clear();
        self.actor.mobs.extend_from_slice(v);
    }

    /// Store the LOCAL third-person player body to draw this frame (`None` in
    /// first person — the body, and its held item, then draw nothing). Its
    /// held item animates from the renderer's own first-person `held_item`
    /// view, exactly as before remote players existed.
    pub fn set_player(&mut self, v: Option<PlayerRenderInstance>) {
        self.actor.player_view = v;
    }

    /// Store the REMOTE players' bodies + held-item views for this frame
    /// (already interpolated/posed by the game). Reuses capacity.
    pub fn set_remote_players(&mut self, v: &[super::RemotePlayerRender]) {
        self.actor.remote_players.clear();
        self.actor.remote_players.extend_from_slice(v);
    }

    /// Store the block-atlas particle cubes to draw this frame. Reuses capacity.
    pub fn set_particles(&mut self, v: &[ParticleInstance]) {
        self.particle.instances.clear();
        self.particle.instances.extend_from_slice(v);
    }

    /// Store the model-atlas particle cubes (bbmodel-block flecks) for this frame; they
    /// bake into the same particle vbuf after the block cubes and draw with the model
    /// atlas bound. Reuses capacity.
    pub fn set_model_particles(&mut self, v: &[ParticleInstance]) {
        self.particle.model_instances.clear();
        self.particle.model_instances.extend_from_slice(v);
    }

    /// Store loaded block-row particle emitters for this frame. The renderer derives
    /// transient translucent cubes from these in `bake_world_instances`.
    pub fn set_particle_emitters(&mut self, v: &[ParticleEmitterInstance]) {
        self.particle.emitters.clear();
        self.particle.emitters.extend_from_slice(v);
    }

    /// Store the solid-color simulated particles (emitter-burst droplets) for
    /// this frame; they join the emitter cubes' alpha-blended bake.
    pub fn set_solid_particles(&mut self, v: &[SolidParticleInstance]) {
        self.particle.solid_instances.clear();
        self.particle.solid_instances.extend_from_slice(v);
    }

    pub fn clear_world_state(&mut self) {
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
    }

    /// True while terrain columns are still queued for GPU upload. Uploads are
    /// spread over frames to protect frame time, so a caller that must draw the
    /// COMPLETE terrain in one shot pumps [`Renderer::sync_meshes`] until this
    /// clears.
    pub fn terrain_uploads_pending(&self) -> bool {
        !self.terrain.upload_pending.is_empty()
    }

    /// Synchronize GPU meshes with the terrain CPU meshes.
    pub fn sync_meshes(&mut self, terrain: &mut TerrainRenderHandoff<'_>) {
        self.terrain.upload_frame = self.terrain.upload_frame.wrapping_add(1);
        let upload_frame = self.terrain.upload_frame;
        // Drop packed GPU columns whose CPU meshes are gone.
        let before_columns = self.terrain.columns.len();
        self.terrain
            .columns
            .retain(|p, _| terrain.has_column_mesh(*p));
        if self.terrain.columns.len() != before_columns {
            self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
        }

        let cam = self.view.cam_pos;
        let frustum = self.view.frustum;
        let render_origin = self.view.render_origin;
        let fog = self.terrain_cull_dist();
        let priority = |column: ChunkPos| {
            let min = glam::Vec3::new(
                (column.cx * 16) as f32,
                petramond::chunk::WORLD_MIN_Y as f32,
                (column.cz * 16) as f32,
            );
            let max = glam::Vec3::new(
                (column.cx * 16 + 16) as f32,
                petramond::chunk::WORLD_MAX_Y as f32,
                (column.cz * 16 + 16) as f32,
            );
            let visible_soon = frustum.aabb_visible(min - render_origin, max - render_origin)
                && aabb_distance_sq(cam, min, max) <= fog * fog;
            let center = glam::Vec3::new(
                column.cx as f32 * 16.0 + 8.0,
                cam.y,
                column.cz as f32 * 16.0 + 8.0,
            );
            (
                u8::from(!visible_soon),
                (cam - center).length_squared().to_bits(),
                column.cx,
                column.cz,
            )
        };
        terrain.for_dirty_columns(&mut |column, revision| {
            let mut enqueue = false;
            if let Some(pending) = self.terrain.upload_pending.get_mut(&column) {
                if pending.revision != revision {
                    pending.revision = revision;
                    pending.quiet_after = upload_frame + MESH_UPLOAD_QUIET_FRAMES;
                    enqueue = true;
                }
            } else {
                self.terrain.upload_pending.insert(
                    column,
                    PendingTerrainUpload {
                        revision,
                        quiet_after: upload_frame + MESH_UPLOAD_QUIET_FRAMES,
                        deadline: upload_frame + MESH_UPLOAD_MAX_WAIT_FRAMES,
                    },
                );
                enqueue = true;
            }
            if enqueue {
                let (hidden, distance, cx, cz) = priority(column);
                self.terrain
                    .upload_heap
                    .push(Reverse((hidden, distance, cx, cz, revision)));
            }
        });

        let device = &self.device;
        let queue = &self.queue;
        let columns = &mut self.terrain.columns;
        let upload_scratch = &mut self.terrain.upload_scratch;
        let origins = &mut self.terrain.column_origins;
        let arena = &mut self.terrain.geometry;
        let quad_index = &mut self.terrain.quad_index;
        let start = std::time::Instant::now();
        let mut uploaded_columns = 0usize;
        let mut attempts = 0usize;
        let mut heap_pops = 0usize;
        let mut deferred = Vec::new();
        while attempts < 64 && heap_pops < 128 && uploaded_columns < MESH_COLUMN_UPLOADS_PER_FRAME {
            if uploaded_columns > 0 && start.elapsed() >= MESH_COLUMN_UPLOAD_TIME_BUDGET {
                break;
            }
            let Some(Reverse((_, _, cx, cz, revision))) = self.terrain.upload_heap.pop() else {
                break;
            };
            heap_pops += 1;
            let column = ChunkPos::new(cx, cz);
            let Some(pending) = self.terrain.upload_pending.get(&column) else {
                continue;
            };
            if pending.revision != revision {
                continue;
            }
            attempts += 1;
            if upload_frame < pending.quiet_after && upload_frame < pending.deadline {
                deferred.push((column, revision));
                continue;
            }
            let mut pending = self
                .terrain
                .upload_pending
                .remove(&column)
                .expect("pending upload checked above");
            if !terrain.has_column_mesh(column) {
                let removed = columns.remove(&column).is_some();
                terrain.mark_column_uploaded(column);
                if removed {
                    self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
                }
                continue;
            }
            // Released CPU meshes: the repack must wait for their forced remesh.
            // The column stays upload-dirty and its current GPU buffers keep drawing.
            if terrain.needs_repack_remeshes(column) {
                pending.quiet_after = upload_frame + MESH_UPLOAD_QUIET_FRAMES;
                pending.deadline = upload_frame + MESH_UPLOAD_MAX_WAIT_FRAMES;
                self.terrain.upload_pending.insert(column, pending);
                deferred.push((column, revision));
                continue;
            }
            let uploaded = {
                let meshes = terrain.column_meshes(column);
                if meshes.is_empty() {
                    let removed = columns.remove(&column).is_some();
                    terrain.mark_column_uploaded(column);
                    if removed {
                        self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
                    }
                    false
                } else {
                    let prev = columns.remove(&column);
                    let gpu = upload_column_mesh(
                        device,
                        queue,
                        &meshes,
                        prev,
                        upload_scratch,
                        origins,
                        arena,
                        quad_index,
                    );
                    columns.insert(column, gpu);
                    true
                }
            };
            if uploaded {
                terrain.mark_column_uploaded(column);
                uploaded_columns += 1;
                self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
            }
        }
        for (column, revision) in deferred {
            if self
                .terrain
                .upload_pending
                .get(&column)
                .is_some_and(|pending| pending.revision == revision)
            {
                let (hidden, distance, cx, cz) = priority(column);
                self.terrain
                    .upload_heap
                    .push(Reverse((hidden, distance, cx, cz, revision)));
            }
        }
        let terrain_columns = &self.terrain.columns;
        // A section that lost its far mesh must lose its LOD state too: the
        // planner only consults (and only maintains) the map for sections that
        // still own one.
        self.terrain.far_leaf_lod_state.retain(|sp, _| {
            terrain_columns.get(&sp.chunk_pos()).is_some_and(|column| {
                column
                    .sections
                    .iter()
                    .any(|(pos, s)| pos == sp && s.far_opaque_vertex_count > 0)
            })
        });
    }
}
