use crate::atlas::decode_atlas_mips;
use crate::chunk::{ChunkPos, SECTION_COUNT};
use crate::mesh::{ChunkMesh, MeshIndexSection};
use crate::texture_mips::build_cutout_mips;

use wgpu::util::DeviceExt;

/// A sprite composited into the GUI atlas (see [`create_gui_atlas`]). Each sprite
/// is loaded from its own PNG and blitted at a fixed pixel offset; [`rect`] gives
/// the sprite's UV rect (`u0,v0,u1,v1`) into the composited atlas and [`size_px`]
/// its source pixel dimensions, so the UI renderer can place + scale it without
/// hard-coding atlas math.
///
/// [`rect`]: GuiSprite::rect
/// [`size_px`]: GuiSprite::size_px
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GuiSprite {
    /// The 9-slot hotbar strip (182×22), drawn centered along the bottom.
    Hotbar,
    /// The selection highlight (24×23) drawn over the active hotbar slot.
    HotbarSelection,
    /// The full inventory panel sheet (256×256; the classic 176×166 panel sits in
    /// the top-left). Drawn centered when the inventory is open.
    InventoryPanel,
    /// A single slot frame (80×80) for the main-grid slots.
    SlotFrame,
    /// The crafting-table panel sheet (256×256; 176×166 art in the top-left,
    /// adding the 3×3 grid). Drawn centered when a crafting table is open.
    CraftingTablePanel,
    /// The furnace panel sheet (256×256; 176×166 art). Drawn when a furnace is open.
    FurnacePanel,
    /// The lit smelt-progress arrow (24×16), drawn cropped left→right by cook
    /// progress over the panel's empty arrow outline.
    FurnaceArrow,
    /// The lit fuel flame (14×14), drawn cropped bottom→up by remaining burn time
    /// over the panel's empty flame outline.
    FurnaceFlame,
    /// The chest panel sheet (256×256; 176×166 art with three storage rows at the
    /// top). Drawn when a chest is open. Reuses the vanilla single-container layout.
    ChestPanel,
}

/// All GUI sprites in atlas-layout order. Used for compositing + iteration.
const GUI_SPRITES: [GuiSprite; 9] = [
    GuiSprite::Hotbar,
    GuiSprite::HotbarSelection,
    GuiSprite::InventoryPanel,
    GuiSprite::SlotFrame,
    GuiSprite::CraftingTablePanel,
    GuiSprite::FurnacePanel,
    GuiSprite::FurnaceArrow,
    GuiSprite::FurnaceFlame,
    GuiSprite::ChestPanel,
];

/// Width / height (px) of the composited GUI atlas. Sprites are packed in a
/// single row at fixed offsets (see [`GuiSprite::atlas_offset_px`]); height is the
/// tallest sprite. Keep these in sync with the offsets/sizes below.
pub const GUI_ATLAS_W: u32 = 1348; // 182 + 24 + 256 + 80 + 256 + 256 + 24 + 14 + 256
pub const GUI_ATLAS_H: u32 = 256; // tallest sprite (inventory / table / furnace / chest panel)

impl GuiSprite {
    /// Source pixel size `(w, h)` of this sprite (matches its PNG dimensions).
    /// This is the size blitted into the composited atlas; for sprites whose PNG
    /// is larger than their drawn art (the inventory panel) the smaller drawn
    /// region is given by [`art_size_px`](Self::art_size_px).
    #[inline]
    pub fn size_px(self) -> (u32, u32) {
        match self {
            GuiSprite::Hotbar => (182, 22),
            GuiSprite::HotbarSelection => (24, 23),
            GuiSprite::InventoryPanel => (256, 256),
            GuiSprite::SlotFrame => (80, 80),
            GuiSprite::CraftingTablePanel => (256, 256),
            GuiSprite::FurnacePanel => (256, 256),
            GuiSprite::FurnaceArrow => (24, 16),
            GuiSprite::FurnaceFlame => (14, 14),
            GuiSprite::ChestPanel => (256, 256),
        }
    }

    /// The drawable art size `(w, h)` of this sprite — the sub-rect at the
    /// sprite's top-left that holds the actual graphic. Equals [`size_px`] for
    /// every sprite except the inventory panel, whose `inventory.png` is a
    /// 256×256 sheet with the classic panel art in only the top-left 176×166;
    /// [`rect`](Self::rect) addresses exactly this region so the drawn UV and the
    /// on-screen panel rect (`PANEL_W`×`PANEL_H`) match 1:1 instead of squashing
    /// the whole sheet into the smaller panel rect.
    ///
    /// [`size_px`]: Self::size_px
    #[inline]
    pub fn art_size_px(self) -> (u32, u32) {
        match self {
            GuiSprite::InventoryPanel
            | GuiSprite::CraftingTablePanel
            | GuiSprite::FurnacePanel
            | GuiSprite::ChestPanel => (176, 166),
            other => other.size_px(),
        }
    }

    /// Top-left pixel offset of this sprite within the composited GUI atlas.
    /// Sprites are laid out left-to-right in a single row.
    #[inline]
    pub fn atlas_offset_px(self) -> (u32, u32) {
        // Cumulative x offsets following GUI_SPRITES order.
        match self {
            GuiSprite::Hotbar => (0, 0),
            GuiSprite::HotbarSelection => (182, 0),
            GuiSprite::InventoryPanel => (206, 0),
            GuiSprite::SlotFrame => (462, 0),
            GuiSprite::CraftingTablePanel => (542, 0),
            GuiSprite::FurnacePanel => (798, 0),
            GuiSprite::FurnaceArrow => (1054, 0),
            GuiSprite::FurnaceFlame => (1078, 0),
            GuiSprite::ChestPanel => (1092, 0),
        }
    }

    /// Embedded PNG bytes for this sprite (compiled in via `include_bytes!`).
    #[inline]
    fn png_bytes(self) -> &'static [u8] {
        match self {
            GuiSprite::Hotbar => {
                include_bytes!("../../assets/textures/gui/hotbar.png")
            }
            GuiSprite::HotbarSelection => {
                include_bytes!("../../assets/textures/gui/hotbar_selection.png")
            }
            GuiSprite::InventoryPanel => {
                include_bytes!("../../assets/textures/gui/inventory.png")
            }
            GuiSprite::SlotFrame => {
                include_bytes!("../../assets/textures/gui/slot_frame.png")
            }
            GuiSprite::CraftingTablePanel => {
                include_bytes!("../../assets/textures/gui/crafting_table.png")
            }
            GuiSprite::FurnacePanel => {
                include_bytes!("../../assets/textures/gui/furnace.png")
            }
            GuiSprite::FurnaceArrow => {
                include_bytes!("../../assets/textures/gui/furnace_arrow.png")
            }
            GuiSprite::FurnaceFlame => {
                include_bytes!("../../assets/textures/gui/furnace_flame.png")
            }
            GuiSprite::ChestPanel => {
                include_bytes!("../../assets/textures/gui/chest.png")
            }
        }
    }

    /// UV rect `(u0, v0, u1, v1)` of this sprite's drawable art within the
    /// composited atlas. Addresses only the [`art_size_px`](Self::art_size_px)
    /// sub-rect at the sprite's top-left (which equals the full sprite for all but
    /// the inventory panel), so the UI samples just the panel graphic, not the
    /// empty margins of `inventory.png`.
    #[inline]
    pub fn rect(self) -> [f32; 4] {
        let (ox, oy) = self.atlas_offset_px();
        let (w, h) = self.art_size_px();
        let u0 = ox as f32 / GUI_ATLAS_W as f32;
        let v0 = oy as f32 / GUI_ATLAS_H as f32;
        let u1 = (ox + w) as f32 / GUI_ATLAS_W as f32;
        let v1 = (oy + h) as f32 / GUI_ATLAS_H as f32;
        [u0, v0, u1, v1]
    }
}

/// Load the four GUI sprite PNGs and composite them into ONE RGBA texture at the
/// fixed offsets in [`GuiSprite::atlas_offset_px`]. Filtering is NEAREST (crisp
/// pixel UI). Returns the texture, a default view, and the sampler; the per-
/// sprite UV rects come from [`GuiSprite::rect`]. This is a separate atlas from
/// the block atlas (different bind group), per the UI render contract.
pub(super) fn create_gui_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let mut rgba = vec![0u8; (GUI_ATLAS_W * GUI_ATLAS_H * 4) as usize];
    for sprite in GUI_SPRITES {
        let img = image::load_from_memory(sprite.png_bytes())
            .expect("decode gui sprite")
            .to_rgba8();
        let (sw, sh) = (img.width(), img.height());
        let (ox, oy) = sprite.atlas_offset_px();
        let src = img.as_raw();
        for y in 0..sh {
            let dst_row = (((oy + y) * GUI_ATLAS_W + ox) * 4) as usize;
            let src_row = (y * sw * 4) as usize;
            let bytes = (sw * 4) as usize;
            rgba[dst_row..dst_row + bytes].copy_from_slice(&src[src_row..src_row + bytes]);
        }
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui atlas"),
        size: wgpu::Extent3d {
            width: GUI_ATLAS_W,
            height: GUI_ATLAS_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(GUI_ATLAS_W * 4),
            rows_per_image: Some(GUI_ATLAS_H),
        },
        wgpu::Extent3d {
            width: GUI_ATLAS_W,
            height: GUI_ATLAS_H,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("gui atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// Upload a baked data-driven GUI panel PNG (from the `gui-builder`) as its own
/// texture + nearest sampler (sRGB, like the gui atlas). Arbitrary size — each
/// baked panel is its own image, not a fixed atlas slot. See `super::gui_def`.
pub(super) fn create_gui_panel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    png: &[u8],
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let img = image::load_from_memory(png)
        .expect("decode gui panel png")
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui panel"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        img.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("gui panel sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// Upload an entity/model RGBA texture (decoded from a `.bbmodel`) as its own GPU
/// texture + nearest sampler — a SEPARATE atlas from the block atlas, because model
/// faces carry arbitrary sub-rectangle UVs into this sheet (see `crate::bbmodel`).
/// Mips use cutout-alpha expansion so thin transparent decals, like the workbench's
/// tabletop grid, stay stable at distance under the shader's alpha test.
///
/// `w`/`h` of 0 are clamped to 1 so a missing/empty texture still yields a valid 1×1
/// binding.
pub(super) fn create_model_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let w = w.max(1);
    let h = h.max(1);
    let mips = build_cutout_mips(rgba, w, h);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("entity model texture"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, mip) in mips.iter().enumerate() {
        let level_w = (w >> level).max(1);
        let level_h = (h >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            mip,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level_w * 4),
                rows_per_image: Some(level_h),
            },
            wgpu::Extent3d {
                width: level_w,
                height: level_h,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("entity model sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: (mips.len() - 1) as f32,
        ..Default::default()
    });
    (texture, view, sampler)
}

pub struct GpuMesh {
    pub opaque_vbuf: Option<wgpu::Buffer>,
    pub opaque_ibuf: Option<wgpu::Buffer>,
    pub opaque_idx_count: u32,
    pub opaque_sections: [MeshIndexSection; SECTION_COUNT],
    pub far_opaque_vbuf: Option<wgpu::Buffer>,
    pub far_opaque_ibuf: Option<wgpu::Buffer>,
    pub far_opaque_idx_count: u32,
    pub far_opaque_sections: [MeshIndexSection; SECTION_COUNT],
    pub transparent_vbuf: Option<wgpu::Buffer>,
    pub transparent_ibuf: Option<wgpu::Buffer>,
    pub transparent_idx_count: u32,
    pub transparent_sections: [MeshIndexSection; SECTION_COUNT],
    /// bbmodel-block geometry (explicit-UV [`ModelVertex`], sampling the model atlas),
    /// drawn in the model pass. `None`/`0` for the common chunk.
    pub model_vbuf: Option<wgpu::Buffer>,
    pub model_ibuf: Option<wgpu::Buffer>,
    pub model_idx_count: u32,
    pub pos: ChunkPos,
    pub origin: (i32, i32),
}

pub(super) fn create_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (mips, w, h) = decode_atlas_mips();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlas"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, rgba) in mips.iter().enumerate() {
        let level_w = (w >> level).max(1);
        let level_h = (h >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level_w * 4),
                rows_per_image: Some(level_h),
            },
            wgpu::Extent3d {
                width: level_w,
                height: level_h,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: (mips.len() - 1) as f32,
        ..Default::default()
    });
    (texture, view, sampler)
}

pub(super) fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn upload_mesh(device: &wgpu::Device, mesh: &ChunkMesh, pos: ChunkPos) -> GpuMesh {
    let opaque_vbuf = if mesh.opaque.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.opaque),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let opaque_ibuf = if mesh.opaque_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.opaque_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let far_opaque_vbuf = if mesh.far_opaque.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.far_opaque),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let far_opaque_ibuf = if mesh.far_opaque_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.far_opaque_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let transparent_vbuf = if mesh.transparent.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.transparent),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let transparent_ibuf = if mesh.transparent_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.transparent_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let model_vbuf = if mesh.model.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.model),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let model_ibuf = if mesh.model_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.model_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    GpuMesh {
        opaque_vbuf,
        opaque_ibuf,
        opaque_idx_count: mesh.opaque_idx.len() as u32,
        opaque_sections: mesh.opaque_sections,
        far_opaque_vbuf,
        far_opaque_ibuf,
        far_opaque_idx_count: mesh.far_opaque_idx.len() as u32,
        far_opaque_sections: mesh.far_opaque_sections,
        transparent_vbuf,
        transparent_ibuf,
        transparent_idx_count: mesh.transparent_idx.len() as u32,
        transparent_sections: mesh.transparent_sections,
        model_vbuf,
        model_ibuf,
        model_idx_count: mesh.model_idx.len() as u32,
        pos,
        origin: (pos.cx * 16, pos.cz * 16),
    }
}
