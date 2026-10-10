use super::upload_queue::{FrameDrain, UploadPriority};
use super::*;

const MESH_COLUMN_UPLOAD_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(1_750);
const RENDER_ORIGIN_GRID: i32 = 16;

const SUN_ARC_TILT: f32 = 0.15;

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

#[inline]
fn relative_view_proj(
    cam: &Camera,
    render_origin: glam::IVec3,
    view_offset: glam::Mat4,
) -> glam::Mat4 {
    let local_pos = cam.pos.relative_to(render_origin);
    cam.proj()
        * view_offset
        * glam::Mat4::look_at_rh(local_pos, local_pos + cam.forward(), cam.up())
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

    pub fn set_selection(&mut self, shape: Option<SelectionShape>) {
        self.chrome.selection = shape;
    }

    pub fn set_break_overlays(&mut self, v: &[BreakOverlayView]) {
        self.hand.break_overlays.clear();
        self.hand.break_overlays.extend_from_slice(v);
    }

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

    pub fn set_hand_shake(&mut self, shake: [f32; 2]) {
        self.hand.shake = shake;
    }

    pub fn set_crosshair_visible(&mut self, visible: bool) {
        self.chrome.crosshair_visible = visible;
    }

    pub fn set_held_item_light(
        &mut self,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) {
        self.hand.held_item_skylight = skylight.min(crate::lighting::FULL_SKYLIGHT);
        self.hand.held_item_blocklight = blocklight;
    }

    pub fn swap_block_draws(&mut self, v: &mut Vec<crate::BlockDrawInstance>) {
        std::mem::swap(&mut self.item_entity.block_draws, v);
    }

    pub fn swap_cloths(
        &mut self,
        cloths: &mut Vec<crate::views::ClothPresentation>,
        points: &mut Vec<crate::views::ClothPoint>,
        texels: &mut Vec<Option<[f32; 3]>>,
    ) {
        std::mem::swap(&mut self.item_entity.cloths, cloths);
        std::mem::swap(&mut self.item_entity.cloth_points, points);
        std::mem::swap(&mut self.item_entity.cloth_texels, texels);
    }

    pub fn swap_item_entities(&mut self, v: &mut Vec<ItemEntityInstance>) {
        std::mem::swap(&mut self.item_entity.instances, v);
    }

    pub(crate) fn swap_block_entities(&mut self, v: &mut Vec<BlockEntityInstance>) {
        std::mem::swap(&mut self.block_entity.instances, v);
    }

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

    pub fn swap_player_bodies(
        &mut self,
        bodies: &mut Vec<PlayerBodyRender>,
        poses: &mut Vec<glam::Mat4>,
    ) {
        std::mem::swap(&mut self.actor.bodies, bodies);
        std::mem::swap(&mut self.actor.body_poses, poses);
    }

    pub fn swap_particles(&mut self, v: &mut Vec<ParticleInstance>) {
        std::mem::swap(&mut self.particle.instances, v);
    }

    pub fn swap_model_particles(&mut self, v: &mut Vec<ParticleInstance>) {
        std::mem::swap(&mut self.particle.model_instances, v);
    }

    pub fn swap_particle_emitters(&mut self, v: &mut Vec<ParticleEmitterInstance>) {
        std::mem::swap(&mut self.particle.emitters, v);
    }

    pub fn swap_solid_particles(&mut self, v: &mut Vec<SolidParticleInstance>) {
        std::mem::swap(&mut self.particle.solid_instances, v);
    }

    pub fn swap_shadows(&mut self, v: &mut Vec<EntityShadow>) {
        std::mem::swap(&mut self.shadow.instances, v);
    }

    pub fn clear_world_state(&mut self) {
        self.terrain.clear_world();
        self.clear_presented_moment();
    }

    pub fn clear_presented_moment(&mut self) {
        self.ghosts.clear_world();
        self.selection.clear_world();
        self.chrome.clear_world();
        self.hand.clear_world();
        self.particle.clear_world();
        self.item_entity.clear_world();
        self.block_entity.clear_world();
        self.actor.clear_world();
        self.shadow.clear_world();
    }

    pub fn terrain_uploads_pending(&self) -> bool {
        !self.terrain.uploads.is_empty()
    }

    pub fn sync_meshes(&mut self, terrain: &mut TerrainRenderHandoff<'_>) {
        self.terrain.uploads.begin_frame();
        let before_columns = self.terrain.columns.len();
        self.terrain.columns.retain(|p| terrain.has_column_mesh(p));
        if self.terrain.columns.len() != before_columns {
            self.terrain.gpu_revision = self.terrain.gpu_revision.wrapping_add(1);
        }

        let frustum = self.view.frustum;
        let render_origin = self.view.render_origin;
        let cam = self.view.cam_pos.relative_to(render_origin);
        let fog = self.terrain_cull_dist();
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

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn a_view_far_out_draws_with_the_matrix_it_has_near_the_origin() {
        let edge = f64::from(petramond_world::border::WORLD_BORDER - 1);
        for (pitch, roll) in [(0.2, 0.0), (-0.4, 0.7), (-std::f32::consts::FRAC_PI_2, 0.0)] {
            let look = |pos| {
                let mut cam = Camera::new(pos, 16.0 / 9.0);
                cam.yaw = 2.1;
                cam.pitch = pitch;
                cam.roll = roll;
                cam
            };
            let far = look(WorldPos::new(edge - 0.75, -edge + 5.5, -edge + 3.25));
            let origin = render_origin_for_camera(far.pos);
            let offset = far.pos.relative_to(origin);
            assert!(
                offset.cmpge(glam::Vec3::ZERO).all()
                    && offset
                        .cmplt(glam::Vec3::splat(RENDER_ORIGIN_GRID as f32))
                        .all(),
                "offset {offset} left its grid cell"
            );
            let far_vp = relative_view_proj(&far, origin, glam::Mat4::IDENTITY);
            let near = look(WorldPos::new(
                f64::from(offset.x),
                f64::from(offset.y),
                f64::from(offset.z),
            ));
            let near_vp = relative_view_proj(
                &near,
                render_origin_for_camera(near.pos),
                glam::Mat4::IDENTITY,
            );
            assert!(far_vp.is_finite(), "pitch {pitch} roll {roll}");
            assert!(
                far_vp.abs_diff_eq(near_vp, 1e-5),
                "pitch {pitch} roll {roll}"
            );
        }
    }

    #[test]
    fn camera_right_is_drawn_on_the_right_of_the_screen() {
        for (yaw, pitch) in [(0.0f32, 0.0f32), (0.7, 0.3), (2.5, -0.4), (-1.9, 0.9)] {
            let mut cam = Camera::new(
                petramond_math::world_pos::WorldPos::new(100.5, 70.0, -40.25),
                16.0 / 9.0,
            );
            cam.yaw = yaw;
            cam.pitch = pitch;
            let origin = render_origin_for_camera(cam.pos);
            let view_proj = relative_view_proj(&cam, origin, glam::Mat4::IDENTITY);
            let ahead_right =
                cam.pos.relative_to(origin) + cam.forward() * 10.0 + cam.right() * 3.0;
            let clip = view_proj * ahead_right.extend(1.0);
            assert!(
                clip.x / clip.w > 0.0,
                "yaw {yaw} pitch {pitch}: camera right drew left of centre"
            );
        }
    }
}
