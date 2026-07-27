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

/// One block of arena storage. A block whose last live allocation goes away is
/// RELEASED (slot left empty, reused by the next new block): travelling across
/// a world retires whole neighbourhoods at once, and without this the arena
/// would settle at the peak of every size class it ever saw rather than at what
/// is loaded.
struct Block {
    buf: wgpu::Buffer,
    /// Bytes handed out from the front; the tail is virgin space.
    bump: u64,
    size: u64,
    /// Allocations handed out and not yet recycled.
    live: u32,
}

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
    /// Slots, not a dense list: a released block leaves a hole so live
    /// [`LayerAlloc`] block indices stay valid.
    blocks: Vec<Option<Block>>,
    /// Freed allocations by class size. Same-class allocations are
    /// interchangeable, so this needs no search.
    free: std::collections::HashMap<u64, Vec<(u32, u64)>>,
    recycle: Recycle,
    usage: wgpu::BufferUsages,
}

impl GeometryArena {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            free: std::collections::HashMap::new(),
            recycle: Recycle::default(),
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_DST,
        }
    }

    /// Total bytes of GPU buffer the arena holds.
    pub fn reserved_bytes(&self) -> u64 {
        self.blocks.iter().flatten().map(|b| b.size).sum()
    }

    pub fn block_count(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    #[inline]
    fn block(&self, index: u32) -> &Block {
        self.blocks[index as usize]
            .as_ref()
            .expect("live allocation in a released arena block")
    }

    /// The bound range for a live allocation, `len` bytes from its start.
    pub fn slice(&self, alloc: &LayerAlloc, len: u64) -> wgpu::BufferSlice<'_> {
        let b = self.block(alloc.block);
        b.buf.slice(alloc.offset..alloc.offset + len)
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
        let b = self.block(alloc.block);
        queue.write_buffer(&b.buf, alloc.offset + offset, bytes);
        true
    }

    /// Claim `len` bytes. Never fails: a request larger than a block gets a
    /// block of its own.
    pub fn alloc(&mut self, device: &wgpu::Device, len: u64) -> LayerAlloc {
        self.reclaim();
        let capacity = class_size(len);
        if let Some(list) = self.free.get_mut(&capacity) {
            if let Some((block, offset)) = list.pop() {
                self.blocks[block as usize]
                    .as_mut()
                    .expect("free entry in a released arena block")
                    .live += 1;
                return LayerAlloc {
                    block,
                    offset,
                    capacity,
                    recycle: std::sync::Arc::clone(&self.recycle),
                };
            }
        }
        for (i, b) in self.blocks.iter_mut().enumerate() {
            let Some(b) = b.as_mut() else { continue };
            if b.size - b.bump >= capacity {
                let offset = b.bump;
                b.bump += capacity;
                b.live += 1;
                return LayerAlloc {
                    block: i as u32,
                    offset,
                    capacity,
                    recycle: std::sync::Arc::clone(&self.recycle),
                };
            }
        }
        let size = capacity.max(BLOCK_BYTES);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain geometry arena"),
            size,
            usage: self.usage,
            mapped_at_creation: false,
        });
        let block = Block {
            buf,
            bump: capacity,
            size,
            live: 1,
        };
        let index = match self.blocks.iter().position(|b| b.is_none()) {
            Some(i) => {
                self.blocks[i] = Some(block);
                i
            }
            None => {
                self.blocks.push(Some(block));
                self.blocks.len() - 1
            }
        };
        LayerAlloc {
            block: index as u32,
            offset: 0,
            capacity,
            recycle: std::sync::Arc::clone(&self.recycle),
        }
    }

    /// Fold dropped allocations into the free lists, releasing any block they
    /// emptied.
    fn reclaim(&mut self) {
        let Ok(mut r) = self.recycle.lock() else {
            return;
        };
        if r.is_empty() {
            return;
        }
        let mut emptied = false;
        for (capacity, block, offset) in r.drain(..) {
            self.free.entry(capacity).or_default().push((block, offset));
            if let Some(b) = self.blocks[block as usize].as_mut() {
                b.live -= 1;
                emptied |= b.live == 0;
            }
        }
        drop(r);
        if !emptied {
            return;
        }
        // Keep ONE empty block as a spare: the streaming frontier empties and
        // refills the tail of the arena continuously, and releasing on the
        // first zero would trade a GPU allocation for every wobble.
        let mut spare = false;
        for slot in &mut self.blocks {
            if slot.as_ref().is_some_and(|b| b.live == 0) {
                if !spare {
                    spare = true;
                    continue;
                }
                *slot = None;
            }
        }
        // A released block's free entries would hand out memory that no longer
        // exists.
        self.free.retain(|_, list| {
            list.retain(|(block, _)| self.blocks[*block as usize].is_some());
            !list.is_empty()
        });
    }

    /// Bytes currently sitting in the free lists (including drop-recycled ones)
    /// — arena space that is reserved but not handed to any column.
    pub fn free_bytes(&self) -> u64 {
        let listed: u64 = self
            .free
            .iter()
            .map(|(size, list)| size * list.len() as u64)
            .sum();
        let pending: u64 = self
            .recycle
            .lock()
            .map(|r| r.iter().map(|(size, _, _)| *size).sum())
            .unwrap_or(0);
        listed + pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The class function is the arena's whole memory policy: it must never
    /// round DOWN (that would hand out an allocation the caller overruns) and
    /// its waste must stay bounded, or terrain VRAM balloons silently.
    #[test]
    fn size_classes_cover_the_request_with_bounded_waste() {
        for len in (1u64..1 << 22).step_by(97) {
            let c = class_size(len);
            assert!(c >= len, "class {c} smaller than {len}");
            assert!(
                c <= len + MIN_CLASS.max(len / 8),
                "class {c} wastes too much on {len}"
            );
            assert_eq!(c % 4, 0, "class {c} breaks wgpu's 4-byte alignment");
        }
    }

    /// Same-class frees must be reusable by any same-class request — that is
    /// what keeps allocation O(1) with no fragmentation search.
    #[test]
    fn freed_allocations_are_reused_by_the_same_class() {
        assert_eq!(class_size(5000), class_size(5100));
        assert_ne!(class_size(5000), class_size(9000));
    }
}
