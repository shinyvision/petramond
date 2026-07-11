//! Per-frame dynamic CPU geometry bakes for [`Renderer`], lifted verbatim out
//! of `render`'s prologue. Three `&mut self` steps run before encoding:
//! overlay-buffer refresh, held-item geometry, and the world-instance bakes.
//! Behavior, ordering, borrow/scratch-reuse patterns are byte-for-byte identical.

use super::*;

impl Renderer {
    /// The frame's CPU lighting environment (sky scale + colour), mirroring the
    /// shader uniform lanes for the explicit-shade dynamic bakes.
    #[inline]
    fn light_env(&self) -> crate::render::lighting::LightEnv {
        crate::render::lighting::LightEnv {
            sky_scale: self.sky_scale,
            sky_color: self.sky_color,
        }
    }

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

    /// The hurt-shake as a clip-space post-transform: left-multiplying a
    /// translation adds `t * w` to the clip position, which after the divide is
    /// exactly an NDC screen shift — the whole hand jitters without touching
    /// any pose math.
    fn hand_shake_mat(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(glam::Vec3::new(self.hand_shake[0], self.hand_shake[1], 0.0))
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
            let mvp = self.hand_shake_mat()
                * build_hand_lit(
                    &self.held_item,
                    aspect,
                    crate::render::lighting::DynLight {
                        sky: self.held_item_skylight,
                        block: self.held_item_blocklight,
                    },
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
            if let Some((kind, mvp)) = crate::render::hand::held_model(&self.held_item, aspect)
                .map(|(kind, mvp)| (kind, self.hand_shake_mat() * mvp))
            {
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
                    crate::render::lighting::DynLight {
                        sky: self.held_item_skylight,
                        block: self.held_item_blocklight,
                    },
                    self.light_env(),
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
                    .map(|(tile, mvp)| (tile, self.hand_shake_mat() * mvp))
            {
                let mut iv = std::mem::take(&mut self.item3d_verts);
                let count = crate::render::item_model::build_extruded_item_lit(
                    tile,
                    crate::render::lighting::DynLight {
                        sky: self.held_item_skylight,
                        block: self.held_item_blocklight,
                    },
                    self.light_env(),
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
        let env = self.light_env();
        self.item_model_entity_draw.bake(
            &self.queue,
            &mut self.item_model_entity_verts,
            &mut self.item_model_entity_indices,
            |verts, indices| {
                crate::render::item_entity::build_item_model_entities(visible, env, verts, indices)
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
                self.mob_gpu[inst.kind.0 as usize]
                    .visible
                    .push(inst.clone());
            }
        }
        let queue = &self.queue;
        for g in &mut self.mob_gpu {
            let model = g.model;
            let scale = g.scale;
            let visible = &g.visible;
            g.draw
                .bake(queue, &mut g.verts, &mut g.indices, |verts, indices| {
                    build_mob_instances(model, scale, env, visible, verts, indices)
                });
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
        self.player_visible.clear();
        {
            let pad = glam::Vec3::new(1.0, 2.2, 1.0);
            if let Some(p) = self.player_view {
                if visible_world_aabb(p.pos - pad, p.pos + pad) {
                    self.player_visible.push((p, self.held_item));
                }
            }
            for r in &self.remote_players {
                if visible_world_aabb(r.body.pos - pad, r.body.pos + pad) {
                    self.player_visible.push((r.body, r.held));
                }
            }
        }
        // Combined streams + per-body scratch taken out so the loop below can
        // borrow them alongside `self` reads (restored after the uploads).
        let mut body_verts = std::mem::take(&mut self.player_gpu.verts);
        let mut body_indices = std::mem::take(&mut self.player_gpu.indices);
        let mut sprite_verts = std::mem::take(&mut self.player_item_verts);
        let mut sprite_indices = std::mem::take(&mut self.player_item_indices);
        let mut model_verts = std::mem::take(&mut self.player_model_item_verts);
        let mut model_indices = std::mem::take(&mut self.player_model_item_indices);
        let mut block_verts = std::mem::take(&mut self.item_entity_verts);
        let mut block_indices = std::mem::take(&mut self.item_entity_indices);
        let mut scratch_verts = std::mem::take(&mut self.player_body_verts);
        let mut scratch_indices = std::mem::take(&mut self.player_body_indices);
        let mut sprite_scratch = std::mem::take(&mut self.player_sprite_verts);
        body_verts.clear();
        body_indices.clear();
        sprite_verts.clear();
        sprite_indices.clear();
        model_verts.clear();
        model_indices.clear();
        block_verts.clear();
        block_indices.clear();
        let model = self.player_gpu.model;
        for (inst, held) in &self.player_visible {
            // The builder clears its buffers, so each body bakes into the
            // scratch and appends with a base-vertex offset.
            let (_, hand) = crate::render::player_model::build_player_body(
                model,
                env,
                inst,
                held.swing,
                held.swing_scale,
                held.eat,
                held.eat_bob,
                &mut scratch_verts,
                &mut scratch_indices,
            );
            let base = body_verts.len() as u32;
            body_verts.extend_from_slice(&scratch_verts);
            body_indices.extend(scratch_indices.iter().map(|&i| i + base));

            let light = crate::render::lighting::DynLight {
                sky: inst.skylight,
                block: inst.blocklight,
            };
            // A sleeper's hands are empty — the held item would poke through the bed.
            let held_item = (!inst.sleeping).then_some(held.item).flatten();
            match held_item.map(|it| it.render_kind()) {
                Some(crate::item::ItemRenderKind::BlockCube(block)) => {
                    let m = crate::render::player_model::held_block_transform(hand);
                    let start = block_verts.len();
                    if block == crate::block::Block::Chest {
                        crate::render::chest_model::push_chest_item(
                            &mut block_verts,
                            &mut block_indices,
                            glam::Vec3::splat(-0.5),
                            1.0,
                            light,
                        );
                    } else {
                        crate::render::item_cube::push_block_item_cube_lit_with_state(
                            &mut block_verts,
                            &mut block_indices,
                            block,
                            held.block_state,
                            glam::Vec3::splat(-0.5),
                            1.0,
                            light,
                        );
                    }
                    crate::render::player_model::transform_positions(&mut block_verts, start, m);
                }
                Some(crate::item::ItemRenderKind::Sprite(tile)) => {
                    // The extrusion clears its buffer and emits a non-indexed
                    // triangle list; transform in place, then append with
                    // sequential offset indices to ride the indexed draw.
                    let m = crate::render::player_model::held_sprite_transform(hand);
                    let count = crate::render::item_model::build_extruded_item_lit(
                        tile,
                        light,
                        env,
                        &mut sprite_scratch,
                    );
                    crate::render::player_model::transform_item_positions(
                        &mut sprite_scratch,
                        0,
                        m,
                    );
                    let base = sprite_verts.len() as u32;
                    sprite_verts.extend_from_slice(&sprite_scratch);
                    sprite_indices.extend((0..count).map(|i| i + base));
                }
                Some(crate::item::ItemRenderKind::Model(kind)) => {
                    // Appends with absolute indices into the shared buffer.
                    let m = crate::render::player_model::held_model_transform(hand, kind);
                    crate::render::item_model::build_block_model_item(
                        kind,
                        m,
                        light,
                        env,
                        0,
                        None,
                        &mut model_verts,
                        &mut model_indices,
                    );
                }
                None => {}
            }
        }
        // Upload the four combined streams (a stream that stayed empty draws
        // nothing; over-cap frames drop that stream, per DynamicDraw).
        let prebuilt = |_: &mut Vec<_>, i: &mut Vec<u32>| i.len() as u32;
        self.player_gpu
            .draw
            .bake(&self.queue, &mut body_verts, &mut body_indices, prebuilt);
        self.player_item_draw.bake(
            &self.queue,
            &mut sprite_verts,
            &mut sprite_indices,
            prebuilt,
        );
        self.player_model_item_draw.bake(
            &self.queue,
            &mut model_verts,
            &mut model_indices,
            prebuilt,
        );
        self.player_block_item_draw.bake(
            &self.queue,
            &mut block_verts,
            &mut block_indices,
            |_: &mut Vec<_>, i: &mut Vec<u32>| i.len() as u32,
        );
        self.player_gpu.verts = body_verts;
        self.player_gpu.indices = body_indices;
        self.player_item_verts = sprite_verts;
        self.player_item_indices = sprite_indices;
        self.player_model_item_verts = model_verts;
        self.player_model_item_indices = model_indices;
        self.item_entity_verts = block_verts;
        self.item_entity_indices = block_indices;
        self.player_body_verts = scratch_verts;
        self.player_body_indices = scratch_indices;
        self.player_sprite_verts = sprite_scratch;

        // Break-overlay (destroy crack) geometry: ONE combined stream over
        // every active overlay (the local miner's own + the capped remotes),
        // each baked exactly like the single overlay always was.
        let break_overlays = std::mem::take(&mut self.break_overlays);
        self.break_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_break_overlays(&break_overlays, verts, indices),
        );
        self.break_overlays = break_overlays;

        // Tiny 3D particle cubes into the reusable vbuf (static cube ibuf): block-atlas
        // flecks first, then bbmodel-block (model-atlas) flecks, so the draw splits at one
        // contiguous index boundary (`particle_block_vertex_count`).
        let particles = &self.particles;
        let model_particles = &self.model_particles;
        let mut block_v = 0u32;
        self.particle_draw.bake(
            &self.device,
            &self.queue,
            &mut self.particle_verts,
            |verts| {
                let (total, nb) = build_particles_split(particles, model_particles, env, verts);
                block_v = nb;
                total
            },
        );
        self.particle_block_vertex_count = if self.particle_draw.vertex_count == 0 {
            0
        } else {
            block_v
        };

        // Block-row particle emitters (torch flames and mod-content emitters): cull the
        // emitter envelope first, then synthesize alpha-blended cubes sorted far-to-near.
        self.particle_emitter_visible.clear();
        let fog = self.terrain_cull_dist();
        let fog_sq = fog * fog;
        // Particles option "off": no looping-emitter particles at all (burst
        // solids are silenced at their game-side spawn, the same option).
        let emitters_enabled = self.particle_density > 0.0;
        for inst in &self.particle_emitters {
            if !emitters_enabled {
                break;
            }
            let (min, max) = emitter_world_bounds(inst);
            if !visible_world_aabb(min, max) {
                continue;
            }
            if aabb_distance_sq(self.cam_pos, min, max) > fog_sq {
                continue;
            }
            self.particle_emitter_visible.push(*inst);
        }
        self.particle_emitter_visible.sort_by(|a, b| {
            let da = (a.origin - self.cam_pos).length_squared();
            let db = (b.origin - self.cam_pos).length_squared();
            da.total_cmp(&db)
        });
        let emitters = &self.particle_emitter_visible;
        let solids = &self.solid_particles;
        let time = self.visual_time;
        let cam_pos = self.cam_pos;
        let density = self.particle_density;
        self.emitter_particle_draw.bake(
            &self.device,
            &self.queue,
            &mut self.emitter_particle_verts,
            |verts| {
                build_transparent_emitter_particles(
                    emitters,
                    solids,
                    time,
                    cam_pos,
                    env,
                    density,
                    verts,
                    &mut self.emitter_particle_scratch,
                )
            },
        );
    }
}

fn emitter_world_bounds(inst: &ParticleEmitterInstance) -> (glam::Vec3, glam::Vec3) {
    let e = inst.emitter;
    let max_life = e.lifetime[1].max(e.lifetime[0]);
    let max_size = e.size[1].max(e.size[0]);
    let velocity = glam::Vec3::from_array(e.velocity);
    let jitter = glam::Vec3::from_array(e.velocity_jitter);
    let travel = glam::Vec3::new(
        velocity.x.abs() + jitter.x,
        velocity.y.abs() + jitter.y,
        velocity.z.abs() + jitter.z,
    ) * max_life;
    let spawn = glam::Vec3::from_array(e.spawn_box);
    let extent = spawn + travel + glam::Vec3::splat(max_size + 0.05);
    (inst.origin - extent, inst.origin + extent)
}
