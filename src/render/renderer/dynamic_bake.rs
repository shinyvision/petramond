//! Per-frame dynamic CPU geometry bakes for [`Renderer`], lifted verbatim out
//! of `render`'s prologue. Three `&mut self` steps run before encoding:
//! overlay-buffer refresh, held-item geometry, and the world-instance bakes.
//! Behavior, ordering, borrow/scratch-reuse patterns are byte-for-byte identical.

use super::*;

impl Renderer {
    /// Refresh the crosshair + selection-outline vertex buffers when their
    /// inputs changed (resize / new target). Extracted from `render`'s prologue.
    pub(super) fn refresh_overlay_buffers(&mut self) {
        if !self.crosshair_visible {
            self.crosshair_vertex_count = 0;
        } else if self.crosshair_drawn_size != (self.config.width, self.config.height)
            || self.crosshair_vertex_count == 0
        {
            let verts = crosshair_vertices(self.config.width, self.config.height);
            self.crosshair_vertex_count = verts.count;
            if verts.count > 0 {
                self.queue.write_buffer(
                    &self.crosshair_vbuf,
                    0,
                    bytemuck::cast_slice(&verts.vertices[..verts.count as usize]),
                );
            }
            self.crosshair_drawn_size = (self.config.width, self.config.height);
        }

        // Refresh the outline vertex buffer only when the target changed.
        if self.selection != self.selection_drawn {
            self.outline_vertex_count = 0;
            if let Some(shape) = self.selection {
                let outline = outline_vertices(shape);
                self.outline_vertex_count = outline.count;
                if outline.count > 0 {
                    self.queue.write_buffer(
                        &self.outline_vbuf,
                        0,
                        bytemuck::cast_slice(&outline.vertices[..outline.count as usize]),
                    );
                }
            }
            self.selection_drawn = self.selection;
        }
    }

    /// Build + upload this frame's first-person hand geometry and the extruded /
    /// bbmodel held-item geometry (mutually exclusive). Extracted from `render`.
    pub(super) fn prepare_held_item(&mut self) {
        if !self.hand_visible {
            self.hand_index_count = 0;
            self.hand_vertex_count = 0;
            self.item3d_vertex_count = 0;
            return;
        }

        // Build + upload the first-person hand geometry for this frame. The hand
        // uses its own fixed perspective (it is drawn over the world, no depth),
        // so the MVP is computed entirely here from the framebuffer aspect and the
        // App-supplied swing/place phases, then written to MVP slot 0. The dynamic
        // vbuf/ibuf are rewritten in place (no per-frame allocation).
        self.hand_index_count = 0;
        self.hand_vertex_count = 0;
        {
            let aspect = if self.config.height > 0 {
                self.config.width as f32 / self.config.height as f32
            } else {
                1.0
            };
            // Take the reusable hand staging out so `build_hand` can borrow them
            // mutably alongside the immutable `held_item` borrow, then restore.
            let mut hv = std::mem::take(&mut self.hand_verts);
            let mut hi = std::mem::take(&mut self.hand_indices);
            let mvp = build_hand_lit(
                &self.held_item,
                aspect,
                self.held_item_skylight,
                self.held_item_warm,
                &mut hv,
                &mut hi,
            );
            if !hi.is_empty() {
                self.queue
                    .write_buffer(&self.model3d_vbuf, 0, bytemuck::cast_slice(&hv));
                self.queue
                    .write_buffer(&self.model3d_ibuf, 0, bytemuck::cast_slice(&hi));
                // MVP slot 0: a 64-byte mat4 at offset 0 of the 256-aligned buffer.
                self.queue.write_buffer(
                    &self.model3d_mvp_buf,
                    0,
                    bytemuck::cast_slice(&mvp.to_cols_array()),
                );
                self.hand_index_count = hi.len() as u32;
                self.hand_vertex_count = hv.len() as u32;
            }
            self.hand_verts = hv;
            self.hand_indices = hi;
        }

        // Build + upload the EXTRUDED held item (sprite-kind: flowers / future
        // tools), drawn by the dedicated item3d pipeline in the hand pass. Mutually
        // exclusive with the model3d hand geometry (a sprite emits none above), so
        // its MVP reuses slot 0 of `model3d_mvp_buf`. The item3d vbuf is rewritten
        // in place (no per-frame allocation beyond capacity).
        self.item3d_vertex_count = 0;
        self.held_is_model = false;
        {
            let aspect = if self.config.height > 0 {
                self.config.width as f32 / self.config.height as f32
            } else {
                1.0
            };
            if let Some((kind, mvp)) = crate::render::hand::held_model(&self.held_item, aspect) {
                // A held bbmodel block: bake its real model (model atlas) into the item3d
                // vbuf and draw it through the item3d pipeline bound to the MODEL atlas.
                // item3d is non-indexed, so expand the baked indexed mesh to a triangle
                // list. Mutually exclusive with a held sprite (one render kind).
                let mut iv = std::mem::take(&mut self.item3d_verts);
                iv.clear();
                let (mut tv, mut ti) = (Vec::new(), Vec::new());
                crate::render::item_model::build_block_model_item(
                    kind,
                    glam::Mat4::IDENTITY,
                    self.held_item_skylight,
                    self.held_item_warm,
                    None,
                    &mut tv,
                    &mut ti,
                );
                for &idx in &ti {
                    iv.push(tv[idx as usize]);
                }
                let cap = crate::render::pipeline::MAX_ITEM3D_VERTICES as usize;
                if !iv.is_empty() && iv.len() <= cap {
                    self.queue
                        .write_buffer(&self.item3d_vbuf, 0, bytemuck::cast_slice(&iv));
                    self.queue.write_buffer(
                        &self.model3d_mvp_buf,
                        0,
                        bytemuck::cast_slice(&mvp.to_cols_array()),
                    );
                    self.item3d_vertex_count = iv.len() as u32;
                    self.held_is_model = true;
                }
                self.item3d_verts = iv;
            } else if let Some((tile, mvp)) =
                crate::render::hand::held_sprite(&self.held_item, aspect)
            {
                let mut iv = std::mem::take(&mut self.item3d_verts);
                let count = crate::render::item_model::build_extruded_item_lit(
                    tile,
                    self.held_item_skylight,
                    &mut iv,
                );
                // Warm the extruded held sprite by the block-light at the player, to
                // match the warm tint static blocks + the model3d hand take near a
                // torch/furnace. (Item entities reuse this builder but aren't warmed.)
                if self.held_item_warm > 0 {
                    let w = self.held_item_warm as f32 / 255.0;
                    for v in iv.iter_mut() {
                        v.tint = crate::torch::warm_tint(v.tint, w);
                    }
                }
                let cap = crate::render::pipeline::MAX_ITEM3D_VERTICES as usize;
                if count > 0 && iv.len() <= cap {
                    self.queue
                        .write_buffer(&self.item3d_vbuf, 0, bytemuck::cast_slice(&iv));
                    // MVP slot 0 (the model3d hand slot is free for a held sprite).
                    self.queue.write_buffer(
                        &self.model3d_mvp_buf,
                        0,
                        bytemuck::cast_slice(&mvp.to_cols_array()),
                    );
                    self.item3d_vertex_count = count;
                }
                self.item3d_verts = iv;
            }
        }
    }

    /// Bake every dynamic world subsystem (item-entity, item-model-entity, chest,
    /// door, mob, break, particle) for this frame, in the order that reuses the
    /// shared item-entity scratch. Extracted verbatim from `render`.
    pub(super) fn bake_world_instances(&mut self) {
        let render_origin = self.render_origin;
        let visible_world_aabb = |min: glam::Vec3, max: glam::Vec3| {
            self.frustum
                .aabb_visible(min - render_origin, max - render_origin)
        };
        // Bake the dynamic world subsystems. Item-entity, chest, and break-overlay
        // each clear-and-refill the SAME shared CPU scratch (`item_entity_verts` /
        // `item_entity_indices`) in this exact order — `bake` (clear count → build
        // → bounds-check → upload to that subsystem's OWN fixed buffers → store
        // count) runs sequentially, never aliasing two GPU buffers at once. Each
        // subsystem keeps its OWN buffer caps (item-entity vs chest sized apart so
        // a wall of chests can't make dropped items vanish).

        // Item entities (spinning cubes / sprite billboards), frustum-culled so
        // off-screen drops cost nothing. Drawn by the EXISTING opaque pipeline.
        self.item_entity_visible.clear();
        for inst in &self.item_entities {
            // ~0.5 m cull box around the item centre.
            let c = inst.pos;
            let min = c - glam::Vec3::splat(0.5);
            let max = c + glam::Vec3::new(0.5, 1.0, 0.5);
            if visible_world_aabb(min, max) {
                self.item_entity_visible.push(*inst);
            }
        }
        let basis = self.billboard_basis;
        let visible = &self.item_entity_visible;
        self.item_entity_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_item_entities(visible, basis, verts, indices),
        );
        // Dropped bbmodel items (their own model atlas), baked from the same visible set.
        let visible = &self.item_entity_visible;
        self.item_model_entity_draw.bake(
            &self.queue,
            &mut self.item_model_entity_verts,
            &mut self.item_model_entity_indices,
            |verts, indices| {
                crate::render::item_entity::build_item_model_entities(visible, verts, indices)
            },
        );

        // Chests (inset body + hinged lid), frustum-culled like item entities and
        // reusing their CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.chest_visible.clear();
        for inst in &self.chests {
            // Cull box: the block cell, expanded upward to include the open lid.
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.chest_visible.push(*inst);
            }
        }
        let chest_visible = &self.chest_visible;
        self.chest_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_chests(chest_visible, verts, indices),
        );

        // Doors (2-tall hinged slab), frustum-culled and baked exactly like chests,
        // reusing the same CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.door_visible.clear();
        for inst in &self.doors {
            // Cull box: the door's two-cell column (its swung slab stays within it).
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.door_visible.push(*inst);
            }
        }
        let door_visible = &self.door_visible;
        self.door_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_doors(door_visible, verts, indices),
        );

        // Mobs (animated entity models), grouped by species and frustum-culled, baked
        // into each species' OWN `ItemVertex` buffers (a different vertex type from the
        // packed block vertex). Each instance is posed by the walk animation at its
        // `anim_time` when moving, else the model's rest pose.
        for g in &mut self.mob_gpu {
            g.visible.clear();
        }
        for inst in &self.mobs {
            // Cull box: ~0.5 m around the feet, expanded up for the standing body. A
            // killed mob is flung from its (frozen) death point and tumbles across the
            // ground, so use a generous box while it's ragdolling so the flying corpse
            // doesn't pop out of view.
            let pad = if inst.ragdoll.is_some() {
                glam::Vec3::splat(6.0)
            } else {
                glam::Vec3::new(0.5, 1.2, 0.5)
            };
            let min = inst.pos - pad;
            let max = inst.pos + pad;
            if visible_world_aabb(min, max) {
                self.mob_gpu[inst.kind as usize].visible.push(inst.clone());
            }
        }
        let queue = &self.queue;
        for g in &mut self.mob_gpu {
            let model = g.model;
            let scale = g.scale;
            let visible = &g.visible;
            g.draw
                .bake(queue, &mut g.verts, &mut g.indices, |verts, indices| {
                    build_mob_instances(model, scale, visible, verts, indices)
                });
        }

        // Break-overlay (destroy crack) cube, when a block is targeted.
        let break_overlay = self.break_overlay;
        self.break_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| match break_overlay {
                Some(view) => build_break_overlay(&view, verts, indices),
                None => {
                    verts.clear();
                    indices.clear();
                    0
                }
            },
        );

        // Tiny 3D particle cubes into the reusable vbuf (static quad ibuf): block-atlas
        // flecks first, then bbmodel-block (model-atlas) flecks, so the draw splits at one
        // contiguous index boundary (`particle_block_vertex_count`).
        let particles = &self.particles;
        let model_particles = &self.model_particles;
        let mut block_v = 0u32;
        self.particle_draw
            .bake(&self.queue, &mut self.particle_verts, |verts| {
                let (total, nb) = build_particles_split(particles, model_particles, verts);
                block_v = nb;
                total
            });
        self.particle_block_vertex_count = if self.particle_draw.vertex_count == 0 {
            0
        } else {
            block_v
        };
    }
}
