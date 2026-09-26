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

use crate::ui::icon::{flat_icon_mvp, iso_icon_mvp, model_icon_mvp};
use petramond::gui::SlotRect;
use petramond_world::item::{ItemRenderKind, ItemType};

use super::super::item_cube::{push_billboard_quad, push_block_item_cube};
use super::super::item_model::{build_block_model_icon, ItemVertex};
use glam::Vec3;
use petramond_mesh::Vertex;

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
    /// group(0) bind for the UI pass, built against the gui-atlas layout (`ui_bgl` /
    /// `atlas_bgl`: `{texture: Float filterable D2, sampler: Filtering}`) so it binds
    /// to `ui_pipe` exactly where the gui atlas does.
    pub bind: wgpu::BindGroup,
    /// Atlas dimensions (px), for the UV math.
    width: f32,
    height: f32,
    /// Item count at bake — a stack's DYED twin cell sits at `item_cells + id`.
    item_cells: u32,
}

impl IconAtlas {
    /// The atlas-cell UV rect `[u0, v0, u1, v1]` for `item` (top-left, bottom-right;
    /// v increases downward, matching the gui atlas). Exact integer cell edges so a
    /// Nearest-sampled quad never bleeds into a neighbour cell.
    pub fn cell_uv(&self, item: ItemType) -> [f32; 4] {
        self.cell_uv_at(item.id() as u32)
    }

    /// The DYED twin cell for `item`: the same icon baked off the dye-base
    /// tiles, for stacks carrying a `petramond:tint` (the UI multiplies the
    /// tint on top).
    pub fn cell_uv_dyed(&self, item: ItemType) -> [f32; 4] {
        self.cell_uv_at(self.item_cells + item.id() as u32)
    }

    fn cell_uv_at(&self, i: u32) -> [f32; 4] {
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
    uniform_buf: &wgpu::Buffer,
) -> IconAtlas {
    let count = ItemType::all().len() as u32;
    // Every item gets TWO cells: its ordinary icon at index `item id`, and a
    // DYED twin at `count + id` — the same icon rendered off the atlas's
    // dye-base tiles (desaturated, peak-white), which the UI multiplies by a
    // stack's `petramond:tint`. Model (bbmodel) icons have no dye-base half
    // in the model atlas, so their twin is a plain copy (the multiply alone).
    let rows = (2 * count).div_ceil(COLS);
    let size = (COLS * CELL, rows * CELL);
    let texture = create_atlas_texture(device, format, size);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = create_atlas_bind(device, atlas_bgl, &view);
    // Full-atlas depth buffer for Pass B (the model icons' z resolves their draw
    // order). Pass A is depthless and never touches it.
    let depth_view = create_atlas_depth(device, size);

    let geometry = IconGeometry::build(count);
    let buffers = geometry.upload(device, model3d_mvp_bgl, uv_rects_buf, uniform_buf);
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
        if !geometry.cube_icons.is_empty() {
            pass.set_pipeline(model3d_pipe);
            pass.set_bind_group(1, block_atlas_bind, &[]);
            pass.set_vertex_buffer(0, buffers.cube_vbuf.slice(..));
            pass.set_index_buffer(buffers.cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for icon in &geometry.cube_icons {
                set_cell(&mut pass, icon.col, icon.row);
                pass.set_bind_group(0, &buffers.mvp_bind, &[icon.mvp_offset]);
                pass.draw_indexed(
                    icon.index_start..icon.index_start + icon.index_count,
                    0,
                    0..1,
                );
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
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if !geometry.model_icons.is_empty() {
            pass.set_pipeline(model_icon_pipe);
            pass.set_bind_group(0, model_atlas_bind, &[]);
            pass.set_vertex_buffer(0, buffers.model_vbuf.slice(..));
            pass.set_index_buffer(buffers.model_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for icon in &geometry.model_icons {
                set_cell(&mut pass, icon.col, icon.row);
                pass.draw_indexed(
                    icon.index_start..icon.index_start + icon.index_count,
                    0,
                    0..1,
                );
            }
        }
    }
    queue.submit(std::iter::once(enc.finish()));

    // Debug aid: PETRAMOND_DUMP_ICON_ATLAS=<path.png> writes the atlas exactly
    // as baked, for checking icon fidelity without clicking through the game.
    if let Ok(path) = std::env::var("PETRAMOND_DUMP_ICON_ATLAS") {
        dump_atlas(device, queue, &texture, size.0, size.1, format, &path);
    }

    IconAtlas {
        bind,
        width: size.0 as f32,
        height: size.1 as f32,
        item_cells: count,
    }
}

/// The atlas colour texture, in the surface format so sampling and store
/// cancel like the gui atlas (no double gamma). Readable back only when the
/// debug dump asks for it.
fn create_atlas_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    if std::env::var_os("PETRAMOND_DUMP_ICON_ATLAS").is_some() {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    crate::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("icon atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        },
    )
}

/// The UI node's bind over the atlas: a Nearest sampler, so a quad never
/// blends a neighbour cell in.
fn create_atlas_bind(
    device: &wgpu::Device,
    atlas_bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
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
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("icon atlas bg"),
        layout: atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    })
}

fn create_atlas_depth(device: &wgpu::Device, (width, height): (u32, u32)) -> wgpu::TextureView {
    crate::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("icon atlas depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        },
    )
    .create_view(&wgpu::TextureViewDescriptor::default())
}

/// `(col, row)` of atlas cell `index`.
fn cell_of(index: u32) -> (u32, u32) {
    (index % COLS, index / COLS)
}

/// Every icon's geometry, built CPU-side and grouped by render kind.
#[derive(Default)]
struct IconGeometry {
    /// Cube/sprite icons (block atlas, model3d pipe): one shared vbuf/ibuf with
    /// GLOBAL indices (`push_block_item_cube`/`push_billboard_quad` base each quad
    /// at `verts.len()`), so every icon draws with base_vertex 0 and its own index
    /// sub-range. Each also gets its own MVP slot (Pass A holds them all live at
    /// once).
    cube_verts: Vec<Vertex>,
    cube_indices: Vec<u32>,
    cube_icons: Vec<CubeIcon>,
    /// Packed 256-aligned mat4 slots.
    cube_mvps: Vec<u8>,
    /// Model icons (model atlas, model_icon pipe): one shared vbuf/ibuf, MVP
    /// baked into the vertices.
    model_verts: Vec<ItemVertex>,
    model_indices: Vec<u32>,
    model_icons: Vec<ModelIcon>,
}

/// The bake's GPU copies of an [`IconGeometry`], plus the dedicated Pass-A
/// MVP bind.
struct IconBuffers {
    cube_vbuf: wgpu::Buffer,
    cube_ibuf: wgpu::Buffer,
    mvp_bind: wgpu::BindGroup,
    model_vbuf: wgpu::Buffer,
    model_ibuf: wgpu::Buffer,
}

impl IconGeometry {
    /// Every non-`Air` item's icon and its dyed twin (at `count + id`).
    fn build(count: u32) -> Self {
        // The square 64×64 cell every icon's MVP is auto-framed to (undistorted).
        let screen = (CELL, CELL);
        let cell_rect = SlotRect {
            x: 0.0,
            y: 0.0,
            w: CELL as f32,
            h: CELL as f32,
        };
        let mut geometry = Self::default();
        for &item in ItemType::all() {
            // Air never appears in a slot; skip its cell entirely (left transparent).
            if item == ItemType::Air {
                continue;
            }
            let i = item.id() as u32;
            let (cell, twin) = (cell_of(i), cell_of(count + i));
            match item.render_kind() {
                ItemRenderKind::BlockCube(block) => {
                    geometry.push_cube_icon(cell, twin, iso_icon_mvp(screen, cell_rect), |v, ix| {
                        push_block_item_cube(v, ix, block, Vec3::splat(-0.5), 1.0)
                    })
                }
                ItemRenderKind::Sprite(tile) => {
                    geometry.push_cube_icon(cell, twin, flat_icon_mvp(screen, cell_rect), |v, ix| {
                        push_billboard_quad(v, ix, tile, Vec3::ZERO, 1.0)
                    })
                }
                ItemRenderKind::Model(kind) => {
                    let mvp = model_icon_mvp(screen, cell_rect, kind);
                    geometry.push_model_icon(cell, twin, |v, ix| {
                        build_block_model_icon(kind, mvp, v, ix)
                    })
                }
            }
        }
        geometry
    }

    /// A cube or sprite icon in `cell` and its dyed twin in `twin`: the same
    /// geometry pushed twice, the second copy flagged dyed so model3d samples
    /// the dye-base tiles. Both draw through one MVP slot.
    fn push_cube_icon(
        &mut self,
        cell: (u32, u32),
        twin: (u32, u32),
        mvp: glam::Mat4,
        push: impl Fn(&mut Vec<Vertex>, &mut Vec<u32>),
    ) {
        let mvp_offset = self.cube_mvps.len() as u32;
        self.cube_mvps.extend_from_slice(&mvp_slot_bytes(&mvp));
        for ((col, row), dyed) in [(cell, false), (twin, true)] {
            let index_start = self.cube_indices.len() as u32;
            let vert_start = self.cube_verts.len();
            push(&mut self.cube_verts, &mut self.cube_indices);
            if dyed {
                for v in &mut self.cube_verts[vert_start..] {
                    v.packed2 |= petramond_mesh::DYED_FLAG2;
                }
            }
            self.cube_icons.push(CubeIcon {
                col,
                row,
                index_start,
                index_count: self.cube_indices.len() as u32 - index_start,
                mvp_offset,
            });
        }
    }

    /// A bbmodel icon in `cell` and its twin in `twin`: a plain copy of the
    /// same index range (no dye-base half in the model atlas; the UI's tint
    /// multiply still applies).
    fn push_model_icon(
        &mut self,
        cell: (u32, u32),
        twin: (u32, u32),
        push: impl FnOnce(&mut Vec<ItemVertex>, &mut Vec<u32>),
    ) {
        let index_start = self.model_indices.len() as u32;
        push(&mut self.model_verts, &mut self.model_indices);
        let index_count = self.model_indices.len() as u32 - index_start;
        for (col, row) in [cell, twin] {
            self.model_icons.push(ModelIcon {
                col,
                row,
                index_start,
                index_count,
            });
        }
    }

    /// Upload the geometry and build the dedicated Pass-A MVP buffer + bind.
    fn upload(
        &self,
        device: &wgpu::Device,
        model3d_mvp_bgl: &wgpu::BindGroupLayout,
        uv_rects_buf: &wgpu::Buffer,
        uniform_buf: &wgpu::Buffer,
    ) -> IconBuffers {
        let buffer = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // One 256-aligned MVP slot per cube/sprite icon, all live simultaneously
        // through the single submit (so Pass A can't reuse one slot across draws).
        // Always at least one slot so the 64-byte mvp binding is valid even with
        // no cube/sprite icons at all (the bind is then simply never drawn).
        let empty_slot = [0u8; MVP_SLOT_SIZE as usize];
        let mvps = if self.cube_mvps.is_empty() {
            &empty_slot[..]
        } else {
            &self.cube_mvps[..]
        };
        let mvp_buf = buffer("icon bake mvp", mvps, wgpu::BufferUsages::UNIFORM);
        // Built against `model3d_mvp_bgl` (binding 0 = dynamic MVP, binding 1 =
        // the shared uv_rects, binding 2 = the frame uniforms).
        let mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icon bake mvp bg"),
            layout: model3d_mvp_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    // A 64-byte mat4 window; the per-draw 256-aligned offset
                    // selects the slot.
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
                // The frame Uniforms (model3d reads only the sky-scale lane,
                // fog_color.w). At init its value is the identity 1.0, so baked
                // icons are full-bright regardless of any later in-game scale.
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });
        IconBuffers {
            cube_vbuf: buffer(
                "icon bake cube vbuf",
                cast_or_empty(&self.cube_verts),
                wgpu::BufferUsages::VERTEX,
            ),
            cube_ibuf: buffer(
                "icon bake cube ibuf",
                cast_or_empty(&self.cube_indices),
                wgpu::BufferUsages::INDEX,
            ),
            mvp_bind,
            model_vbuf: buffer(
                "icon bake model vbuf",
                cast_or_empty(&self.model_verts),
                wgpu::BufferUsages::VERTEX,
            ),
            model_ibuf: buffer(
                "icon bake model ibuf",
                cast_or_empty(&self.model_indices),
                wgpu::BufferUsages::INDEX,
            ),
        }
    }
}

/// Read the baked atlas back and write it as a PNG (BGRA surfaces swapped to
/// RGBA). Debug-only path behind `PETRAMOND_DUMP_ICON_ATLAS`.
fn dump_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    path: &str,
) {
    let row = w * 4; // 4096 for the 16-col atlas: already 256-aligned
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("icon atlas dump"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(enc.finish()));
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let mut data = buf.slice(..).get_mapped_range().to_vec();
    if matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        for px in data.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    image::save_buffer(path, &data, w, h, image::ColorType::Rgba8).unwrap();
    eprintln!("icon atlas dumped to {path}");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every slot-visible item bakes into its own cell and a dyed twin at
    /// `count + id`, no two icons share a cell, and a cube twin reuses its
    /// original's MVP slot and geometry size.
    #[test]
    fn every_item_gets_its_cell_and_a_dyed_twin() {
        let count = ItemType::all().len() as u32;
        let geometry = IconGeometry::build(count);
        let cells: Vec<(u32, u32)> = geometry
            .cube_icons
            .iter()
            .map(|icon| (icon.col, icon.row))
            .chain(geometry.model_icons.iter().map(|icon| (icon.col, icon.row)))
            .collect();
        let items: Vec<ItemType> = ItemType::all()
            .iter()
            .copied()
            .filter(|&item| item != ItemType::Air)
            .collect();
        assert_eq!(cells.len(), 2 * items.len());
        let unique: std::collections::HashSet<_> = cells.iter().copied().collect();
        assert_eq!(unique.len(), cells.len(), "two icons share a cell");
        for item in items {
            let id = item.id() as u32;
            assert!(unique.contains(&cell_of(id)), "item {id} has no icon");
            assert!(unique.contains(&cell_of(count + id)), "item {id} has no dyed twin");
        }
        for pair in geometry.cube_icons.chunks_exact(2) {
            assert_eq!(pair[0].mvp_offset, pair[1].mvp_offset);
            assert_eq!(pair[0].index_count, pair[1].index_count);
        }
        assert_eq!(
            geometry.cube_mvps.len(),
            geometry.cube_icons.len() / 2 * MVP_SLOT_SIZE as usize
        );
    }
}
