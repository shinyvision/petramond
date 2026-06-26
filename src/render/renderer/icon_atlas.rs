//! Render-to-texture inventory ICON ATLAS.
//!
//! Every item's slot icon used to be rendered as live 3D geometry every frame (an
//! isometric cube, a flat billboard, or a baked bbmodel). Instead, each item's icon
//! is rendered ONCE at renderer init into a 64×64 cell of this atlas texture, and a
//! slot then draws a single 2D textured quad sampling its cell (see the UI pass in
//! `renderer::mod`). The icons never change, so baking once and sampling a quad per
//! slot is far cheaper than re-projecting cubes/models per frame.
//!
//! ## Layout
//! Cells are 64×64 (the max icon size), laid out [`COLS`] per row. Cell index `i`
//! (an item's stable `ItemType::id()`) sits at `(col = i % COLS, row = i / COLS)`,
//! pixel origin `(col*64, row*64)`. The atlas is `(COLS*64) × (rows*64)`.
//!
//! ## Format
//! The color texture uses the SURFACE format (an `*Srgb` format). Sampling decodes
//! sRGB→linear and the UI pass's blend/store re-encodes, exactly cancelling like the
//! existing gui atlas — so colors do NOT double-encode. A plain `Unorm` format would
//! darken every icon. A [`wgpu::FilterMode::Nearest`] sampler keeps the pixel art
//! crisp and, with exact integer cell UVs, prevents bleed between neighbouring cells.
//!
//! ## Baking (two passes, one submit)
//! `model3d_pipe` (cube + sprite icons) has NO depth attachment and CANNOT run in a
//! pass that has one; the bbmodel `model_icon_pipe` REQUIRES a depth buffer (its MVP
//! maps z into [0.1, 0.9] and the double-sided model self-sorts by depth). So the
//! bake uses two passes over the same atlas:
//! - **Pass A** (cube + sprite): color = atlas, NO depth. Each icon sets its cell
//!   viewport+scissor and draws with its own MVP slot in a dedicated, item-count-
//!   sized MVP buffer (the per-frame `model3d_mvp_buf` has too few slots to hold one
//!   per icon simultaneously, and all queue writes land before the single submit).
//! - **Pass B** (model): color = atlas (LOAD, preserving Pass A), depth = a full-
//!   atlas `Depth32Float` cleared to 1.0. The icon MVP is baked into the vertex
//!   positions by `build_block_model_icon`, so there is no per-icon uniform.

use wgpu::util::DeviceExt;

use crate::item::{ItemRenderKind, ItemType};
use crate::render::ui::icon::{flat_icon_mvp, iso_icon_mvp, model_icon_mvp};
use crate::render::ui::SlotRect;

use super::super::block_model::{push_billboard_quad, push_block_item_cube};
use super::super::chest_model::push_chest_item_full;
use super::super::item_model::{build_block_model_icon, ItemVertex};
use crate::block::Block;
use crate::mesh::Vertex;
use glam::Vec3;

/// Cells per atlas row.
const COLS: u32 = 16;
/// Side length (px) of one square icon cell — also the max icon size.
const CELL: u32 = 64;
/// Bytes of one model3d MVP slot (a `mat4` padded to the 256-byte dynamic-offset
/// alignment), matching the per-frame model3d MVP buffer.
const MVP_SLOT_SIZE: u64 = 256;

/// The baked icon atlas: a color texture (one 64×64 cell per item) sampled by the UI
/// pass via [`Self::bind`], plus the cell-UV lookup. Built once in the renderer
/// constructor; immutable thereafter.
pub(super) struct IconAtlas {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
    /// group(0) bind for the UI pass, built against the gui-atlas layout (`ui_bgl` /
    /// `atlas_bgl`: `{texture: Float filterable D2, sampler: Filtering}`) so it binds
    /// to `ui_pipe` exactly where the gui atlas does.
    pub bind: wgpu::BindGroup,
    /// Atlas dimensions (px), for the UV math.
    width: f32,
    height: f32,
}

impl IconAtlas {
    /// The atlas-cell UV rect `[u0, v0, u1, v1]` for `item` (top-left, bottom-right;
    /// v increases downward, matching the gui atlas). Exact integer cell edges so a
    /// Nearest-sampled quad never bleeds into a neighbour cell.
    pub fn cell_uv(&self, item: ItemType) -> [f32; 4] {
        let i = item.id() as u32;
        let col = i % COLS;
        let row = i / COLS;
        let x0 = (col * CELL) as f32;
        let y0 = (row * CELL) as f32;
        [
            x0 / self.width,
            y0 / self.height,
            (x0 + CELL as f32) / self.width,
            (y0 + CELL as f32) / self.height,
        ]
    }
}

/// One cube/sprite icon to draw in Pass A: its cell + index sub-range in the shared
/// model3d buffers + the 256-aligned dynamic offset of its MVP slot.
struct CubeIcon {
    col: u32,
    row: u32,
    index_start: u32,
    index_count: u32,
    mvp_offset: u32,
}

/// One bbmodel-model icon to draw in Pass B: its cell + index sub-range in the shared
/// model-icon buffers (the MVP is baked into the vertex positions).
struct ModelIcon {
    col: u32,
    row: u32,
    index_start: u32,
    index_count: u32,
}

/// Bake every non-`Air` item's icon into a fresh icon atlas and return it. `format`
/// MUST be the surface format (sRGB). `atlas_bgl` is the shared texture+sampler
/// layout (`{Float filterable D2, Filtering}`). `block_atlas_bind`/`model_atlas_bind`
/// are the existing group(1) binds the cube/sprite (block atlas) and model icons
/// (model atlas) sample. `model3d_pipe` is depthless; `model_icon_pipe` is depth-
/// tested. `model3d_mvp_bgl` + `uv_rects_buf` build the dedicated, item-count-sized
/// MVP buffer Pass A needs.
#[allow(clippy::too_many_arguments)]
pub(super) fn bake(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    atlas_bgl: &wgpu::BindGroupLayout,
    block_atlas_bind: &wgpu::BindGroup,
    model_atlas_bind: &wgpu::BindGroup,
    model3d_pipe: &wgpu::RenderPipeline,
    model_icon_pipe: &wgpu::RenderPipeline,
    model3d_mvp_bgl: &wgpu::BindGroupLayout,
    uv_rects_buf: &wgpu::Buffer,
) -> IconAtlas {
    let count = ItemType::ALL.len() as u32;
    let rows = count.div_ceil(COLS);
    let aw = COLS * CELL;
    let ah = rows * CELL;

    // --- Atlas color texture (surface sRGB format) + Nearest sampler + UI bind. ---
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("icon atlas"),
        size: wgpu::Extent3d {
            width: aw,
            height: ah,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("icon atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("icon atlas bg"),
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

    // Full-atlas depth buffer for Pass B (the model icons' z resolves their draw
    // order). Pass A is depthless and never touches it.
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("icon atlas depth"),
        size: wgpu::Extent3d {
            width: aw,
            height: ah,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    // The square 64×64 cell every icon's MVP is auto-framed to (undistorted).
    let screen = (CELL, CELL);
    let cell_rect = SlotRect {
        x: 0.0,
        y: 0.0,
        w: CELL as f32,
        h: CELL as f32,
    };

    // --- Build all icon geometry CPU-side, grouped by render kind. ---
    // Cube/sprite icons (block atlas, model3d pipe): one shared vbuf/ibuf with GLOBAL
    // indices (push_block_item_cube/push_billboard_quad base each quad at verts.len()), so
    // every icon draws with base_vertex 0 and its own index sub-range. Each also gets
    // its own MVP slot (Pass A holds them all live at once).
    let mut cube_verts: Vec<Vertex> = Vec::new();
    let mut cube_indices: Vec<u32> = Vec::new();
    let mut cube_icons: Vec<CubeIcon> = Vec::new();
    let mut cube_mvps: Vec<u8> = Vec::new(); // packed 256-aligned mat4 slots
    // Model icons (model atlas, model_icon pipe): one shared vbuf/ibuf, MVP baked in.
    let mut model_verts: Vec<ItemVertex> = Vec::new();
    let mut model_indices: Vec<u32> = Vec::new();
    let mut model_icons: Vec<ModelIcon> = Vec::new();

    for &item in ItemType::ALL {
        // Air never appears in a slot; skip its cell entirely (left transparent).
        if item == ItemType::Air {
            continue;
        }
        let i = item.id() as u32;
        let (col, row) = (i % COLS, i / COLS);
        match item.render_kind() {
            ItemRenderKind::BlockCube(block) => {
                let index_start = cube_indices.len() as u32;
                if block == Block::Chest {
                    push_chest_item_full(
                        &mut cube_verts,
                        &mut cube_indices,
                        Vec3::splat(-0.5),
                        1.0,
                    );
                } else {
                    push_block_item_cube(
                        &mut cube_verts,
                        &mut cube_indices,
                        block,
                        Vec3::splat(-0.5),
                        1.0,
                    );
                }
                let mvp_offset = cube_mvps.len() as u32;
                let mvp = iso_icon_mvp(screen, cell_rect);
                cube_mvps.extend_from_slice(mvp_slot_bytes(&mvp).as_slice());
                cube_icons.push(CubeIcon {
                    col,
                    row,
                    index_start,
                    index_count: cube_indices.len() as u32 - index_start,
                    mvp_offset,
                });
            }
            ItemRenderKind::Sprite(tile) => {
                let index_start = cube_indices.len() as u32;
                push_billboard_quad(&mut cube_verts, &mut cube_indices, tile, Vec3::ZERO, 1.0);
                let mvp_offset = cube_mvps.len() as u32;
                let mvp = flat_icon_mvp(screen, cell_rect);
                cube_mvps.extend_from_slice(mvp_slot_bytes(&mvp).as_slice());
                cube_icons.push(CubeIcon {
                    col,
                    row,
                    index_start,
                    index_count: cube_indices.len() as u32 - index_start,
                    mvp_offset,
                });
            }
            ItemRenderKind::Model(kind) => {
                let index_start = model_indices.len() as u32;
                let mvp = model_icon_mvp(screen, cell_rect, kind);
                build_block_model_icon(kind, mvp, &mut model_verts, &mut model_indices);
                model_icons.push(ModelIcon {
                    col,
                    row,
                    index_start,
                    index_count: model_indices.len() as u32 - index_start,
                });
            }
        }
    }

    // --- Upload the bake geometry + the dedicated Pass-A MVP buffer/bind. ---
    let cube_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("icon bake cube vbuf"),
        contents: cast_or_empty(&cube_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let cube_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("icon bake cube ibuf"),
        contents: cast_or_empty(&cube_indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    // One 256-aligned MVP slot per cube/sprite icon, all live simultaneously through
    // the single submit (so Pass A can't reuse one slot across draws). Built against
    // `model3d_mvp_bgl` (binding 0 = dynamic MVP, binding 1 = the shared uv_rects).
    // Always at least one 256-byte slot so the 64-byte mvp binding is valid even if
    // there were no cube/sprite icons at all (then the bind is simply never drawn).
    if cube_mvps.is_empty() {
        cube_mvps.resize(MVP_SLOT_SIZE as usize, 0);
    }
    let mvp_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("icon bake mvp"),
        contents: &cube_mvps,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("icon bake mvp bg"),
        layout: model3d_mvp_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                // A 64-byte mat4 window; the per-draw 256-aligned offset selects the slot.
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &mvp_buf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
        ],
    });

    let model_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("icon bake model vbuf"),
        contents: cast_or_empty(&model_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let model_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("icon bake model ibuf"),
        contents: cast_or_empty(&model_indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    // --- Record + submit the two bake passes. ---
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("icon atlas bake"),
    });
    // Pass A: cube + sprite icons. Color CLEAR (transparent — color's first use),
    // NO depth. Each icon: cell viewport+scissor, its MVP slot, its index range.
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("icon bake pass A (cube/sprite)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if !cube_icons.is_empty() {
            pass.set_pipeline(model3d_pipe);
            pass.set_bind_group(1, block_atlas_bind, &[]);
            pass.set_vertex_buffer(0, cube_vbuf.slice(..));
            pass.set_index_buffer(cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for icon in &cube_icons {
                set_cell(&mut pass, icon.col, icon.row);
                pass.set_bind_group(0, &mvp_bind, &[icon.mvp_offset]);
                pass.draw_indexed(icon.index_start..icon.index_start + icon.index_count, 0, 0..1);
            }
        }
    }
    // Pass B: bbmodel-model icons. Color LOAD (keep Pass A), depth CLEAR(1.0) —
    // depth's first use; the model_icon MVP expects a 1.0-cleared buffer.
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("icon bake pass B (model)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if !model_icons.is_empty() {
            pass.set_pipeline(model_icon_pipe);
            pass.set_bind_group(0, model_atlas_bind, &[]);
            pass.set_vertex_buffer(0, model_vbuf.slice(..));
            pass.set_index_buffer(model_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for icon in &model_icons {
                set_cell(&mut pass, icon.col, icon.row);
                pass.draw_indexed(icon.index_start..icon.index_start + icon.index_count, 0, 0..1);
            }
        }
    }
    queue.submit(std::iter::once(enc.finish()));

    IconAtlas {
        texture,
        view,
        sampler,
        bind,
        width: aw as f32,
        height: ah as f32,
    }
}

/// Restrict a render pass to cell `(col, row)`'s 64×64 pixel rect (viewport maps the
/// icon's NDC into the cell; scissor clips any fragment outside it, so an icon can
/// never bleed into a neighbour cell).
fn set_cell(pass: &mut wgpu::RenderPass, col: u32, row: u32) {
    let (x, y) = ((col * CELL) as f32, (row * CELL) as f32);
    pass.set_viewport(x, y, CELL as f32, CELL as f32, 0.0, 1.0);
    pass.set_scissor_rect(col * CELL, row * CELL, CELL, CELL);
}

/// One 256-byte dynamic-offset MVP slot: the 64-byte column-major `mat4` followed by
/// zero padding to the alignment, so successive slots sit at 256-byte offsets.
fn mvp_slot_bytes(mvp: &glam::Mat4) -> [u8; MVP_SLOT_SIZE as usize] {
    let mut slot = [0u8; MVP_SLOT_SIZE as usize];
    slot[..64].copy_from_slice(bytemuck::cast_slice(&mvp.to_cols_array()));
    slot
}

/// `bytemuck::cast_slice` of a possibly-empty `Pod` slice. `create_buffer_init`
/// rejects zero-length contents, so an empty slice yields a 4-byte zero pad (the
/// buffer is then never bound/drawn — its icon list is empty).
fn cast_or_empty<T: bytemuck::Pod>(v: &[T]) -> &[u8] {
    if v.is_empty() {
        &[0u8; 4]
    } else {
        bytemuck::cast_slice(v)
    }
}
