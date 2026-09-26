//! The arena's allocation policy as plain bookkeeping: which block slots
//! exist, how far each is bumped, what is free by size class, and when an
//! emptied block is released. No GPU object lives here — [`GeometryArena`]
//! pairs each live slot with its buffer — so the whole policy is testable
//! against a model without a device.
//!
//! [`GeometryArena`]: super::GeometryArena

use std::collections::HashMap;

use super::class_size;

/// One block's accounting. A block whose last live allocation goes away is
/// RELEASED (slot left empty, reused by the next new block): travelling
/// across a world retires whole neighbourhoods at once, and without this the
/// arena would settle at the peak of every size class it ever saw rather
/// than at what is loaded.
struct BlockBook {
    /// Bytes handed out from the front; the tail is virgin space.
    bump: u64,
    size: u64,
    /// Allocations handed out and not yet recycled.
    live: u32,
}

/// Where an allocation landed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct Placement {
    pub(super) block: u32,
    pub(super) offset: u64,
    /// The class size the allocation may grow into.
    pub(super) capacity: u64,
    /// `Some(size)` when the placement opened a new block of that size: the
    /// caller must back the slot with a buffer before using it.
    pub(super) new_block: Option<u64>,
}

pub(super) struct Book {
    /// The element size every class (and so every offset) is a whole
    /// multiple of.
    unit: u64,
    /// Size of a fresh block, a whole number of units.
    block_bytes: u64,
    /// Slots, not a dense list: a released block leaves a hole so live
    /// allocations' block indices stay valid.
    blocks: Vec<Option<BlockBook>>,
    /// Freed allocations by class size. Same-class allocations are
    /// interchangeable, so this needs no search.
    free: HashMap<u64, Vec<(u32, u64)>>,
}

impl Book {
    /// Bookkeeping for an arena of `unit`-byte elements in blocks of about
    /// `block_bytes`.
    pub(super) fn new(unit: u64, block_bytes: u64) -> Self {
        Self {
            unit,
            block_bytes: (block_bytes / unit).max(1) * unit,
            blocks: Vec::new(),
            free: HashMap::new(),
        }
    }

    /// Claim `len` bytes: a freed allocation of the same class first, then
    /// bump space in any live block, then a fresh block (a request larger
    /// than a block gets a block of its own). Never fails.
    pub(super) fn place(&mut self, len: u64) -> Placement {
        let capacity = class_size(len, self.unit);
        if let Some((block, offset)) = self.free.get_mut(&capacity).and_then(Vec::pop) {
            self.blocks[block as usize]
                .as_mut()
                .expect("free entry in a released arena block")
                .live += 1;
            return Placement {
                block,
                offset,
                capacity,
                new_block: None,
            };
        }
        for (i, b) in self.blocks.iter_mut().enumerate() {
            let Some(b) = b.as_mut() else { continue };
            if b.size - b.bump >= capacity {
                let offset = b.bump;
                b.bump += capacity;
                b.live += 1;
                return Placement {
                    block: i as u32,
                    offset,
                    capacity,
                    new_block: None,
                };
            }
        }
        let size = capacity.max(self.block_bytes);
        let block = BlockBook {
            bump: capacity,
            size,
            live: 1,
        };
        let index = match self.blocks.iter().position(Option::is_none) {
            Some(i) => {
                self.blocks[i] = Some(block);
                i
            }
            None => {
                self.blocks.push(Some(block));
                self.blocks.len() - 1
            }
        };
        Placement {
            block: index as u32,
            offset: 0,
            capacity,
            new_block: Some(size),
        }
    }

    /// Fold dropped allocations — `(capacity, block, offset)` — into the free
    /// lists and release the blocks they emptied. Returns the released slots,
    /// whose buffers the caller drops.
    pub(super) fn reclaim(&mut self, recycled: Vec<(u64, u32, u64)>) -> Vec<u32> {
        let mut emptied = false;
        for (capacity, block, offset) in recycled {
            self.free.entry(capacity).or_default().push((block, offset));
            if let Some(b) = self.blocks[block as usize].as_mut() {
                b.live -= 1;
                emptied |= b.live == 0;
            }
        }
        let mut released = Vec::new();
        if !emptied {
            return released;
        }
        // Keep ONE empty block as a spare: the streaming frontier empties and
        // refills the tail of the arena continuously, and releasing on the
        // first zero would trade a GPU allocation for every wobble.
        let mut spare = false;
        for (i, slot) in self.blocks.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|b| b.live == 0) {
                if !spare {
                    spare = true;
                    continue;
                }
                *slot = None;
                released.push(i as u32);
            }
        }
        // A released block's free entries would hand out memory that no
        // longer exists.
        let blocks = &self.blocks;
        self.free.retain(|_, list| {
            list.retain(|(block, _)| blocks[*block as usize].is_some());
            !list.is_empty()
        });
        released
    }

    /// Total bytes of the live blocks.
    pub(super) fn reserved_bytes(&self) -> u64 {
        self.blocks.iter().flatten().map(|b| b.size).sum()
    }

    pub(super) fn block_count(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    /// Bytes sitting in the free lists.
    pub(super) fn free_bytes(&self) -> u64 {
        self.free
            .iter()
            .map(|(size, list)| size * list.len() as u64)
            .sum()
    }
}
