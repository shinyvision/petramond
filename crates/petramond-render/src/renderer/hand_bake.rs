//! Baking the first-person hand pass: the rig's arms in the player's skin and
//! each hand's item CARRIED by its fist from the item's rest seat, all in
//! view space under one MVP.

use super::Renderer;

impl Renderer {
    /// The hurt-shake as a clip-space post-transform: left-multiplying a
    /// translation adds `t * w` to the clip position, which after the divide is
    /// exactly an NDC screen shift — the whole hand jitters without touching
    /// any pose math.
    fn hand_shake_mat(&self) -> glam::Mat4 {
        if !self.hand.screen_shake {
            return glam::Mat4::IDENTITY;
        }
        glam::Mat4::from_translation(glam::Vec3::new(self.hand.shake[0], self.hand.shake[1], 0.0))
    }

    /// Build + upload this frame's hand pass geometry. Both hands' block cubes
    /// share one indexed model3d stream (absolute indices); sprites, bbmodels
    /// and the arms share the item3d stream as ranges. Nothing draws without
    /// the rig.
    pub(super) fn prepare_held_item(&mut self) {
        use crate::first_person::Hand;
        use crate::player_model::transform_positions;
        use petramond_world::item::ItemRenderKind;

        self.hand.index_count = 0;
        self.hand.vertex_count = 0;
        self.hand.item3d_vertex_count = 0;
        self.hand.held_is_model = false;
        self.hand.off_item3d_start = 0;
        self.hand.off_item3d_count = 0;
        self.hand.off_is_model = false;
        self.hand.arm_count = 0;
        if !self.hand.visible {
            return;
        }
        let Some(first_person) = self.hand.first_person.take() else {
            return;
        };
        let aspect = if self.config.height > 0 {
            self.config.width as f32 / self.config.height as f32
        } else {
            1.0
        };
        let shake = self.hand_shake_mat();
        let light = self.held_item_light();
        let env = self.light_env();

        let mut hv = std::mem::take(&mut self.hand.verts);
        let mut hi = std::mem::take(&mut self.hand.indices);
        let mut iv = std::mem::take(&mut self.hand.item3d_verts);
        let mut tv = std::mem::take(&mut self.hand.model_scratch_verts);
        let mut ti = std::mem::take(&mut self.hand.model_scratch_indices);
        let mut sv = std::mem::take(&mut self.hand.off_item3d_scratch);
        hv.clear();
        hi.clear();
        iv.clear();
        let (rig, bones) = (&first_person.rig, &first_person.bones);

        for (hand, view) in [
            (Hand::Main, self.hand.held_item),
            (Hand::Off, self.hand.off_item),
        ] {
            let off = hand == Hand::Off;
            let start = iv.len() as u32;
            let mut is_model = false;
            let seated =
                view.item
                    .zip(crate::hand::rest_seat(&view, off))
                    .and_then(|(item, seat)| {
                        let seat = match item.render_kind() {
                            ItemRenderKind::Sprite(tile) => rig.sprite_in_fist(
                                hand,
                                seat,
                                crate::item_model::grip_point(tile, item.tool().is_some()),
                            ),
                            _ => seat,
                        };
                        rig.carry(bones, hand, item)
                            .map(|carry| (item.render_kind(), crate::hand::carried(seat, carry)))
                    });
            if let Some((kind, at)) = seated {
                match kind {
                    ItemRenderKind::BlockCube(block) => {
                        let first = hv.len();
                        if block == petramond_world::block::Block::Chest {
                            crate::chest_model::push_chest_item(
                                &mut hv,
                                &mut hi,
                                glam::Vec3::splat(-0.5),
                                1.0,
                                light,
                            );
                        } else {
                            crate::item_cube::push_block_item_cube_lit_with_state(
                                &mut hv,
                                &mut hi,
                                block,
                                view.block_state,
                                glam::Vec3::splat(-0.5),
                                1.0,
                                light,
                                false,
                            );
                        }
                        crate::item_model::dye_block_verts(&mut hv[first..], view.variant);
                        transform_positions(hv[first..].iter_mut().map(|v| &mut v.pos), at);
                    }
                    ItemRenderKind::Sprite(tile) => {
                        crate::item_model::build_extruded_stack_lit(
                            tile,
                            view.variant,
                            light,
                            env,
                            &mut sv,
                        );
                        transform_positions(sv.iter_mut().map(|v| &mut v.pos), at);
                        iv.extend_from_slice(&sv);
                    }
                    ItemRenderKind::Model(kind) => {
                        tv.clear();
                        ti.clear();
                        crate::item_model::build_block_model_item(
                            kind, at, light, env, None, &mut tv, &mut ti,
                        );
                        iv.extend(ti.iter().map(|&i| tv[i as usize]));
                        is_model = true;
                    }
                }
            }
            let count = iv.len() as u32 - start;
            if off {
                self.hand.off_item3d_start = start;
                self.hand.off_item3d_count = count;
                self.hand.off_is_model = is_model;
            } else {
                self.hand.item3d_vertex_count = count;
                self.hand.held_is_model = is_model;
            }
        }
        self.hand.index_count = hi.len() as u32;
        self.hand.vertex_count = hv.len() as u32;

        tv.clear();
        ti.clear();
        let tint = crate::mob_model::body_tint(0.0, [1.0; 3], light, env, 0.0);
        rig.bake(bones, tint, &mut tv, &mut ti);
        self.hand.arm_start = iv.len() as u32;
        iv.extend(ti.iter().map(|&i| tv[i as usize]));
        self.hand.arm_count = iv.len() as u32 - self.hand.arm_start;

        let view_offset = if self.hand.screen_shake {
            first_person.view_offset()
        } else {
            glam::Mat4::IDENTITY
        };
        let mvp = shake * crate::hand::model_hand_view_proj(aspect) * view_offset;
        self.upload_hand(&hv, &hi, &iv, mvp);
        self.hand.verts = hv;
        self.hand.indices = hi;
        self.hand.item3d_verts = iv;
        self.hand.model_scratch_verts = tv;
        self.hand.model_scratch_indices = ti;
        self.hand.off_item3d_scratch = sv;
        self.hand.first_person = Some(first_person);
    }

    /// One upload per hand stream, each buffer grown to fit, and the MVP
    /// both hands draw through (slot 0 of the hand and item3d MVP buffers;
    /// slot 1, at byte 256, mirrors it so either binding works).
    fn upload_hand(
        &mut self,
        hv: &[petramond_mesh::Vertex],
        hi: &[u32],
        iv: &[crate::item_model::ItemVertex],
        mvp: glam::Mat4,
    ) {
        if !hi.is_empty() {
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.model3d_vbuf,
                hv,
                wgpu::BufferUsages::VERTEX,
                "model3d vbuf",
            );
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.model3d_ibuf,
                hi,
                wgpu::BufferUsages::INDEX,
                "model3d ibuf",
            );
        }
        if !iv.is_empty() {
            super::dynamic_draw::upload(
                &self.device,
                &self.queue,
                &mut self.hand.item3d_vbuf,
                iv,
                wgpu::BufferUsages::VERTEX,
                "item3d vbuf",
            );
        }
        for slot in [0u64, 256u64] {
            self.queue.write_buffer(
                &self.hand.model3d_mvp_buf,
                slot,
                bytemuck::cast_slice(&mvp.to_cols_array()),
            );
        }
    }
}
