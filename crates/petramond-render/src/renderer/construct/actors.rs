use super::super::{create_model_texture, MobGpu, PlayerGpu, SkinnedModel};

const MOB_CULL_SLACK: f32 = 0.5;

pub(super) fn build_mob_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> Vec<MobGpu> {
    petramond::mob::defs()
        .iter()
        .map(|d| {
            let kind = d.mob;
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
            let (bmin, bmax) = model.rest_bounds();
            let r = [bmin.x, bmax.x]
                .into_iter()
                .flat_map(|x| [bmin.z, bmax.z].map(|z| (x * x + z * z).sqrt()))
                .fold(0.0f32, f32::max);
            let rig = crate::mob_model::MobRig::resolve(model, d.hands, d.shear.map(|s| s.coat.0))
                .with_self_ao(model, d.scale, d.self_ao);
            let mesh = SkinnedModel::new(device, &rig.mesh(model, d.scale), "mob");
            MobGpu {
                model,
                scale: d.scale,
                rig,
                bind,
                mesh,
                drawn: 0..0,
                cull_r: r * d.scale + MOB_CULL_SLACK,
                cull_y0: bmin.y * d.scale - MOB_CULL_SLACK,
                cull_y1: bmax.y * d.scale + MOB_CULL_SLACK,
                visible: Vec::new(),
                pose: Default::default(),
            }
        })
        .collect()
}

pub(super) fn build_player_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
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
    let mesh = petramond::player::rigs::presented(petramond::player::Presenter::Body)
        .map(|(_, rig)| {
            crate::skinned::SkinMesh::build(
                &rig.model,
                petramond::player::model::PLAYER_MODEL_SCALE,
                None,
                |_| 0,
            )
        })
        .unwrap_or_default();
    PlayerGpu {
        bind,
        mesh: SkinnedModel::new(device, &mesh, "player"),
        drawn: 0..0,
    }
}
