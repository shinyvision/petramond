//! Per-frame dynamic CPU geometry bakes for [`Renderer`], lifted verbatim out
//! of `render`'s prologue. Three `&mut self` steps run before encoding:
//! overlay-buffer refresh, held-item geometry, and the world-instance bakes.
//! Behavior, ordering, borrow/scratch-reuse patterns are byte-for-byte identical.

use super::*;

impl Renderer {
    /// The frame's CPU lighting environment (sky scale + colour), mirroring the
    /// shader uniform lanes for the explicit-shade dynamic bakes.
    #[inline]
    pub(super) fn light_env(&self) -> crate::lighting::LightEnv {
        crate::lighting::LightEnv {
            sky_scale: self.sky.scale,
            sky_color: self.sky.color,
        }
    }

    /// The two-channel light sampled at the local player, lighting every
    /// held-item variant.
    #[inline]
    pub(super) fn held_item_light(&self) -> crate::lighting::DynLight {
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

        // Refresh the outline vertex buffer only when the target (or the render
        // origin its vertices are relative to) changed.
        let origin = self.view.render_origin;
        let wanted = self.chrome.selection.map(|shape| (shape, origin));
        if wanted != self.chrome.selection_drawn {
            self.chrome.outline_vertex_count = 0;
            if let Some(shape) = self.chrome.selection {
                let outline = outline_vertices(shape, origin);
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
            self.chrome.selection_drawn = wanted;
        }
    }

    /// Bake every dynamic world subsystem (item-entity, item-model-entity, chest,
    /// door, mob, break, particle) for this frame, in the order that reuses the
    /// shared item-entity scratch. Extracted verbatim from `render`.
    pub(super) fn bake_world_instances(&mut self) {
        let render_origin = self.view.render_origin;
        let visible_world_aabb =
            |min: petramond_math::world_pos::WorldPos, max: petramond_math::world_pos::WorldPos| {
                self.view.frustum.aabb_visible(
                    min.relative_to(render_origin),
                    max.relative_to(render_origin),
                )
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
                        let (mn, mx) = petramond::world::draw::world_bounds(&d.frame, lo, hi);
                        visible_world_aabb(mn, mx)
                    }
                })
                .map(|(i, _)| i as u32),
        );
        let draws = crate::block_draw::VisibleDraws {
            all: &self.item_entity.block_draws,
            visible: &visible_draws,
            origin: render_origin,
            time: self.view.visual_time,
            eye: self.view.cam_pos.relative_to(render_origin),
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
                build_item_entities(visible, render_origin, verts, indices);
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
                crate::item_entity::build_item_model_entities(
                    visible,
                    render_origin,
                    env,
                    verts,
                    indices,
                );
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
                    render_origin,
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
            let min = petramond_math::world_pos::WorldPos::block_min(inst.pos);
            let max = min + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.block_entity.chest_visible.push(*inst);
            }
        }
        // Static geometry: rebake only when the visible set or the origin its
        // vertices are relative to actually changed (see `chest_baked`).
        let origin_moved = self.block_entity.baked_origin != render_origin;
        if origin_moved || self.block_entity.chest_visible != self.block_entity.chest_baked {
            let chest_visible = &self.block_entity.chest_visible;
            self.block_entity.chest_draw.bake(
                &self.device,
                &self.queue,
                &mut self.item_entity.verts,
                &mut self.item_entity.indices,
                |verts, indices| build_chests(chest_visible, render_origin, verts, indices),
            );
            self.block_entity.chest_baked.clear();
            self.block_entity
                .chest_baked
                .extend_from_slice(&self.block_entity.chest_visible);
        }

        // Doors (2-tall hinged slab), frustum-culled and baked exactly like chests,
        // reusing the same CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.block_entity.door_visible.clear();
        for inst in &self.block_entity.doors {
            // Cull box: the door's two-cell column (its swung slab stays within it).
            let min = petramond_math::world_pos::WorldPos::block_min(inst.pos);
            let max = min + glam::Vec3::new(1.0, 2.0, 1.0);
            if visible_world_aabb(min, max) {
                self.block_entity.door_visible.push(*inst);
            }
        }
        if origin_moved || self.block_entity.door_visible != self.block_entity.door_baked {
            let door_visible = &self.block_entity.door_visible;
            self.block_entity.door_draw.bake(
                &self.device,
                &self.queue,
                &mut self.item_entity.verts,
                &mut self.item_entity.indices,
                |verts, indices| build_doors(door_visible, render_origin, verts, indices),
            );
            self.block_entity.door_baked.clear();
            self.block_entity
                .door_baked
                .extend_from_slice(&self.block_entity.door_visible);
        }
        self.block_entity.baked_origin = render_origin;

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
        let mut mob_held = Vec::new();
        for g in &mut self.actor.mob_gpu {
            let model = g.model;
            let scale = g.scale;
            let visible = &g.visible;
            let rig = &g.rig;
            g.draw.bake(
                device,
                queue,
                &mut g.verts,
                &mut g.indices,
                |verts, indices| {
                    build_mob_instances(
                        model,
                        scale,
                        env,
                        visible,
                        render_origin,
                        verts,
                        indices,
                        &mut mob_held,
                        rig,
                    )
                },
            );
        }

        // Player bodies + their held items: the LOCAL third-person body (when
        // the view is up, driven by the renderer's own local hand state) plus
        // EVERY remote player (each carrying its own), frustum-culled like
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
                    self.actor.player_visible.push(super::VisibleBody {
                        inst: p,
                        held: self.hand.held_item,
                        off: self.hand.off_item,
                        key: crate::player_model::LOCAL_BODY,
                        frames: self.hand.frames,
                        animator: None,
                    });
                }
            }
            for r in &self.actor.remote_players {
                if visible_world_aabb(r.body.pos - pad, r.body.pos + pad) {
                    self.actor.player_visible.push(super::VisibleBody {
                        inst: r.body,
                        held: r.held,
                        off: r.held_off,
                        key: r.key,
                        frames: Some(r.frames),
                        animator: Some(r.animator),
                    });
                }
            }
        }
        // Combined streams + per-body scratch taken out so the loop below can
        // borrow them alongside `self` reads (restored after the uploads).
        let mut body_verts = std::mem::take(&mut self.actor.player_gpu.verts);
        let mut body_indices = std::mem::take(&mut self.actor.player_gpu.indices);
        let mut streams = HeldStreams {
            sprite_verts: std::mem::take(&mut self.actor.item_verts),
            sprite_indices: std::mem::take(&mut self.actor.item_indices),
            model_verts: std::mem::take(&mut self.actor.model_item_verts),
            model_indices: std::mem::take(&mut self.actor.model_item_indices),
            block_verts: std::mem::take(&mut self.item_entity.verts),
            block_indices: std::mem::take(&mut self.item_entity.indices),
            sprite_scratch: std::mem::take(&mut self.actor.sprite_verts),
        };
        let mut scratch_verts = std::mem::take(&mut self.actor.body_verts);
        let mut scratch_indices = std::mem::take(&mut self.actor.body_indices);
        body_verts.clear();
        body_indices.clear();
        streams.sprite_verts.clear();
        streams.sprite_indices.clear();
        streams.model_verts.clear();
        streams.model_indices.clear();
        streams.block_verts.clear();
        streams.block_indices.clear();
        let body_rig = petramond::player::rigs::presented(petramond::player::Presenter::Body)
            .map(|(_, rig)| rig);
        let dt = self.hand.frame_dt;
        self.actor
            .body_animators
            .retain(self.actor.remote_players.iter().map(|r| r.key));
        let local_inputs = crate::AnimatorInputs {
            params: &self.hand.local_params,
            plays: &self.hand.local_plays,
            events: &self.hand.local_events,
        };
        // Every roster body nobody draws this frame still advances, so what
        // it did off-screen is under way — never a stale edge — when it is
        // drawn again.
        for r in &self.actor.remote_players {
            if self.actor.player_visible.iter().any(|b| b.key == r.key) {
                continue;
            }
            if let Some(animator) = self.actor.body_animators.body(r.key) {
                let inputs = crate::AnimatorInputs {
                    params: r.animator.params.of(&self.actor.animator_params),
                    plays: r.animator.plays.of(&self.actor.animator_plays),
                    events: r.animator.events.of(&self.actor.animator_events),
                };
                animator.advance(Some(&r.body), Some(&r.frames), inputs, dt);
            }
        }
        let local_drawn = self
            .actor
            .player_visible
            .iter()
            .any(|b| b.key == crate::player_model::LOCAL_BODY);
        if !local_drawn {
            if let Some(animator) = self
                .actor
                .body_animators
                .body(crate::player_model::LOCAL_BODY)
            {
                animator.advance(
                    self.actor.player_view.as_ref(),
                    self.hand.frames.as_ref(),
                    local_inputs,
                    dt,
                );
            }
        }
        for body in &self.actor.player_visible {
            let Some(rig) = body_rig else { break };
            let (inst, held, off) = (&body.inst, &body.held, &body.off);
            let inputs = match body.animator {
                Some(ranges) => crate::AnimatorInputs {
                    params: ranges.params.of(&self.actor.animator_params),
                    plays: ranges.plays.of(&self.actor.animator_plays),
                    events: ranges.events.of(&self.actor.animator_events),
                },
                None => local_inputs,
            };
            let drive = self.actor.body_animators.body(body.key).map(|animator| {
                crate::player_model::BodyDrive {
                    animator,
                    frames: body.frames.as_ref(),
                    inputs,
                    dt,
                }
            });
            // The builder clears its buffers, so each body bakes into the
            // scratch and appends with a base-vertex offset.
            let (_, hand, off_hand) = crate::player_model::build_player_body(
                rig,
                env,
                inst,
                render_origin,
                inst.bones.of(&self.actor.bone_offsets),
                drive,
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
                let grip = if off_side {
                    crate::player_model::Grip::body_off(hand_mat)
                } else {
                    crate::player_model::Grip::body(hand_mat)
                };
                let Some(item) = (!inst.sleeping).then_some(view.item).flatten() else {
                    continue;
                };
                streams.push(
                    item,
                    view.variant,
                    view.block_state,
                    grip,
                    off_side,
                    light,
                    env,
                );
            }
        }
        for held in mob_held {
            streams.push(
                held.item,
                petramond_world::item::VariantId::NONE,
                petramond_world::block_state::HeldBlockState::None,
                held.grip,
                held.off_side,
                held.light,
                env,
            );
        }
        let HeldStreams {
            mut block_verts,
            mut block_indices,
            mut sprite_verts,
            mut sprite_indices,
            sprite_scratch,
            mut model_verts,
            mut model_indices,
        } = streams;
        // Edges, consumed by this bake: a redraw before the next
        // `set_local_animator` must not fire them again.
        self.hand.local_events.clear();
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
        // every active CELL-SHAPED overlay (the local miner's own + every
        // remote's), each baked exactly like the single overlay always was. A
        // cracked bbmodel block bakes nothing — the decal pass re-draws the
        // model's own triangles under its outline mask.
        let break_overlays = std::mem::take(&mut self.hand.break_overlays);
        self.model_break
            .upload(&self.queue, &break_overlays, render_origin);
        self.hand.break_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.verts,
            &mut self.item_entity.indices,
            |verts, indices| build_break_overlays(&break_overlays, render_origin, verts, indices),
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
                let (total, nb) =
                    build_particles_split(particles, model_particles, env, render_origin, verts);
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
                    render_origin,
                    cam_pos.relative_to(render_origin),
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
                build_entity_shadows(&shadows, render_origin, verts)
            });
        self.shadow.instances = shadows;
    }
}

/// The combined held-item streams of one frame, one per render kind, shared
/// by every body and mob holding something.
struct HeldStreams {
    block_verts: Vec<petramond_mesh::Vertex>,
    block_indices: Vec<u32>,
    sprite_verts: Vec<ItemVertex>,
    sprite_indices: Vec<u32>,
    sprite_scratch: Vec<ItemVertex>,
    model_verts: Vec<ItemVertex>,
    model_indices: Vec<u32>,
}

impl HeldStreams {
    /// Append one held item seated at `grip` (the off-hand twins mirror it).
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        item: petramond_world::item::ItemType,
        variant: petramond_world::item::VariantId,
        block_state: petramond_world::block_state::HeldBlockState,
        grip: crate::player_model::Grip,
        off_side: bool,
        light: crate::lighting::DynLight,
        env: crate::lighting::LightEnv,
    ) {
        match item.render_kind() {
            petramond_world::item::ItemRenderKind::BlockCube(block) => {
                let m = if off_side {
                    crate::player_model::held_block_off_at(grip)
                } else {
                    crate::player_model::held_block_at(grip)
                };
                let start = self.block_verts.len();
                if block == petramond_world::block::Block::Chest {
                    crate::chest_model::push_chest_item(
                        &mut self.block_verts,
                        &mut self.block_indices,
                        glam::Vec3::splat(-0.5),
                        1.0,
                        light,
                    );
                } else {
                    crate::item_cube::push_block_item_cube_lit_with_state(
                        &mut self.block_verts,
                        &mut self.block_indices,
                        block,
                        block_state,
                        glam::Vec3::splat(-0.5),
                        1.0,
                        light,
                        false,
                    );
                }
                // Instance-data tint on the held mini-cube (dyed wool in a
                // remote or third-person hand).
                crate::item_model::dye_block_verts(&mut self.block_verts[start..], variant);
                crate::player_model::transform_positions(
                    self.block_verts[start..].iter_mut().map(|v| &mut v.pos),
                    m,
                );
            }
            petramond_world::item::ItemRenderKind::Sprite(tile) => {
                // The extrusion clears its buffer and emits a non-indexed
                // triangle list; transform in place, then append with
                // sequential offset indices to ride the indexed draw.
                let m = if off_side {
                    crate::player_model::held_sprite_off_at(grip)
                } else {
                    crate::player_model::held_sprite_at(grip)
                };
                let count = crate::item_model::build_extruded_stack_lit(
                    tile,
                    variant,
                    light,
                    env,
                    &mut self.sprite_scratch,
                );
                crate::player_model::transform_positions(
                    self.sprite_scratch.iter_mut().map(|v| &mut v.pos),
                    m,
                );
                let base = self.sprite_verts.len() as u32;
                self.sprite_verts.extend_from_slice(&self.sprite_scratch);
                self.sprite_indices.extend((0..count).map(|i| i + base));
            }
            petramond_world::item::ItemRenderKind::Model(kind) => {
                // Appends with absolute indices into the shared buffer.
                let m = if off_side {
                    crate::player_model::held_model_off_at(grip, kind)
                } else {
                    crate::player_model::held_model_at(grip, kind)
                };
                crate::item_model::build_block_model_item(
                    kind,
                    m,
                    light,
                    env,
                    None,
                    &mut self.model_verts,
                    &mut self.model_indices,
                );
            }
        }
    }
}
