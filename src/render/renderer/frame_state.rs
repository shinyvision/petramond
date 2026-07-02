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
const MESH_COLUMN_UPLOADS_PER_FRAME: usize = 6;
/// Soft render-thread budget for packing/writing terrain columns. One upload is always
/// allowed so terrain keeps making progress; after that, leave time for the actual frame.
const MESH_COLUMN_UPLOAD_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(1_750);
const RENDER_ORIGIN_GRID: f32 = 16.0;

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
    ) {
        let render_origin = render_origin_for_camera(cam.pos);
        let local_cam = cam.pos - render_origin;
        let view_proj = relative_view_proj(cam, render_origin);
        let inv_view_proj = view_proj.inverse();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        self.cam_pos = cam.pos;
        self.render_origin = render_origin;
        // Camera right/up axes for world-space billboards (item sprites + dust):
        // a quad spanned by these always faces the viewer.
        self.billboard_basis = BillboardBasis {
            right: cam.right(),
            up: cam.up(),
        };
        self.clear_color = fog_color;
        let (fog_start, fog_end) = if underwater {
            (UNDERWATER_FOG_START, UNDERWATER_FOG_END)
        } else {
            (FOG_START, FOG_END)
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [local_cam.x, local_cam.y, local_cam.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            render_origin: [render_origin.x, render_origin.y, render_origin.z, 0.0],
            water_anim: crate::atlas::water_anim_uniform(),
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    /// Set (or clear) the target highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, shape: Option<SelectionShape>) {
        self.selection = shape;
    }

    /// Store the block-break overlay to draw this frame (or `None` to clear).
    pub fn set_break_overlay(&mut self, v: Option<BreakOverlayView>) {
        self.break_overlay = v;
    }

    /// Advance and store the first-person held-item / hand state for this frame.
    pub fn set_held_item(&mut self, v: HeldItemFrame) {
        self.held_item = self.held_item_anim.update(v);
    }

    pub fn set_hand_visible(&mut self, visible: bool) {
        self.hand_visible = visible;
    }

    pub fn set_crosshair_visible(&mut self, visible: bool) {
        self.crosshair_visible = visible;
    }

    /// Store the combined light + warm-tint amount to apply to the first-person hand
    /// / held item (so it brightens AND warms near torches/furnaces).
    pub fn set_held_item_light(&mut self, skylight: u8, warm: u8) {
        self.held_item_skylight = skylight.min(crate::render::lighting::FULL_SKYLIGHT);
        self.held_item_warm = warm;
    }

    /// Store the dropped item-entities to draw this frame. Reuses the existing
    /// `Vec` capacity (clear + extend) to avoid per-frame reallocation.
    pub fn set_item_entities(&mut self, v: &[ItemEntityInstance]) {
        self.item_entities.clear();
        self.item_entities.extend_from_slice(v);
    }

    /// Store the placed chests to draw this frame. Reuses the existing `Vec`
    /// capacity (clear + extend) to avoid per-frame reallocation.
    pub(in crate::render) fn set_chests(&mut self, v: &[ChestInstance]) {
        self.chests.clear();
        self.chests.extend_from_slice(v);
    }

    /// Store the placed doors to draw this frame. Reuses the existing `Vec` capacity
    /// (clear + extend) to avoid per-frame reallocation.
    pub(in crate::render) fn set_doors(&mut self, v: &[DoorInstance]) {
        self.doors.clear();
        self.doors.extend_from_slice(v);
    }

    /// Store the mobs to draw this frame (already interpolated by the scene adapter).
    /// Reuses the existing `Vec` capacity.
    pub fn set_mobs(&mut self, v: &[MobRenderInstance]) {
        self.mobs.clear();
        self.mobs.extend_from_slice(v);
    }

    /// Store the block-atlas particle cubes to draw this frame. Reuses capacity.
    pub fn set_particles(&mut self, v: &[ParticleInstance]) {
        self.particles.clear();
        self.particles.extend_from_slice(v);
    }

    /// Store the model-atlas particle cubes (bbmodel-block flecks) for this frame; they
    /// bake into the same particle vbuf after the block cubes and draw with the model
    /// atlas bound. Reuses capacity.
    pub fn set_model_particles(&mut self, v: &[ParticleInstance]) {
        self.model_particles.clear();
        self.model_particles.extend_from_slice(v);
    }

    /// Store the already-owned UI state needed for this frame's UI pass.
    pub fn set_ui(&mut self, v: UiSnapshot) {
        self.ui = v;
    }

    pub fn clear_world_state(&mut self) {
        self.terrain_columns.clear();
        self.far_leaf_lod_state.clear();
        self.draw_order.clear();
        self.opaque_column_order.clear();
        self.model_column_order.clear();
        self.selection = None;
        self.selection_drawn = None;
        self.outline_vertex_count = 0;
        self.crosshair_visible = false;
        self.crosshair_vertex_count = 0;
        self.hand_visible = false;
        self.hand_index_count = 0;
        self.hand_vertex_count = 0;
        self.item3d_vertex_count = 0;
        self.break_overlay = None;
        self.break_draw.index_count = 0;
        self.item_entity_draw.index_count = 0;
        self.item_model_entity_draw.index_count = 0;
        self.chest_draw.index_count = 0;
        self.door_draw.index_count = 0;
        self.particle_draw.vertex_count = 0;
        self.item_entities.clear();
        self.particles.clear();
        self.model_particles.clear();
        self.chests.clear();
        self.doors.clear();
        self.mobs.clear();
        for mob in &mut self.mob_gpu {
            mob.draw.index_count = 0;
            mob.visible.clear();
        }
    }

    /// Synchronize GPU meshes with the terrain CPU meshes.
    pub(crate) fn sync_meshes(&mut self, terrain: &mut TerrainRenderHandoff<'_>) {
        // Drop packed GPU columns whose CPU meshes are gone.
        self.terrain_columns
            .retain(|p, _| terrain.has_column_mesh(*p));

        let cam = self.cam_pos;
        let frustum = self.frustum;
        let render_origin = self.render_origin;
        let mut dirty_columns = std::mem::take(&mut self.terrain_upload_order);
        dirty_columns.clear();
        terrain.for_dirty_columns(&mut |column| {
            let min = glam::Vec3::new(
                (column.cx * 16) as f32,
                crate::chunk::WORLD_MIN_Y as f32,
                (column.cz * 16) as f32,
            );
            let max = glam::Vec3::new(
                (column.cx * 16 + 16) as f32,
                crate::chunk::WORLD_MAX_Y as f32,
                (column.cz * 16 + 16) as f32,
            );
            let fog = FOG_END + TERRAIN_FOG_CULL_PAD;
            let visible_soon = frustum.aabb_visible(min - render_origin, max - render_origin)
                && aabb_distance_sq(cam, min, max) <= fog * fog;
            let center = glam::Vec3::new(
                column.cx as f32 * 16.0 + 8.0,
                cam.y,
                column.cz as f32 * 16.0 + 8.0,
            );
            dirty_columns.push((!visible_soon, (cam - center).length_squared(), column));
        });
        dirty_columns.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));

        let device = &self.device;
        let queue = &self.queue;
        let columns = &mut self.terrain_columns;
        let upload_scratch = &mut self.terrain_upload_scratch;
        let start = std::time::Instant::now();
        let mut uploaded_columns = 0usize;
        for &(_, _, column) in &dirty_columns {
            if uploaded_columns >= MESH_COLUMN_UPLOADS_PER_FRAME {
                break;
            }
            let uploaded = {
                let meshes = terrain.column_meshes(column);
                if meshes.is_empty() {
                    columns.remove(&column);
                    terrain.mark_column_uploaded(column);
                    false
                } else {
                    let prev = columns.remove(&column);
                    let gpu = upload_column_mesh(device, queue, &meshes, prev, upload_scratch);
                    columns.insert(column, gpu);
                    true
                }
            };
            if uploaded {
                terrain.mark_column_uploaded(column);
                uploaded_columns += 1;
                if uploaded_columns > 0 && start.elapsed() >= MESH_COLUMN_UPLOAD_TIME_BUDGET {
                    break;
                }
            }
        }
        dirty_columns.clear();
        self.terrain_upload_order = dirty_columns;
        let terrain_columns = &self.terrain_columns;
        self.far_leaf_lod_state.retain(|sp, _| {
            terrain_columns
                .get(&sp.chunk_pos())
                .is_some_and(|column| column.sections.iter().any(|(pos, _)| pos == sp))
        });
    }
}
