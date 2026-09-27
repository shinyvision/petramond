use std::collections::HashMap;

use super::class_size;

struct BlockBook {
    bump: u64,
    size: u64,
    live: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct Placement {
    pub(super) block: u32,
    pub(super) offset: u64,
    pub(super) capacity: u64,
    pub(super) new_block: Option<u64>,
}

pub(super) struct Book {
    unit: u64,
    block_bytes: u64,
    blocks: Vec<Option<BlockBook>>,
    free: HashMap<u64, Vec<(u32, u64)>>,
}

impl Book {
    pub(super) fn new(unit: u64, block_bytes: u64) -> Self {
        Self {
            unit,
            block_bytes: (block_bytes / unit).max(1) * unit,
            blocks: Vec::new(),
            free: HashMap::new(),
        }
    }

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
        let blocks = &self.blocks;
        self.free.retain(|_, list| {
            list.retain(|(block, _)| blocks[*block as usize].is_some());
            !list.is_empty()
        });
        released
    }

    pub(super) fn reserved_bytes(&self) -> u64 {
        self.blocks.iter().flatten().map(|b| b.size).sum()
    }

    pub(super) fn block_count(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    pub(super) fn free_bytes(&self) -> u64 {
        self.free
            .iter()
            .map(|(size, list)| size * list.len() as u64)
            .sum()
    }
}
