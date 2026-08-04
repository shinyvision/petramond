//! Process-wide GPU texture byte accounting.
//!
//! wgpu reports nothing about resident texture memory, and VRAM is the scarce
//! resource on the target machine (8 GB). Terrain geometry is measured by the
//! geometry arena; this covers the other half — every texture the renderer
//! creates, billed by its own descriptor at creation.
//!
//! The total is GROSS, not net: a drop is not refunded, so a target recreated
//! on resize is counted twice. Modelling drops would need the census to own
//! every texture handle, and a figure that is honest about being an upper
//! bound is more useful than one that pretends to track a resource wgpu will
//! not report.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
static TEXTURE_COUNT: AtomicU64 = AtomicU64::new(0);
static BY_LABEL: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

/// Bill a texture by its descriptor (all mips, all array layers).
pub fn note_texture(desc: &wgpu::TextureDescriptor<'_>) {
    let bpp = match desc.format.block_copy_size(None) {
        Some(b) => b as u64,
        None => 4,
    };
    let layers = desc.size.depth_or_array_layers as u64;
    let mut bytes = 0u64;
    for level in 0..desc.mip_level_count {
        let w = (desc.size.width >> level).max(1) as u64;
        let h = (desc.size.height >> level).max(1) as u64;
        bytes += w * h * bpp * layers;
    }
    TEXTURE_BYTES.fetch_add(bytes, Relaxed);
    TEXTURE_COUNT.fetch_add(1, Relaxed);
    if let Ok(mut by) = BY_LABEL.lock() {
        let label = desc.label.unwrap_or("unlabelled");
        match by.iter_mut().find(|(l, _)| l == label) {
            Some((_, b)) => *b += bytes,
            None => by.push((label.to_string(), bytes)),
        }
    }
}

/// Create a texture and bill it. Every renderer texture goes through here.
pub fn create_texture(
    device: &wgpu::Device,
    desc: &wgpu::TextureDescriptor<'_>,
) -> wgpu::Texture {
    note_texture(desc);
    device.create_texture(desc)
}

/// `(total texture bytes, texture count)` created so far.
pub fn texture_totals() -> (u64, u64) {
    (TEXTURE_BYTES.load(Relaxed), TEXTURE_COUNT.load(Relaxed))
}

/// Texture bytes per descriptor label, largest first.
pub fn texture_by_label() -> Vec<(String, u64)> {
    let mut v = BY_LABEL.lock().map(|b| b.clone()).unwrap_or_default();
    v.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    v
}
