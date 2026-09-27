use super::super::{create_gui_panel, HudLayer, HudLayerTexture, UiVertex};

pub(super) fn build_hud_layers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> Vec<HudLayer> {
    let load_gui_bind = |rel: &str| -> Option<wgpu::BindGroup> {
        let (bytes, _path) = petramond_world::assets::read_bytes(rel)?;
        let Some((_tex, view, sampler)) = create_gui_panel(device, queue, &bytes) else {
            log::warn!("{rel} is not a decodable PNG; its HUD layer draws nothing");
            return None;
        };
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gui texture bind"),
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
        }))
    };
    let hud_layer = |label: &'static str,
                     source: fn(&crate::ui::UiBuild) -> &[UiVertex],
                     texture: HudLayerTexture,
                     under_chrome: bool| {
        HudLayer {
            source,
            texture,
            under_chrome,
            vbuf: crate::renderer::dynamic_draw::new_buffer(
                device,
                wgpu::BufferUsages::VERTEX,
                label,
            ),
            vertex_count: 0,
        }
    };
    let effects_bind = crate::effect_icons::compose_atlas().map(|img| {
        let (_tex, view, sampler) =
            crate::resources::create_rgba_nearest(device, queue, &img, "effect icons");
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect icons bind"),
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
        })
    });
    vec![
        hud_layer(
            "hud vignette",
            |b| &b.vignette,
            HudLayerTexture::Solid,
            true,
        ),
        hud_layer(
            "hud hearts",
            |b| &b.hearts,
            HudLayerTexture::Texture(load_gui_bind("textures/gui/hearts.png")),
            false,
        ),
        hud_layer(
            "hud effects",
            |b| &b.effects,
            HudLayerTexture::Texture(effects_bind),
            false,
        ),
    ]
}
