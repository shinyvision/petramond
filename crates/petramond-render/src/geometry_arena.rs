//! Suballocated GPU storage for the packed terrain columns.
//!
//! Every terrain layer of every column used to own a `wgpu::Buffer` — measured
//! at render distance 32, **11 138 live buffer objects**. wgpu validates and
//! resource-tracks per DISTINCT buffer a command buffer touches, and a frame
//! that binds ~3 700 of them pays for it on the render thread: an ablation that
//! pointed every terrain draw at one shared buffer (identical command count,
//! identical draw count — only the number of distinct resources changed) cut
//! `CommandEncoder::finish` from 0.80 to 0.37 ms, `Queue::submit` from 0.15 to
//! 0.02 ms and pass encoding from 0.30 to 0.17 ms.
//!
//! So the arena changes nothing about how terrain is packed, culled, ordered or
//! drawn: it is still one column's geometry per draw, and a repack still
//! rewrites only that column. Only the ALLOCATION moves — into a handful of
//! large blocks that every draw slices into.
//!
//! Allocation is size-classed rather than free-list-coalesced, which makes both
//! `alloc` and `free` O(1) and removes fragmentation search entirely. A class
//! rounds up to an eighth of the next power of two, so the rounding waste is
//! bounded by 12.5% — the same order as the growth headroom the per-buffer
//! policy already carried, and it doubles as that headroom (a column that
//! remeshes slightly larger stays inside its class and writes in place).

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
    /// The CLASS size — what the allocation may grow into, not what is in use.
    capacity: u64,
    recycle: Recycle,
}

/// Space handed back by dropped allocations, drained into the arena's free
/// lists on the next `alloc`. Shared (not owned by the arena) precisely so a
/// drop needs no access to the arena.
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

/// Block size. The tail of the last block is the arena's only real overhead, so
/// this trades reserved VRAM at LOW render distance (a 64 MiB block reserved
/// 128 MiB for 94 MiB of terrain at RD16) against the number of distinct
/// buffers a frame binds — and the CPU win is flat from a handful of blocks up
/// to a few dozen, because the cost was per-buffer over THOUSANDS.
const BLOCK_BYTES: u64 = 16 * 1024 * 1024;

/// Smallest class, and the granularity below `MIN_CLASS * 8`. Also satisfies
/// every wgpu offset alignment the terrain path needs (vertex/index binds and
/// `write_buffer` want 4).
const MIN_CLASS: u64 = 512;

/// Round `len` up to its size class: multiples of [`MIN_CLASS`] up to
/// `MIN_CLASS * 8`, then eighths of the enclosing power of two.
pub(super) fn class_size(len: u64) -> u64 {
    let len = len.max(1);
    if len <= MIN_CLASS * 8 {
        return len.div_ceil(MIN_CLASS) * MIN_CLASS;
    }
    let step = 1u64 << (63 - (len - 1).leading_zeros() as u64 - 3);
    len.div_ceil(step) * step
}

pub struct GeometryArena {
    /// The allocation policy's bookkeeping (see [`book`]).
    book: Book,
    /// One buffer per live block slot of the book; a released slot is `None`
    /// so live [`LayerAlloc`] block indices stay valid.
    buffers: Vec<Option<wgpu::Buffer>>,
    recycle: Recycle,
    usage: wgpu::BufferUsages,
    copy_scratch: Option<wgpu::Buffer>,
}

impl Default for GeometryArena {
    fn default() -> Self {
        Self::new()
    }
}

impl GeometryArena {
    pub fn new() -> Self {
        Self {
            book: Book::default(),
            buffers: Vec::new(),
            recycle: Recycle::default(),
            copy_scratch: None,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        }
    }

    /// Total bytes of GPU buffer the arena holds.
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

    /// The bound range for a live allocation, `len` bytes from its start.
    pub fn slice(&self, alloc: &LayerAlloc, len: u64) -> wgpu::BufferSlice<'_> {
        self.block(alloc.block).slice(alloc.offset..alloc.offset + len)
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

    /// Write `bytes` at `offset` inside a live allocation. Returns false when
    /// the write would leave the allocation, which is the caller's cue that the
    /// GPU copy is stale and must be repacked.
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

    /// Copy live geometry between distinct allocations without a CPU readback.
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
            // wgpu forbids even disjoint copies within one buffer.
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

    /// Claim `len` bytes. Never fails: a request larger than a block gets a
    /// block of its own.
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

    /// Fold dropped allocations into the free lists, dropping the buffer of
    /// every block they emptied past the spare.
    fn reclaim(&mut self) {
        let recycled = match self.recycle.lock() {
            Ok(mut r) if !r.is_empty() => std::mem::take(&mut *r),
            _ => return,
        };
        for block in self.book.reclaim(recycled) {
            self.buffers[block as usize] = None;
        }
    }

    /// Bytes currently sitting in the free lists (including drop-recycled ones)
    /// — arena space that is reserved but not handed to any column.
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
