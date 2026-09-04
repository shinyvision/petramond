//! Per-frame dynamic CPU geometry bakes for [`Renderer`], lifted verbatim out
//! of `render`'s prologue. Three `&mut self` steps run before encoding:
//! overlay-buffer refresh, held-item geometry, and the world-instance bakes.
//! Behavior, ordering, borrow/scratch-reuse patterns are byte-for-byte identical.

use super::*;

impl Renderer {
    /// The frame's CPU lighting environment (sky scale + colour), mirroring the
    /// shader uniform lanes for the explicit-shade dynamic bakes.
    #[inline]
    fn light_env(&self) -> crate::lighting::LightEnv {
        crate::lighting::LightEnv {
            sky_scale: self.sky.scale,
            sky_color: self.sky.color,
        }
    }

    /// The two-channel light sampled at the local player, lighting every
    /// held-item variant.
    #[inline]
    fn held_item_light(&self) -> crate::lighting::DynLight {
        crate::lighting::DynLight::new(self.hand.held_item_skylight, self.hand.held_item_blocklight)
    }

    /// Refresh the crosshair + selection-outline vertex buffers when their
    /// inputs changed (resize / new target). Extracted from `render`'s prologue.
    pub(super) fn refresh_overlay_buffers(&mut self) {
        if !self.chrome.crosshair_visible {
            self.chrome.crosshair_vertex_count = 0;
        } else if self.chrome.crosshair_drawn_size != (self.config.width, self.config.height)
            || self.chrome.crosshair_vertex_count == 0
        {
            let verts = crosshair_vertices(self.config.width, self.config.height);
            self.chrome.crosshair_vertex_count = verts.count;
            if verts.count > 0 {
                self.queue.write_buffer(
                    &self.chrome.crosshair_vbuf,
                    0,
                    bytemuck::cast_slice(&verts.vertices[..verts.count as usize]),
                );
            }
            self.chrome.crosshair_drawn_size = (self.config.width, self.config.height);
        }

        // Refresh the outline vertex buffer only when the target changed.
        if self.chrome.selection != self.chrome.selection_drawn {
            self.chrome.outline_vertex_count = 0;
            if let Some(shape) = self.chrome.selection {
                let outline = outline_vertices(shape);
                self.chrome.outline_vertex_count = outline.count();
                if outline.count() > 0 {
                    super::dynamic_draw::upload(
                        &self.device,
                        &self.queue,
                        &mut self.chrome.outline_vbuf,
                        &outline.vertices,
                        wgpu::BufferUsages::VERTEX,
                        "outline vbuf",
                    );
                }
            }
            self.chrome.selection_drawn = self.chrome.selection;
        }
    }

    /// The hurt-shake as a clip-space post-transform: left-multiplying a
    /// translation adds `t * w` to the clip position, which after the divide is
    /// exactly an NDC screen shift — the whole hand jitters without touching
    /// any pose math.
    fn hand_shake_mat(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(glam::Vec3::new(self.hand.shake[0], self.hand.shake[1], 0.0))
    }

    /// Build + upload this frame's first-person hand geometry and the extruded /
    /// bbmodel held-item geometry (mutually exclusive per hand). Both hands
    /// build into ONE CPU stream per render kind — the off hand appended after
    /// the main — and each stream uploads once, growing its buffer to fit.
    pub(super) fn prepare_held_item(&mut self) {
        self.hand.index_count = 0;
        self.hand.vertex_count = 0;
        self.hand.item3d_vertex_count = 0;
        self.hand.held_is_model = false;
        self.hand.off_index_count = 0;
        self.hand.off_item3d_start = 0;
        self.hand.off_item3d_count = 0;
        self.hand.off_is_model = false;
        if !self.hand.visible {
            return;
        }
        let aspect = if self.config.height > 0 {
            self.config.width as f32 / self.config.height as f32
        } else {
            1.0
        };
        let shake = self.hand_shake_mat();
        let light = self.held_item_light();
        let env = self.light_env();

        // The hand uses its own fixed perspective (drawn over the world), so
        // each MVP is computed here from the framebuffer aspect and the
        // App-supplied swing/place phases: slot 0 is the main hand, slot 1
        // (byte offset 256) the off hand.
        let mut hv = std::mem::take(&mut self.hand.verts);
        let mut hi = std::mem::take(&mut self.hand.indices);
        let mut iv = std::mem::take(&mut self.hand.item3d_verts);
        let mut tv = std::mem::take(&mut self.hand.model_scratch_verts);
        let mut ti = std::mem::take(&mut self.hand.model_scratch_indices);
        iv.clear();
        let mut main_mvp = None;
        let mut off_mvp = None;

        // MAIN hand: the block cube (model3d) or, mutually exclusively, the
        // extruded sprite / bbmodel (item3d).
        let mvp = shake * build_hand_lit(&self.hand.held_item, aspect, light, &mut hv, &mut hi);
        if !hi.is_empty() {
            self.hand.index_count = hi.len() as u32;
            self.hand.vertex_count = hv.len() as u32;
            main_mvp = Some(mvp);
        }
        if let Some((kind, mvp)) = crate::hand::held_model(&self.hand.held_item, aspect) {
            tv.clear();
            ti.clear();
            crate::item_model::build_block_model_item(
                kind,
                glam::Mat4::IDENTITY,
                light,
                env,
                None,
                &mut tv,
                &mut ti,
            );
            // item3d is non-indexed: expand the baked mesh to a triangle list.
            iv.extend(ti.iter().map(|&idx| tv[idx as usize]));
            if !iv.is_empty() {
                self.hand.item3d_vertex_count = iv.len() as u32;
                self.hand.held_is_model = true;
                main_mvp = Some(shake * mvp);
            }
        } else if let Some((tile, mvp)) = crate::hand::held_sprite(&self.hand.held_item, aspect) {
            let count = crate::item_model::build_extruded_stack_lit(
                tile,
                self.hand.held_item.variant,
                light,
                env,
                &mut iv,
            );
            if count > 0 {
                self.hand.item3d_vertex_count = count;
                main_mvp = Some(shake * mvp);
            }
        }

        // OFF (left) hand: the same three render-kind paths, mirrored
        // placements (`hand::mirror_x`), appended after the main hand's
        // geometry. Empty off-hand = nothing drawn — there is no bare left
        // arm. Model3d indices stay off-stream-relative and draw with
        // `base_vertex = vertex_count`.
        self.hand.off_item3d_start = self.hand.item3d_vertex_count;
        if self.hand.off_item.item.is_some() {
            let mut ov = std::mem::take(&mut self.hand.off_verts);
            let mut oi = std::mem::take(&mut self.hand.off_indices);
            let mvp = shake
                * crate::hand::build_off_hand_lit(
                    &self.hand.off_item,
                    aspect,
                    light,
                    &mut ov,
                    &mut oi,
                );
            if !oi.is_empty() {
                hv.extend_from_slice(&ov);
                hi.extend_from_slice(&oi);
                self.hand.off_index_count = oi.len() as u32;
                off_mvp = Some(mvp);
            }
            self.hand.off_verts = ov;
            self.hand.off_indices = oi;
            if let Some((kind, mvp)) = crate::hand::held_model_off(&self.hand.off_item, aspect) {
                tv.clear();
                ti.clear();
                crate::item_model::build_block_model_item(
                    kind,
                    glam::Mat4::IDENTITY,
                    light,
                    env,
                    None,
                    &mut tv,
                    &mut ti,
                );
                let start = iv.len();
                iv.extend(ti.iter().map(|&idx| tv[idx as usize]));
                if iv.len() > start {
                    self.hand.off_item3d_count = (iv.len() - start) as u32;
                    self.hand.off_is_model = true;
                    off_mvp = Some(shake * mvp);
                }
            } else if let Some((tile, mvp)) =
                crate::hand::held_sprite_off(&self.hand.off_item, aspect)
            {
                // The extrusion clears its buffer, so it bakes into a scratch
                // and appends.
                let mut sv = std::mem::take(&mut self.hand.off_item3d_scratch);
                let count = crate::item_model::build_extruded_stack_lit(
                    tile,
                    self.hand.off_item.variant,
                    light,
                    env,
                    &mut sv,
                );
                if count > 0 {
                    iv.extend_from_slice(&sv);
                    self.hand.off_item3d_count = count;
                    off_mvp = Some(shake * mvp);
                }
                self.hand.off_item3d_scratch = sv;
            }
        }

        // One upload per stream, each buffer grown to fit.
        if !hi.is_empty() {
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.model3d_vbuf,
                &hv,
                wgpu::BufferUsages::VERTEX,
                "model3d vbuf",
            );
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.model3d_ibuf,
                &hi,
                wgpu::BufferUsages::INDEX,
                "model3d ibuf",
            );
        }
        if !iv.is_empty() {
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.item3d_vbuf,
                &iv,
                wgpu::BufferUsages::VERTEX,
                "item3d vbuf",
            );
        }
        for (slot, mvp) in [(0u64, main_mvp), (256u64, off_mvp)] {
            if let Some(mvp) = mvp {
                self.queue.write_buffer(
                    &self.hand.model3d_mvp_buf,
                    slot,
                    bytemuck::cast_slice(&mvp.to_cols_array()),
                );
            }
        }
        self.hand.verts = hv;
        self.hand.indices = hi;
        self.hand.item3d_verts = iv;
        self.hand.model_scratch_verts = tv;
        self.hand.model_scratch_indices = ti;
    }

    /// Bake every dynamic world subsystem (item-entity, item-model-entity, chest,
    /// door, mob, break, particle) for this frame, in the order that reuses the
    /// shared item-entity scratch. Extracted verbatim from `render`.
    pub(super) fn bake_world_instances(&mut self) {
        let render_origin = self.view.render_origin;
        let visible_world_aabb = |min: glam::Vec3, max: glam::Vec3| {
            self.view
                .frustum
                .aabb_visible(min - render_origin, max - render_origin)
        };
        // Bake the dynamic world subsystems. Item-entity, chest, and break-overlay
        // each clear-and-refill the SAME shared CPU scratch (`item_entity_verts` /
        // `item_entity_indices`) in this exact order — `bake` (clear count → build
        // → grow → upload to that subsystem's OWN buffers → store count) runs
        // sequentially, never aliasing two GPU buffers at once.

        // Item entities (spinning cubes / extruded sprite slabs), frustum-culled
        // so off-screen drops cost nothing. Cubes ride the EXISTING opaque
        // pipeline; sprites bake below into their explicit-UV stream.
        self.item_entity.visible.clear();
        for inst in &self.item_entity.instances {
            // ~0.5 m cull box around the item centre.
            let c = inst.pos;
            let min = c - glam::Vec3::splat(0.5);
            let max = c + glam::Vec3::new(0.5, 1.0, 0.5);
            if visible_world_aabb(min, max) {
                self.item_entity.visible.push(*inst);
            }
        }
        // Re-cull against THIS frame's camera, like the item entities above.
        // The gather already culled, but against the camera the game thread
        // published. Recorded as INDICES into the published list, not as a filtered copy
        // of it: the published list is state (narrowing it in place would make
        // the next frame's contents depend on where this frame's camera
        // pointed), but saying WHICH rows survived costs a `u32`, where
        // cloning them cost an atomic refcount pair and ~96 bytes each.
        //
        // The bound is the set's OWN, carried into world axes by its transform:
        // a set is authored in its block's footprint space and may reach a few
        // cells, and a hardcoded box big enough for the largest machine is a
        // cull that stops culling.
        let mut visible_draws = std::mem::take(&mut self.item_entity.block_draws_visible);
        visible_draws.clear();
        visible_draws.extend(
            self.item_entity
                .block_draws
                .iter()
                .enumerate()
                .filter(|(_, d)| match d.set.bounds {
                    None => false,
                    Some((lo, hi)) => {
                        let (mn, mx) = petramond::world::draw::world_bounds(&d.transform, lo, hi);
                        visible_world_aabb(mn.into(), mx.into())
                    }
                })
                .map(|(i, _)| i as u32),
        );
        let draws = crate::block_draw::VisibleDraws {
            all: &self.item_entity.block_draws,
            visible: &visible_draws,
        };
        let visible = &self.item_entity.visible;
        self.item_entity.draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.verts,
            &mut self.item_entity.indices,
            |verts, indices| {
                // Mod draw sets share this stream: same atlas, same pipeline,
                // rebuilt from scratch every frame like everything else in it.
                // A second producer means the closure's index count is the
                // BUFFER's length, not the first builder's return — the
                // builders each report only their own share.
                build_item_entities(visible, verts, indices);
                crate::block_draw::build_block_draws(draws, verts, indices);
                indices.len() as u32
            },
        );
        // Dropped bbmodel items (their own model atlas), baked from the same visible set.
        let visible = &self.item_entity.visible;
        let env = self.light_env();
        self.item_entity.model_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.model_verts,
            &mut self.item_entity.model_indices,
            |verts, indices| {
                // Two producers: the count is the buffer's (see above).
                crate::item_entity::build_item_model_entities(visible, env, verts, indices);
                crate::block_draw::build_block_draw_models(draws, env, verts, indices);
                indices.len() as u32
            },
        );
        // Dropped sprite items as extruded pixel-perfect 3D slabs (block atlas,
        // explicit-UV stream), spinning + bobbing like the cubes above.
        let visible = &self.item_entity.visible;
        let mut sprite_scratch = std::mem::take(&mut self.item_entity.sprite_scratch);
        self.item_entity.sprite_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.sprite_verts,
            &mut self.item_entity.sprite_indices,
            |verts, indices| {
                // Two producers: the count is the buffer's (see above).
                crate::item_entity::build_item_sprite_entities(
                    visible,
                    env,
                    &mut sprite_scratch,
                    verts,
                    indices,
                );
                crate::block_draw::build_block_draw_sprites(
                    draws,
                    env,
                    &mut sprite_scratch,
                    verts,
                    indices,
                );
                indices.len() as u32
            },
        );
        self.item_entity.sprite_scratch = sprite_scratch;
        self.item_entity.block_draws_visible = visible_draws;

        // Chests (inset body + hinged lid), frustum-culled like item entities and
        // reusing their CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.block_entity.chest_visible.clear();
        for inst in &self.block_entity.chests {
            // Cull box: the block cell, expanded upward to include the open lid.
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.block_entity.chest_visible.push(*inst);
            }
        }
        let chest_visible = &self.block_entity.chest_visible;
        self.block_entity.chest_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.verts,
            &mut self.item_entity.indices,
            |verts, indices| build_chests(chest_visible, verts, indices),
        );

        // Doors (2-tall hinged slab), frustum-culled and baked exactly like chests,
        // reusing the same CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.block_entity.door_visible.clear();
        for inst in &self.block_entity.doors {
            // Cull box: the door's two-cell column (its swung slab stays within it).
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.block_entity.door_visible.push(*inst);
            }
        }
        let door_visible = &self.block_entity.door_visible;
        self.block_entity.door_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.verts,
            &mut self.item_entity.indices,
            |verts, indices| build_doors(door_visible, verts, indices),
        );

        // Mobs (animated entity models), grouped by species and frustum-culled, baked
        // into each species' OWN `ItemVertex` buffers (a different vertex type from the
        // packed block vertex). Each instance is posed by the walk animation at its
        // `anim_time` when moving, else the model's rest pose.
        for g in &mut self.actor.mob_gpu {
            g.visible.clear();
        }
        for inst in &self.actor.mobs {
            // Cull box: the species' rest-pose bounds × scale + slack around the
            // feet (`MobGpu::cull_*`, computed at construction) — a hardcoded pad
            // clipped every species taller than it. A killed mob is flung from its
            // (frozen) death point and tumbles across the ground, so use a generous
            // box while it's ragdolling so the flying corpse doesn't pop out of view.
            let g = &self.actor.mob_gpu[inst.kind.0 as usize];
            let (min, max) = if inst.ragdoll.is_some() {
                let pad = glam::Vec3::splat(6.0);
                (inst.pos - pad, inst.pos + pad)
            } else {
                (
                    inst.pos + glam::Vec3::new(-g.cull_r, g.cull_y0, -g.cull_r),
                    inst.pos + glam::Vec3::new(g.cull_r, g.cull_y1, g.cull_r),
                )
            };
            if visible_world_aabb(min, max) {
                self.actor.mob_gpu[inst.kind.0 as usize]
                    .visible
                    .push(inst.clone());
            }
        }
        let (device, queue) = (&self.device, &self.queue);
        for g in &mut self.actor.mob_gpu {
            let model = g.model;
            let scale = g.scale;
            let visible = &g.visible;
            g.draw.bake(
                device,
                queue,
                &mut g.verts,
                &mut g.indices,
                |verts, indices| build_mob_instances(model, scale, env, visible, verts, indices),
            );
        }

        // Player bodies + their held items: the LOCAL third-person body (when
        // the view is up, animated by the renderer's own first-person
        // HeldItemView — unchanged solo behavior) plus EVERY remote player
        // (each carrying its own replicated HeldItemView), frustum-culled like
        // mobs and ALL appended into the one player_gpu vertex/index stream
        // (every body shares the player model + skin bind). Held items
        // accumulate per render kind into three combined streams — block
        // mini-cubes on the packed opaque stream, extruded sprites and bbmodel
        // items on explicit-UV streams split by atlas — each uploaded and
        // drawn once regardless of player count.
        self.actor.player_visible.clear();
        {
            let pad = glam::Vec3::new(1.0, 2.2, 1.0);
            if let Some(p) = self.actor.player_view {
                if visible_world_aabb(p.pos - pad, p.pos + pad) {
                    self.actor
                        .player_visible
                        .push((p, self.hand.held_item, self.hand.off_item));
                }
            }
            for r in &self.actor.remote_players {
                if visible_world_aabb(r.body.pos - pad, r.body.pos + pad) {
                    self.actor.player_visible.push((r.body, r.held, r.held_off));
                }
            }
        }
        // Combined streams + per-body scratch taken out so the loop below can
        // borrow them alongside `self` reads (restored after the uploads).
        let mut body_verts = std::mem::take(&mut self.actor.player_gpu.verts);
        let mut body_indices = std::mem::take(&mut self.actor.player_gpu.indices);
        let mut sprite_verts = std::mem::take(&mut self.actor.item_verts);
        let mut sprite_indices = std::mem::take(&mut self.actor.item_indices);
        let mut model_verts = std::mem::take(&mut self.actor.model_item_verts);
        let mut model_indices = std::mem::take(&mut self.actor.model_item_indices);
        let mut block_verts = std::mem::take(&mut self.item_entity.verts);
        let mut block_indices = std::mem::take(&mut self.item_entity.indices);
        let mut scratch_verts = std::mem::take(&mut self.actor.body_verts);
        let mut scratch_indices = std::mem::take(&mut self.actor.body_indices);
        let mut sprite_scratch = std::mem::take(&mut self.actor.sprite_verts);
        body_verts.clear();
        body_indices.clear();
        sprite_verts.clear();
        sprite_indices.clear();
        model_verts.clear();
        model_indices.clear();
        block_verts.clear();
        block_indices.clear();
        let model = self.actor.player_gpu.model;
        for (inst, held, off) in &self.actor.player_visible {
            // The builder clears its buffers, so each body bakes into the
            // scratch and appends with a base-vertex offset.
            let (_, hand, off_hand) = crate::player_model::build_player_body(
                model,
                env,
                inst,
                inst.bones.of(&self.actor.bone_offsets),
                held,
                off,
                &mut scratch_verts,
                &mut scratch_indices,
            );
            // claimed poses ride their own per-hand attach frames (the off
            // frame mirrors the pose, lefthand-style), upstream of the
            // per-render-kind transforms below so every kind wears them.
            let hand = crate::player_model::posed_hand(hand, &held.pose.third_person, false);
            let off_hand = crate::player_model::posed_hand(off_hand, &off.pose.third_person, true);
            let base = body_verts.len() as u32;
            body_verts.extend_from_slice(&scratch_verts);
            body_indices.extend(scratch_indices.iter().map(|&i| i + base));

            let light = crate::lighting::DynLight::new(inst.skylight, inst.blocklight);
            // A sleeper's hands are empty — the held items would poke through
            // the bed. Each hand emits its own item with its own attach
            // transforms (the off set is the mirrored twin).
            for (view, hand_mat, off_side) in [(held, hand, false), (off, off_hand, true)] {
                let item = (!inst.sleeping).then_some(view.item).flatten();
                match item.map(|it| it.render_kind()) {
                    Some(petramond_world::item::ItemRenderKind::BlockCube(block)) => {
                        let m = if off_side {
                            crate::player_model::held_block_transform_off(hand_mat)
                        } else {
                            crate::player_model::held_block_transform(hand_mat)
                        };
                        let start = block_verts.len();
                        if block == petramond_world::block::Block::Chest {
                            crate::chest_model::push_chest_item(
                                &mut block_verts,
                                &mut block_indices,
                                glam::Vec3::splat(-0.5),
                                1.0,
                                light,
                            );
                        } else {
                            crate::item_cube::push_block_item_cube_lit_with_state(
                                &mut block_verts,
                                &mut block_indices,
                                block,
                                view.block_state,
                                glam::Vec3::splat(-0.5),
                                1.0,
                                light,
                                false,
                            );
                        }
                        // Instance-data tint on the held mini-cube (dyed wool
                        // in a remote or third-person hand).
                        crate::item_model::dye_block_verts(&mut block_verts[start..], view.variant);
                        crate::player_model::transform_positions(
                            block_verts[start..].iter_mut().map(|v| &mut v.pos),
                            m,
                        );
                    }
                    Some(petramond_world::item::ItemRenderKind::Sprite(tile)) => {
                        // The extrusion clears its buffer and emits a non-indexed
                        // triangle list; transform in place, then append with
                        // sequential offset indices to ride the indexed draw.
                        let m = if off_side {
                            crate::player_model::held_sprite_transform_off(hand_mat)
                        } else {
                            crate::player_model::held_sprite_transform(hand_mat)
                        };
                        let count = crate::item_model::build_extruded_stack_lit(
                            tile,
                            view.variant,
                            light,
                            env,
                            &mut sprite_scratch,
                        );
                        crate::player_model::transform_positions(
                            sprite_scratch.iter_mut().map(|v| &mut v.pos),
                            m,
                        );
                        let base = sprite_verts.len() as u32;
                        sprite_verts.extend_from_slice(&sprite_scratch);
                        sprite_indices.extend((0..count).map(|i| i + base));
                    }
                    Some(petramond_world::item::ItemRenderKind::Model(kind)) => {
                        // Appends with absolute indices into the shared buffer.
                        let m = if off_side {
                            crate::player_model::held_model_transform_off(hand_mat, kind)
                        } else {
                            crate::player_model::held_model_transform(hand_mat, kind)
                        };
                        crate::item_model::build_block_model_item(
                            kind,
                            m,
                            light,
                            env,
                            None,
                            &mut model_verts,
                            &mut model_indices,
                        );
                    }
                    None => {}
                }
            }
        }
        // Upload the four combined streams (a stream that stayed empty draws
        // nothing).
        let prebuilt = |_: &mut Vec<_>, i: &mut Vec<u32>| i.len() as u32;
        self.actor.player_gpu.draw.bake(
            &self.device,
            &self.queue,
            &mut body_verts,
            &mut body_indices,
            prebuilt,
        );
        self.actor.item_draw.bake(
            &self.device,
            &self.queue,
            &mut sprite_verts,
            &mut sprite_indices,
            prebuilt,
        );
        self.actor.model_item_draw.bake(
            &self.device,
            &self.queue,
            &mut model_verts,
            &mut model_indices,
            prebuilt,
        );
        self.actor.block_item_draw.bake(
            &self.device,
            &self.queue,
            &mut block_verts,
            &mut block_indices,
            |_: &mut Vec<_>, i: &mut Vec<u32>| i.len() as u32,
        );
        self.actor.player_gpu.verts = body_verts;
        self.actor.player_gpu.indices = body_indices;
        self.actor.item_verts = sprite_verts;
        self.actor.item_indices = sprite_indices;
        self.actor.model_item_verts = model_verts;
        self.actor.model_item_indices = model_indices;
        self.item_entity.verts = block_verts;
        self.item_entity.indices = block_indices;
        self.actor.body_verts = scratch_verts;
        self.actor.body_indices = scratch_indices;
        self.actor.sprite_verts = sprite_scratch;

        // Break-overlay (destroy crack) geometry: ONE combined stream over
        // every active overlay (the local miner's own + every remote's),
        // each baked exactly like the single overlay always was.
        let break_overlays = std::mem::take(&mut self.hand.break_overlays);
        self.hand.break_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.verts,
            &mut self.item_entity.indices,
            |verts, indices| build_break_overlays(&break_overlays, verts, indices),
        );
        self.hand.break_overlays = break_overlays;

        // Tiny 3D particle cubes into the reusable vbuf (static cube ibuf): block-atlas
        // flecks first, then bbmodel-block (model-atlas) flecks, so the draw splits at one
        // contiguous index boundary (`particle_block_vertex_count`).
        let particles = &self.particle.instances;
        let model_particles = &self.particle.model_instances;
        let mut block_v = 0u32;
        self.particle.draw.bake(
            &self.device,
            &self.queue,
            &mut self.particle.verts,
            |verts| {
                let (total, nb) = build_particles_split(particles, model_particles, env, verts);
                block_v = nb;
                total
            },
        );
        self.particle.block_vertex_count = if self.particle.draw.vertex_count == 0 {
            0
        } else {
            block_v
        };

        // Particle emitters (torch flames, mod content, burning mobs). The set
        // arrives already culled against this frame's view volume — the gather
        // holds the same frustum and fog distance published by `update_uniforms`
        // above — so all that is left is the far-to-near order the alpha-blended
        // cubes are built in.
        let cam_pos = self.view.cam_pos;
        self.particle.emitters.sort_by(|a, b| {
            let da = (a.origin - cam_pos).length_squared();
            let db = (b.origin - cam_pos).length_squared();
            da.total_cmp(&db)
        });
        let emitters = &self.particle.emitters;
        let solids = &self.particle.solid_instances;
        let time = self.view.visual_time;
        let density = self.particle.density;
        self.particle.emitter_draw.bake(
            &self.device,
            &self.queue,
            &mut self.particle.emitter_verts,
            |verts| {
                build_transparent_emitter_particles(
                    emitters,
                    solids,
                    time,
                    cam_pos,
                    env,
                    density,
                    verts,
                    &mut self.particle.emitter_scratch,
                )
            },
        );

        // Entity blob shadows: the rows arrive ground-resolved + view-culled
        // from the gather, so this is just the quad bake.
        let shadows = std::mem::take(&mut self.shadow.instances);
        self.shadow
            .draw
            .bake(&self.device, &self.queue, &mut self.shadow.verts, |verts| {
                build_entity_shadows(&shadows, verts)
            });
        self.shadow.instances = shadows;
    }
}
