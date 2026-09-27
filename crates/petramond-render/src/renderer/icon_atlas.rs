//! Render-to-texture inventory ICON ATLAS.
//!
//! Each item icon bakes once at renderer init into a 64x64
//! cell of this atlas, and a slot just draws a textured quad sampling its cell
//! (see UI pass in `renderer::mod`). Icons never change, so baking once beats
//! reprojecting per frame.
//!
//! ## Layout
//! Cells are 64x64, laid out [`IconLayout::cols`] per row, as square as possible
//! so the atlas grows in both dimensions instead of hitting the texture-size
//! limit via one tall column. That caps us at `(max/64)^2` cells (16384 at the
//! common 8192px limit). Cell `i` (an item's `ItemType::id()`) sits at
//! `(i % cols, i / cols)`, pixel origin `(col*64, row*64)`. A catalogue past
//! capacity is reported once at bake, surplus icons render blank.
//!
//! ## Format
//! Color texture uses the SURFACE format (sRGB). Sampling decodes sRGB->linear
//! and the UI pass's blend/store re-encodes, canceling out like the gui atlas,
//! so colors don't double-encode. A plain `Unorm` format would darken every
//! icon. Nearest filtering keeps pixel art crisp, and exact integer cell UVs
//! avoid bleed between cells.
//!
//! ## Baking (two passes, one submit)
//! `model3d_pipe` (cube + sprite) has no depth attachment and can't run in a
//! pass that has one. `model_icon_pipe` (bbmodel) needs depth (its MVP maps z
//! into [0.1, 0.9] and the double-sided model self-sorts by depth). So baking
//! uses two passes over the same atlas:
//! - Pass A (cube + sprite): color = atlas, no depth. Each icon sets its own
//!   cell viewport+scissor and draws with its own MVP slot in a dedicated,
//!   item-count-sized MVP buffer (the per-frame `model3d_mvp_buf` has too few
//!   slots to hold one per icon at once, and all queue writes land before the
//!   single submit).
//! - Pass B (model): color = atlas (LOAD, keeps Pass A), depth = full-atlas
//!   `Depth32Float` cleared to 1.0. Icon MVP is baked into vertex positions by
//!   `build_block_model_icon`, so no per-icon uniform is needed.

use wgpu::util::DeviceExt;

use crate::ui::icon::{flat_icon_mvp, iso_icon_mvp, model_icon_mvp};
use petramond::gui::SlotRect;
use petramond_world::item::{ItemRenderKind, ItemType};

use super::super::item_cube::{push_billboard_quad, push_block_item_cube};
use super::super::item_model::{build_block_model_icon, ItemVertex};
use glam::Vec3;
use petramond_mesh::Vertex;

const MIN_COLS: u32 = 16;
const CELL: u32 = 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct IconLayout {
    cols: u32,
    rows: u32,
}

impl IconLayout {
    fn new(cells: u32, max_dim: u32) -> Self {
        let max_cells = (max_dim / CELL).max(1);
        let square = (cells as f64).sqrt().ceil() as u32;
        let cols = square.max(MIN_COLS).min(max_cells);
        let rows = cells.div_ceil(cols).clamp(1, max_cells);
        Self { cols, rows }
    }

    fn capacity(self) -> u32 {
        self.cols * self.rows
    }

    fn cell(self, i: u32) -> Option<(u32, u32)> {
        (i < self.capacity()).then_some((i % self.cols, i / self.cols))
    }

    fn size(self) -> (u32, u32) {
        (self.cols * CELL, self.rows * CELL)
    }
}
const MVP_SLOT_SIZE: u64 = 256;

pub(super) struct IconAtlas {
    pub bind: wgpu::BindGroup,
    layout: IconLayout,
    item_cells: u32,
}

impl IconAtlas {
    pub fn cell_uv(&self, item: ItemType) -> [f32; 4] {
        self.cell_uv_at(item.id() as u32)
    }

    pub fn cell_uv_dyed(&self, item: ItemType) -> [f32; 4] {
        self.cell_uv_at(self.item_cells + item.id() as u32)
    }

    fn cell_uv_at(&self, i: u32) -> [f32; 4] {
        let (col, row) = self.layout.cell(i).unwrap_or((0, 0));
        let (width, height) = self.layout.size();
        let (width, height) = (width as f32, height as f32);
        let x0 = (col * CELL) as f32;
        let y0 = (row * CELL) as f32;
        [
            x0 / width,
            y0 / height,
            (x0 + CELL as f32) / width,
            (y0 + CELL as f32) / height,
        ]
    }
}

struct CubeIcon {
    col: u32,
    row: u32,
    index_start: u32,
    index_count: u32,
    mvp_offset: u32,
}

struct ModelIcon {
    col: u32,
    row: u32,
    index_start: u32,
    index_count: u32,
}

/// Bakes every non-`Air` item icon into a fresh atlas.
/// `format` has to be the surface format (sRGB). `atlas_bgl` is the shared texture+sampler
/// layout (`{Float filterable D2, Filtering}`).
/// `block_atlas_bind`/`model_atlas_bind` are the group(1) binds we already have for cube/sprite
/// icons (block atlas) and model icons (model atlas).
/// `model3d_pipe` has no depth test, `model_icon_pipe` does.
/// `model3d_mvp_bgl` + `uv_rects_buf` build the item-count-sized MVP buffer Pass A needs.
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
    let max_dim = device.limits().max_texture_dimension_2d;
    let layout = IconLayout::new(2 * count, max_dim);
    if layout.capacity() < 2 * count {
        log::error!(
            "{count} items need {} icon cells (each plus its dyed twin), but this GPU's \
             {max_dim} px texture limit holds {}; icons past that render blank",
            2 * count,
            layout.capacity()
        );
    }
    let size = layout.size();
    let texture = create_atlas_texture(device, format, size);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = create_atlas_bind(device, atlas_bgl, &view);
    let depth_view = create_atlas_depth(device, size);

    let geometry = IconGeometry::build(count, layout);
    let buffers = geometry.upload(device, model3d_mvp_bgl, uv_rects_buf, uniform_buf);
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("icon atlas bake"),
    });
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

    if let Ok(path) = std::env::var("PETRAMOND_DUMP_ICON_ATLAS") {
        dump_atlas(device, queue, &texture, size.0, size.1, format, &path);
    }

    IconAtlas {
        bind,
        layout,
        item_cells: count,
    }
}

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

#[derive(Default)]
struct IconGeometry {
    /// Cube/sprite icons (block atlas, model3d pipe) share one vbuf/ibuf with
    /// GLOBAL indices (`push_block_item_cube`/`push_billboard_quad` base each quad
    /// at `verts.len()`), so every icon draws with base_vertex 0 and its own index
    /// sub-range. Each also gets its own MVP slot, since Pass A needs them all live
    /// at once.
    cube_verts: Vec<Vertex>,
    cube_indices: Vec<u32>,
    cube_icons: Vec<CubeIcon>,
    cube_mvps: Vec<u8>,
    model_verts: Vec<ItemVertex>,
    model_indices: Vec<u32>,
    model_icons: Vec<ModelIcon>,
}

struct IconBuffers {
    cube_vbuf: wgpu::Buffer,
    cube_ibuf: wgpu::Buffer,
    mvp_bind: wgpu::BindGroup,
    model_vbuf: wgpu::Buffer,
    model_ibuf: wgpu::Buffer,
}

impl IconGeometry {
    fn build(count: u32, layout: IconLayout) -> Self {
        let screen = (CELL, CELL);
        let cell_rect = SlotRect {
            x: 0.0,
            y: 0.0,
            w: CELL as f32,
            h: CELL as f32,
        };
        let mut geometry = Self::default();
        for &item in ItemType::all() {
            if item == ItemType::Air {
                continue;
            }
            let i = item.id() as u32;
            let Some(cell) = layout.cell(i) else {
                continue;
            };
            let twin = layout.cell(count + i);
            match item.render_kind() {
                ItemRenderKind::BlockCube(block) => {
                    geometry.push_cube_icon(cell, twin, iso_icon_mvp(screen, cell_rect), |v, ix| {
                        push_block_item_cube(v, ix, block, Vec3::splat(-0.5), 1.0)
                    })
                }
                ItemRenderKind::Sprite(tile) => geometry.push_cube_icon(
                    cell,
                    twin,
                    flat_icon_mvp(screen, cell_rect),
                    |v, ix| push_billboard_quad(v, ix, tile, Vec3::ZERO, 1.0),
                ),
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

    fn push_cube_icon(
        &mut self,
        cell: (u32, u32),
        twin: Option<(u32, u32)>,
        mvp: glam::Mat4,
        push: impl Fn(&mut Vec<Vertex>, &mut Vec<u32>),
    ) {
        let mvp_offset = self.cube_mvps.len() as u32;
        self.cube_mvps.extend_from_slice(&mvp_slot_bytes(&mvp));
        let twin = twin.map(|twin| (twin, true));
        for ((col, row), dyed) in std::iter::once((cell, false)).chain(twin) {
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

    fn push_model_icon(
        &mut self,
        cell: (u32, u32),
        twin: Option<(u32, u32)>,
        push: impl FnOnce(&mut Vec<ItemVertex>, &mut Vec<u32>),
    ) {
        let index_start = self.model_indices.len() as u32;
        push(&mut self.model_verts, &mut self.model_indices);
        let index_count = self.model_indices.len() as u32 - index_start;
        for (col, row) in std::iter::once(cell).chain(twin) {
            self.model_icons.push(ModelIcon {
                col,
                row,
                index_start,
                index_count,
            });
        }
    }

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
        let empty_slot = [0u8; MVP_SLOT_SIZE as usize];
        let mvps = if self.cube_mvps.is_empty() {
            &empty_slot[..]
        } else {
            &self.cube_mvps[..]
        };
        let mvp_buf = buffer("icon bake mvp", mvps, wgpu::BufferUsages::UNIFORM);
        let mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icon bake mvp bg"),
            layout: model3d_mvp_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
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

fn dump_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    path: &str,
) {
    let row = w * 4;
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

fn set_cell(pass: &mut wgpu::RenderPass, col: u32, row: u32) {
    let (x, y) = ((col * CELL) as f32, (row * CELL) as f32);
    pass.set_viewport(x, y, CELL as f32, CELL as f32, 0.0, 1.0);
    pass.set_scissor_rect(col * CELL, row * CELL, CELL, CELL);
}

fn mvp_slot_bytes(mvp: &glam::Mat4) -> [u8; MVP_SLOT_SIZE as usize] {
    let mut slot = [0u8; MVP_SLOT_SIZE as usize];
    slot[..64].copy_from_slice(bytemuck::cast_slice(&mvp.to_cols_array()));
    slot
}

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

    #[test]
    fn every_item_gets_its_cell_and_a_dyed_twin() {
        let count = ItemType::all().len() as u32;
        let layout = IconLayout::new(2 * count, 8192);
        let geometry = IconGeometry::build(count, layout);
        let cell_of = |i: u32| layout.cell(i).expect("the catalogue fits 8192 px");
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
            assert!(
                unique.contains(&cell_of(count + id)),
                "item {id} has no dyed twin"
            );
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

    #[test]
    fn a_small_catalogue_keeps_the_strip() {
        let layout = IconLayout::new(2 * 100, 8192);
        assert_eq!(layout.cols, MIN_COLS);
        assert_eq!(layout.rows, 13);
        assert_eq!(layout.cell(17), Some((1, 1)));
    }

    #[test]
    fn a_large_catalogue_grows_square_within_the_limit() {
        for items in [1024u32, 3000, 8192] {
            let layout = IconLayout::new(2 * items, 8192);
            let (w, h) = layout.size();
            assert!(w <= 8192 && h <= 8192, "{items} items: {w}x{h}");
            assert!(layout.capacity() >= 2 * items, "{items} items fit");
            assert!(layout.cols.abs_diff(layout.rows) <= layout.cols / 2 + 1);
        }
    }

    #[test]
    fn a_catalogue_past_the_limit_is_capped() {
        let layout = IconLayout::new(100_000, 2048);
        assert_eq!(layout.size(), (2048, 2048));
        assert_eq!(layout.capacity(), 32 * 32);
        assert_eq!(layout.cell(layout.capacity()), None);
        assert!(layout.cell(layout.capacity() - 1).is_some());
    }
}
