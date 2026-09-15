//! Per-species and player body GPU resources built at renderer construction.

use super::super::{create_model_texture, DynamicDraw, MobGpu, PlayerGpu};

/// World-space margin added around each species' rest-pose cull bounds.
const MOB_CULL_SLACK: f32 = 0.5;

/// Build per-species mob render resources by iterating the mob registry: load each
/// species' `.bbmodel` (geometry + walk animation + embedded texture), upload its
/// texture as a dedicated atlas, build its group(1) bind, and give it its own
/// dynamic-draw buffers over the shared mob pipeline. Adding a species is a row in
/// `mobs.json` — no renderer edit. A model parse failure degrades to an empty
/// model (that species just doesn't draw) rather than crashing the renderer.
pub(super) fn build_mob_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
    mob_pipe: &crate::pipeline::SampledPipeline,
) -> Vec<MobGpu> {
    petramond::mob::defs()
        .iter()
        .map(|d| {
            let kind = d.mob;
            // Borrow this species' precached model (compiled once on startup, shared with
            // the simulation — see `petramond::mob::model`). The renderer never reads a
            // `.bbmodel`: at runtime the `.llmob` + this in-memory `Model` are golden.
            let model = petramond::mob::model(kind);
            let (_texture, view, sampler) =
                create_model_texture(device, queue, &model.texture_rgba, model.tex_w, model.tex_h);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mob atlas bg"),
                layout: atlas_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            // Cull volume from the rest-posed model bounds × render scale. The
            // horizontal radius takes the farthest posed corner (yaw can point
            // it in any direction); MOB_CULL_SLACK absorbs what the rest pose
            // cannot know — walk/idle limb swing, head-look, the interpolation
            // between replicated positions. Conservative slack costs a few
            // early-visible instances; too tight pops mobs at screen edges.
            let (bmin, bmax) = model.rest_bounds();
            let r = [bmin.x, bmax.x]
                .into_iter()
                .flat_map(|x| [bmin.z, bmax.z].map(|z| (x * x + z * z).sqrt()))
                .fold(0.0f32, f32::max);
            MobGpu {
                model,
                scale: d.scale,
                bind,
                draw: DynamicDraw::new(device, mob_pipe.clone(), "mob"),
                cull_r: r * d.scale + MOB_CULL_SLACK,
                cull_y0: bmin.y * d.scale - MOB_CULL_SLACK,
                cull_y1: bmax.y * d.scale + MOB_CULL_SLACK,
                visible: Vec::new(),
                verts: Vec::new(),
                indices: Vec::new(),
            }
        })
        .collect()
}

/// Player bodies: the precached player model gets the same shape of
/// resources as one mob species (own skin texture bind + dynamic draw over
/// the shared mob pipeline), plus three held-item draws attached to the
/// posed hands: an extruded-sprite stream (2D atlas), a bbmodel-item stream
/// (model atlas), and a packed block-vertex stream (held mini-cube on the
/// opaque pipeline). EVERY connected player's body appends into the one
/// stream, which grows to fit the party.
pub(super) fn build_player_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
    mob_pipe: &crate::pipeline::SampledPipeline,
) -> PlayerGpu {
    let model = petramond::player::model::player_model();
    let (_texture, view, sampler) =
        create_model_texture(device, queue, &model.texture_rgba, model.tex_w, model.tex_h);
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("player atlas bg"),
        layout: atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    PlayerGpu {
        bind,
        draw: DynamicDraw::new(device, mob_pipe.clone(), "player"),
        verts: Vec::new(),
        indices: Vec::new(),
    }
}
