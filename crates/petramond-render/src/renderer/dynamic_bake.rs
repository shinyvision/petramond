use super::*;

impl Renderer {
    #[inline]
    pub(super) fn light_env(&self) -> crate::lighting::LightEnv {
        crate::lighting::LightEnv {
            sky_scale: self.sky.scale,
            sky_color: self.sky.color,
        }
    }

    #[inline]
    pub(super) fn held_item_light(&self) -> crate::lighting::DynLight {
        crate::lighting::DynLight::new(self.hand.held_item_skylight, self.hand.held_item_blocklight)
    }

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

    pub(super) fn bake_world_instances(&mut self) {
        let render_origin = self.view.render_origin;
        let visible_world_aabb =
            |min: petramond_math::world_pos::WorldPos, max: petramond_math::world_pos::WorldPos| {
                self.view.frustum.aabb_visible(
                    min.relative_to(render_origin),
                    max.relative_to(render_origin),
                )
            };

        self.item_entity.visible.clear();
        for inst in &self.item_entity.instances {
            let c = inst.pos;
            let min = c - glam::Vec3::splat(0.5);
            let max = c + glam::Vec3::new(0.5, 1.0, 0.5);
            if visible_world_aabb(min, max) {
                self.item_entity.visible.push(*inst);
            }
        }
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
        let visible = &self.item_entity.visible;
        let env = self.light_env();
        self.item_entity.model_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.model_verts,
            &mut self.item_entity.model_indices,
            |verts, indices| {
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
        let visible = &self.item_entity.visible;
        let (cloths, cloth_points) = (&self.item_entity.cloths, &self.item_entity.cloth_points);
        let cloth_texels = &self.item_entity.cloth_texels;
        let mut sprite_scratch = std::mem::take(&mut self.item_entity.sprite_scratch);
        self.item_entity.sprite_draw.bake(
            &self.device,
            &self.queue,
            &mut self.item_entity.sprite_verts,
            &mut self.item_entity.sprite_indices,
            |verts, indices| {
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
                crate::cloth::build_cloths(
                    cloths,
                    cloth_points,
                    cloth_texels,
                    render_origin,
                    env,
                    verts,
                    indices,
                );
                indices.len() as u32
            },
        );
        self.item_entity.sprite_scratch = sprite_scratch;
        self.item_entity.block_draws_visible = visible_draws;

        self.block_entity.visible.clear();
        for inst in &self.block_entity.instances {
            let Some(model) = inst.block.animated_model() else {
                continue;
            };
            let cull = model.variant(inst.variant).cull;
            let cell = petramond_math::world_pos::WorldPos::block_min(inst.pos);
            let (min, max) = (glam::Vec3::from(cull.min), glam::Vec3::from(cull.max));
            if visible_world_aabb(cell + min, cell + max) {
                self.block_entity.visible.push(*inst);
            }
        }
        if self.block_entity.baked_origin != render_origin
            || self.block_entity.visible != self.block_entity.baked
        {
            let visible = &self.block_entity.visible;
            self.block_entity.draw.bake(
                &self.device,
                &self.queue,
                &mut self.item_entity.verts,
                &mut self.item_entity.indices,
                |verts, indices| {
                    verts.clear();
                    indices.clear();
                    push_block_entities(visible, render_origin, verts, indices);
                    indices.len() as u32
                },
            );
            self.block_entity.baked.clear();
            self.block_entity
                .baked
                .extend_from_slice(&self.block_entity.visible);
            self.block_entity.baked_origin = render_origin;
        }

        for g in &mut self.actor.mob_gpu {
            g.visible.clear();
        }
        for (i, inst) in self.actor.mobs.iter().enumerate() {
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
                    .push(i as u32);
            }
        }
        self.actor.skin.batch.clear();
        let mut mob_held = Vec::new();
        let layers = crate::mob_model::MobLayers {
            arena: &self.actor.mob_arena,
            names: &self.actor.anim_names,
        };
        let mobs = &self.actor.mobs;
        for g in &mut self.actor.mob_gpu {
            g.drawn = pose_mob_instances(
                crate::mob_model::MobPoseSpecies {
                    model: g.model,
                    scale: g.scale,
                    rig: &g.rig,
                    cache: &mut g.pose,
                },
                g.visible.iter().map(|&i| &mobs[i as usize]),
                layers,
                render_origin,
                &mut self.actor.skin.batch,
                &mut mob_held,
            );
        }

        // The local third-person body and every remote player are culled like mobs and share one
        // skin batch range, since they share the rig's mesh and bind. Held items batch per render
        // kind, so each stream is uploaded and drawn once regardless of player count.
        self.actor.player_visible.clear();
        {
            let pad = glam::Vec3::new(1.0, 2.2, 1.0);
            for body in &self.actor.bodies {
                let pos = body.body.pos;
                if visible_world_aabb(pos - pad, pos + pad) {
                    self.actor.player_visible.push(*body);
                }
            }
        }
        let mut streams = HeldStreams {
            sprite_verts: std::mem::take(&mut self.actor.item_verts),
            sprite_indices: std::mem::take(&mut self.actor.item_indices),
            model_verts: std::mem::take(&mut self.actor.model_item_verts),
            model_indices: std::mem::take(&mut self.actor.model_item_indices),
            block_verts: std::mem::take(&mut self.item_entity.verts),
            block_indices: std::mem::take(&mut self.item_entity.indices),
            sprite_scratch: std::mem::take(&mut self.actor.sprite_verts),
        };
        streams.sprite_verts.clear();
        streams.sprite_indices.clear();
        streams.model_verts.clear();
        streams.model_indices.clear();
        streams.block_verts.clear();
        streams.block_indices.clear();
        let body_rig = petramond::player::rigs::presented(petramond::player::Presenter::Body)
            .map(|(_, rig)| rig);
        let bodies_first = self.actor.skin.batch.next_instance();
        for body in &self.actor.player_visible {
            let Some(rig) = body_rig else { break };
            let (inst, held, off) = (&body.body, &body.held, &body.held_off);
            let (hand, off_hand) = crate::player_model::place_player_body(
                rig,
                inst,
                inst.pose.of(&self.actor.body_poses),
                render_origin,
                &mut self.actor.skin.batch,
            );
            let hand = crate::player_model::posed_hand(hand, &held.pose.third_person, false);
            let off_hand = crate::player_model::posed_hand(off_hand, &off.pose.third_person, true);

            let light = crate::lighting::DynLight::new(inst.skylight, inst.blocklight);
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
                    view.hold.third_person,
                    view.variant,
                    view.block_state,
                    grip,
                    off_side,
                    light,
                    env,
                );
            }
        }
        self.actor.player_gpu.drawn = bodies_first..self.actor.skin.batch.next_instance();
        self.actor.skin.upload(&self.device, &self.queue);
        for held in mob_held {
            streams.push(
                held.item,
                held.item.held_pose().third_person,
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
        let prebuilt = |_: &mut Vec<_>, i: &mut Vec<u32>| i.len() as u32;
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
        self.actor.item_verts = sprite_verts;
        self.actor.item_indices = sprite_indices;
        self.actor.model_item_verts = model_verts;
        self.actor.model_item_indices = model_indices;
        self.item_entity.verts = block_verts;
        self.item_entity.indices = block_indices;
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

        // Tiny 3D particle cubes: one instance row each (the vertex stage expands
        // the cube over the static cube pattern). Block-atlas flecks first, then
        // bbmodel-block (model-atlas) flecks, so the draw splits at one contiguous
        // instance boundary (`block_count`).
        let particles = &self.particle.instances;
        let model_particles = &self.particle.model_instances;
        let mut block_rows = 0u32;
        self.particle
            .draw
            .bake(&self.device, &self.queue, &mut self.particle.rows, |rows| {
                let (total, block) =
                    build_particles_split(particles, model_particles, env, render_origin, rows);
                block_rows = block;
                total
            });
        self.particle.block_count = if self.particle.draw.instance_count == 0 {
            0
        } else {
            block_rows
        };

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
            &mut self.particle.emitter_rows,
            |rows| {
                build_transparent_emitter_particles(
                    emitters,
                    solids,
                    time,
                    render_origin,
                    cam_pos.relative_to(render_origin),
                    env,
                    density,
                    rows,
                    &mut self.particle.emitter_scratch,
                )
            },
        );

        let shadows = std::mem::take(&mut self.shadow.instances);
        self.shadow
            .draw
            .bake(&self.device, &self.queue, &mut self.shadow.verts, |verts| {
                build_entity_shadows(&shadows, render_origin, verts)
            });
        self.shadow.instances = shadows;
    }
}

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
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        item: petramond_world::item::ItemType,
        sprite_pose: Option<petramond_world::item::SpriteHeldPose>,
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
                crate::item_model::dye_block_verts(&mut self.block_verts[start..], variant);
                crate::player_model::transform_positions(
                    self.block_verts[start..].iter_mut().map(|v| &mut v.pos),
                    m,
                );
            }
            petramond_world::item::ItemRenderKind::Sprite(tile) => {
                let m = if off_side {
                    crate::player_model::held_sprite_off_at(grip, sprite_pose)
                } else {
                    crate::player_model::held_sprite_at(grip, sprite_pose)
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
