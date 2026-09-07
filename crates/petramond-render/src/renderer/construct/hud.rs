//! The HUD chrome layers built at renderer construction (see `HudLayer`).

use super::super::{create_gui_panel, HudLayer, HudLayerTexture, UiVertex};

/// HUD heart atlas (empty | half | full, side by side). One texture for the whole
/// health bar; the UI pass selects a cell per heart by UV. Resolved through the
/// asset overlay (a pack can reskin it) into its own bind group (reusing the
/// gui-atlas bind layout).
pub(super) fn build_hud_layers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> Vec<HudLayer> {
    let load_gui_bind = |rel: &str| -> Option<wgpu::BindGroup> {
        let (bytes, _path) = petramond_world::assets::read_bytes(rel)?;
        let (_tex, view, sampler) = create_gui_panel(device, queue, &bytes);
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
    // The HUD chrome layers, in draw order. Adding a HUD element = one
    // `UiBuild` vec + one entry here (see `HudLayer`).
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
    // Status-effect icon strip: composed on the CPU from the shared frame +
    // each registered effect's icon (engine and pack rows alike), uploaded
    // once — the HUD indexes cells by effect id like hearts index their atlas.
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
        // Hurt-flash red edge vignette, under all chrome (solid gradient quads).
        hud_layer(
            "hud vignette",
            |b| &b.vignette,
            HudLayerTexture::Solid,
            true,
        ),
        // HUD hearts (bottom-left health bar), from the heart atlas.
        hud_layer(
            "hud hearts",
            |b| &b.hearts,
            HudLayerTexture::Texture(load_gui_bind("textures/gui/hearts.png")),
            false,
        ),
        // Status-effect icons (framed row above the hearts), from the strip.
        hud_layer(
            "hud effects",
            |b| &b.effects,
            HudLayerTexture::Texture(effects_bind),
            false,
        ),
    ]
}
