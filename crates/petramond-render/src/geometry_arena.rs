//! Suballocated GPU storage for the packed terrain columns.
//!
//! Terrain layers share large GPU buffers because wgpu validates and tracks
//! each distinct buffer bound by a command buffer. Suballocation keeps the
//! number of bound resources small at high render distance.
//!
//! A repack still rewrites only its column; only the ALLOCATION is shared —
//! a handful of large blocks every draw addresses. An arena is built for one
//! element UNIT (a vertex stride, or 4 for mixed byte streams) and hands out
//! only whole multiples of it at unit-aligned offsets, so a draw can bind a
//! whole block once and reach any allocation in it by `base_vertex`
//! ([`GeometryArena::first_element`]): that is what lets the terrain passes
//! batch many columns into one indirect multi-draw.
//!
//! Allocation is size-classed rather than free-list-coalesced, which makes both
//! `alloc` and `free` O(1) and removes fragmentation search entirely. A class
//! rounds up to an eighth of the next power of two (in units), so the rounding
//! waste is bounded by 12.5% — the same order as the growth headroom the
//! per-buffer policy already carried, and it doubles as that headroom (a column
//! that remeshes slightly larger stays inside its class and writes in place).

mod book;
use book::Book;

/// A live suballocation. Neither `Copy` nor `Clone`: it returns its space to
/// the arena on DROP, which is what makes the arena leak-proof. A packed column
/// is dropped from half a dozen places (`retain`, `remove`, `clear`, replacement
/// during a repack) and an explicit free would eventually be forgotten at one of
/// them — the arena would then grow without bound as the player travels.
#[derive(Debug)]
pub struct LayerAlloc {
    block: u32,
    offset: u64,
    capacity: u64,
    recycle: Recycle,
}

type Recycle = std::sync::Arc<std::sync::Mutex<Vec<(u64, u32, u64)>>>;

impl LayerAlloc {
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}

impl Drop for LayerAlloc {
    fn drop(&mut self) {
        if let Ok(mut r) = self.recycle.lock() {
            r.push((self.capacity, self.block, self.offset));
        }
    }
}

const BLOCK_BYTES: u64 = 16 * 1024 * 1024;

const MIN_CLASS: u64 = 512;

pub(super) fn class_size(len: u64, unit: u64) -> u64 {
    let units = len.max(1).div_ceil(unit);
    let min = (MIN_CLASS / unit).max(1);
    let classes = if units <= min * 8 {
        units.div_ceil(min) * min
    } else {
        let step = 1u64 << (63 - (units - 1).leading_zeros() as u64 - 3);
        units.div_ceil(step) * step
    };
    classes * unit
}

pub struct GeometryArena {
    book: Book,
    buffers: Vec<Option<wgpu::Buffer>>,
    recycle: Recycle,
    usage: wgpu::BufferUsages,
    copy_scratch: Option<wgpu::Buffer>,
    unit: u64,
}

impl GeometryArena {
    pub fn new(unit: u64, block_bytes: u64) -> Self {
        assert!(
            unit > 0 && unit.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
            "arena unit {unit} breaks wgpu's copy alignment"
        );
        Self {
            book: Book::new(unit, block_bytes),
            buffers: Vec::new(),
            recycle: Recycle::default(),
            copy_scratch: None,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            unit,
        }
    }

    pub fn with_unit(unit: u64) -> Self {
        Self::new(unit, BLOCK_BYTES)
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.book.reserved_bytes() + self.copy_scratch.as_ref().map_or(0, |b| b.size())
    }

    pub fn block_count(&self) -> usize {
        self.book.block_count()
    }

    #[inline]
    fn block(&self, index: u32) -> &wgpu::Buffer {
        self.buffers[index as usize]
            .as_ref()
            .expect("live allocation in a released arena block")
    }

    pub fn slice(&self, alloc: &LayerAlloc, len: u64) -> wgpu::BufferSlice<'_> {
        self.block(alloc.block)
            .slice(alloc.offset..alloc.offset + len)
    }

    pub fn block_buffer(&self, index: u32) -> &wgpu::Buffer {
        self.block(index)
    }

    pub fn first_element(&self, alloc: &LayerAlloc) -> (u32, u32) {
        debug_assert_eq!(alloc.offset % self.unit, 0);
        (alloc.block, (alloc.offset / self.unit) as u32)
    }

    #[cfg(test)]
    pub(crate) fn readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        alloc: &LayerAlloc,
        len: u64,
    ) -> Vec<u8> {
        let dst = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain test readback"),
            size: len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(self.block(alloc.block), alloc.offset, &dst, 0, len);
        queue.submit([encoder.finish()]);
        dst.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .unwrap();
        let bytes = dst.slice(..).get_mapped_range().to_vec();
        dst.unmap();
        bytes
    }

    pub fn write(
        &self,
        queue: &wgpu::Queue,
        alloc: &LayerAlloc,
        offset: u64,
        bytes: &[u8],
    ) -> bool {
        if offset + bytes.len() as u64 > alloc.capacity {
            return false;
        }
        queue.write_buffer(self.block(alloc.block), alloc.offset + offset, bytes);
        true
    }

    pub fn write_with(
        &self,
        queue: &wgpu::Queue,
        alloc: &LayerAlloc,
        offset: u64,
        len: u64,
        fill: impl FnOnce(&mut [u8]),
    ) -> bool {
        if offset + len > alloc.capacity {
            return false;
        }
        let Some(size) = wgpu::BufferSize::new(len) else {
            return true;
        };
        match queue.write_buffer_with(self.block(alloc.block), alloc.offset + offset, size) {
            Some(mut view) => {
                fill(&mut view);
                true
            }
            None => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &LayerAlloc,
        src_offset: u64,
        dst: &LayerAlloc,
        dst_offset: u64,
        len: u64,
    ) {
        assert!(src_offset + len <= src.capacity && dst_offset + len <= dst.capacity);
        if src.block == dst.block {
            if self.copy_scratch.as_ref().is_none_or(|b| b.size() < len) {
                self.copy_scratch = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("terrain relocation scratch"),
                    size: len.next_power_of_two().max(4),
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let scratch = self.copy_scratch.as_ref().unwrap();
            encoder.copy_buffer_to_buffer(
                self.block(src.block),
                src.offset + src_offset,
                scratch,
                0,
                len,
            );
            encoder.copy_buffer_to_buffer(
                scratch,
                0,
                self.block(dst.block),
                dst.offset + dst_offset,
                len,
            );
        } else {
            encoder.copy_buffer_to_buffer(
                self.block(src.block),
                src.offset + src_offset,
                self.block(dst.block),
                dst.offset + dst_offset,
                len,
            );
        }
    }

    pub fn alloc(&mut self, device: &wgpu::Device, len: u64) -> LayerAlloc {
        self.reclaim();
        let placed = self.book.place(len);
        if let Some(size) = placed.new_block {
            let slot = placed.block as usize;
            if self.buffers.len() <= slot {
                self.buffers.resize_with(slot + 1, || None);
            }
            self.buffers[slot] = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terrain geometry arena"),
                size,
                usage: self.usage,
                mapped_at_creation: false,
            }));
        }
        LayerAlloc {
            block: placed.block,
            offset: placed.offset,
            capacity: placed.capacity,
            recycle: std::sync::Arc::clone(&self.recycle),
        }
    }

    fn reclaim(&mut self) {
        let recycled = match self.recycle.lock() {
            Ok(mut r) if !r.is_empty() => std::mem::take(&mut *r),
            _ => return,
        };
        for block in self.book.reclaim(recycled) {
            self.buffers[block as usize] = None;
        }
    }

    pub fn free_bytes(&self) -> u64 {
        let listed = self.book.free_bytes();
        let pending: u64 = self
            .recycle
            .lock()
            .map(|r| r.iter().map(|(size, _, _)| *size).sum())
            .unwrap_or(0);
        listed + pending
    }
}

#[cfg(test)]
mod tests;
