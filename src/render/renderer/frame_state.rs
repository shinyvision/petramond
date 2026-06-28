//! Per-frame view-state setters + terrain sync for [`Renderer`].
//!
//! Cheap mutators the app calls each frame to hand the renderer the camera
//! uniforms, selection/break overlay, held item, world instance lists, UI
//! snapshot, and the terrain mesh/section-visibility sync. Split out of the
//! renderer god-file; behavior is byte-for-byte identical.

use super::*;

impl Renderer {
    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        underwater: bool,
    ) {
        let view_proj = cam.view_proj();
        let inv_view_proj = view_proj.inverse();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        self.cam_pos = cam.pos;
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
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
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

    /// Whether the renderer-owned first-person hand animation is still moving.
    /// The app uses this to keep redraw-on-demand alive for one-shot place/break/
    /// attack swings after the sim event frame has already passed.
    #[inline]
    pub fn hand_animation_active(&self) -> bool {
        self.held_item_anim.is_active()
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

    /// Synchronize GPU meshes with the terrain CPU meshes.
    pub(crate) fn sync_meshes(&mut self, terrain: &mut impl TerrainMeshUploadSource) {
        // Drop GPU meshes whose CPU chunk is gone through the terrain handoff so
        // no per-frame scratch set is allocated.
        self.chunk_meshes.retain(|p, _| terrain.has_mesh(*p));
        // Upload only meshes marked dirty by the world (newly built/changed) or
        // missing on the GPU. The handoff clears the CPU dirty flag only after
        // this callback reports a completed upload.
        let device = &self.device;
        let chunk_meshes = &mut self.chunk_meshes;
        terrain.for_each_mesh_upload(|pos, mesh, dirty| {
            let need_upload = !chunk_meshes.contains_key(&pos) || dirty;
            if need_upload {
                let gm = upload_mesh(device, mesh, pos);
                chunk_meshes.insert(pos, gm);
            }
            need_upload
        });
    }

    pub(crate) fn update_section_visibility(&mut self, terrain: &mut impl TerrainVisibilitySource) {
        self.section_visibility.update(terrain, self.cam_pos);
    }
}
