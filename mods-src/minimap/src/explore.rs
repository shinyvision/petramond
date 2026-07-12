//! Exploration cache: revision-gated host surface sampling, the 16×16
//! explored-tile store with its LRU working set, debounced persistence,
//! and relief shading.

use crate::*;

pub(crate) const SAMPLE_RADIUS: i32 = 96;
pub(crate) const SAMPLE_STEP: i32 = 8;
const TILE_PREFIX: &str = "minimap:tile:";
const TILE_CACHE_MAX: usize = 2048;
/// Evict in one batch once the cache overshoots by this much, instead of a
/// full O(cache) scan per evicted tile.
const TILE_CACHE_SLACK: usize = 64;
/// Frames between persistence flushes of changed tiles (~2 s at 60 fps): a
/// frontier tile fills over many samples but is written once per interval.
pub(crate) const FLUSH_INTERVAL: u64 = 120;
pub(crate) const UNKNOWN_HEIGHT: i16 = i16::MIN;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) height: i16,
    pub(crate) rgb: [u8; 3],
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            height: UNKNOWN_HEIGHT,
            rgb: [0; 3],
        }
    }
}

#[derive(Clone)]
pub(crate) struct Tile {
    pub(crate) cells: [Cell; 256],
}

#[derive(Clone)]
pub(crate) struct CachedTile {
    pub(crate) tile: Tile,
    pub(crate) last_used: u64,
    /// Host column revision this tile was last seen COMPLETE at (0 = never):
    /// echoed back so an unchanged column costs no cell bytes. Session-local
    /// replica state — never persisted with the tile.
    pub(crate) watermark: u64,
    /// Changed since the last persistence flush.
    pub(crate) dirty: bool,
}

impl Default for Tile {
    fn default() -> Self {
        Self {
            cells: [Cell::default(); 256],
        }
    }
}

impl Minimap {
    pub(crate) fn refresh_surface(&mut self, center: (i32, i32)) {
        let min = (
            (center.0 - SAMPLE_RADIUS).div_euclid(16),
            (center.1 - SAMPLE_RADIUS).div_euclid(16),
        );
        let max = (
            (center.0 + SAMPLE_RADIUS).div_euclid(16),
            (center.1 + SAMPLE_RADIUS).div_euclid(16),
        );
        let needed: BTreeSet<_> = (min.1..=max.1)
            .flat_map(|cz| (min.0..=max.0).map(move |cx| (cx, cz)))
            .collect();
        self.ensure_tiles(&needed);
        let queries: Vec<ClientSurfaceQuery> = needed
            .iter()
            .map(|&(cx, cz)| ClientSurfaceQuery {
                coord: [cx, cz],
                revision: self.tiles.get(&(cx, cz)).map_or(0, |tile| tile.watermark),
            })
            .collect();
        let replies = client_surface_columns(queries);

        let mut any_changed = false;
        let mut dirty_rects: Vec<[i32; 4]> = Vec::new();
        for (&(cx, cz), reply) in needed.iter().zip(replies) {
            let Some(cached) = self.tiles.get_mut(&(cx, cz)) else {
                continue;
            };
            let Some(column) = reply else {
                // Column not loaded in the replica: keep prior explored data
                // and keep polling.
                cached.watermark = 0;
                continue;
            };
            let Some(bytes) = column.cells else {
                continue; // unchanged since the watermark
            };
            if bytes.len() != CLIENT_SURFACE_COLUMN_BYTES {
                cached.watermark = 0;
                continue;
            }
            let mut complete = true;
            let mut changed: Option<[i32; 4]> = None;
            for (i, raw) in bytes.chunks_exact(CLIENT_SURFACE_CELL_BYTES).enumerate() {
                let height = i16::from_le_bytes([raw[0], raw[1]]);
                if height == CLIENT_SURFACE_UNKNOWN_HEIGHT {
                    // Unknown never erases what an earlier session explored.
                    complete = false;
                    continue;
                }
                let cell = Cell {
                    height,
                    rgb: [raw[2], raw[3], raw[4]],
                };
                if cached.tile.cells[i] != cell {
                    cached.tile.cells[i] = cell;
                    let (lx, lz) = ((i % 16) as i32, (i / 16) as i32);
                    changed = Some(match changed {
                        None => [lx, lz, lx, lz],
                        Some(r) => [r[0].min(lx), r[1].min(lz), r[2].max(lx), r[3].max(lz)],
                    });
                }
            }
            // Only a fully known reply may arm the skip: a column still
            // streaming in keeps getting polled until every cell is final.
            cached.watermark = if complete { column.revision } else { 0 };
            if let Some([lx0, lz0, lx1, lz1]) = changed {
                cached.dirty = true;
                any_changed = true;
                // +1 for the changed cells themselves being exclusive, +1
                // more for the southeast relief dependency.
                dirty_rects.push([
                    cx * 16 + lx0,
                    cz * 16 + lz0,
                    cx * 16 + lx1 + 2,
                    cz * 16 + lz1 + 2,
                ]);
            }
        }
        if any_changed {
            self.explored_revision = self.explored_revision.wrapping_add(1);
        }
        for rect in dirty_rects {
            self.mark_full_tiles_dirty(rect);
        }
    }

    /// Write every changed tile as one storage batch. Runs on a frame cadence
    /// (and before LRU eviction), not per sample — see [`FLUSH_INTERVAL`].
    pub(crate) fn flush_dirty_tiles(&mut self) {
        let entries: Vec<_> = self
            .tiles
            .iter_mut()
            .filter(|(_, cached)| cached.dirty)
            .map(|(&(cx, cz), cached)| {
                cached.dirty = false;
                (format!("{TILE_PREFIX}{cx}:{cz}"), encode_tile(&cached.tile))
            })
            .collect();
        if !entries.is_empty() {
            client_storage_set_many(entries);
        }
    }

    pub(crate) fn ensure_tiles(&mut self, needed: &BTreeSet<(i32, i32)>) {
        let missing: Vec<_> = needed
            .iter()
            .copied()
            .filter(|coord| !self.tiles.contains_key(coord))
            .collect();
        if !missing.is_empty() {
            let keys: Vec<_> = missing
                .iter()
                .map(|(cx, cz)| format!("{TILE_PREFIX}{cx}:{cz}"))
                .collect();
            for (coord, value) in missing
                .into_iter()
                .zip(client_storage_get_many(keys).into_iter())
            {
                let tile = value.as_deref().and_then(decode_tile).unwrap_or_default();
                self.tiles.insert(
                    coord,
                    CachedTile {
                        tile,
                        last_used: self.frame,
                        watermark: 0,
                        dirty: false,
                    },
                );
            }
        }
        for coord in needed {
            if let Some(cached) = self.tiles.get_mut(coord) {
                cached.last_used = self.frame;
            }
        }
        if self.tiles.len() > TILE_CACHE_MAX + TILE_CACHE_SLACK {
            let mut candidates: Vec<_> = self
                .tiles
                .iter()
                .filter(|(coord, _)| !needed.contains(coord))
                .map(|(&coord, cached)| (cached.last_used, coord))
                .collect();
            candidates.sort_unstable();
            let surplus = (self.tiles.len() - TILE_CACHE_MAX).min(candidates.len());
            let mut unsaved = Vec::new();
            for &(_, coord) in &candidates[..surplus] {
                if let Some(cached) = self.tiles.remove(&coord) {
                    if cached.dirty {
                        unsaved.push((
                            format!("{TILE_PREFIX}{}:{}", coord.0, coord.1),
                            encode_tile(&cached.tile),
                        ));
                    }
                }
            }
            if !unsaved.is_empty() {
                client_storage_set_many(unsaved);
            }
        }
    }
}

/// Sequential cell reads with a two-tile memo: raster loops walk world space
/// coherently, so nearly every read hits the memo instead of the tile map.
pub(crate) struct CellReader<'a> {
    tiles: &'a HashMap<(i32, i32), CachedTile>,
    slots: [((i32, i32), Option<&'a Tile>); 2],
    next: usize,
}

impl<'a> CellReader<'a> {
    pub(crate) fn new(tiles: &'a HashMap<(i32, i32), CachedTile>) -> Self {
        Self {
            tiles,
            slots: [((i32::MAX, i32::MAX), None); 2],
            next: 0,
        }
    }

    fn tile(&mut self, coord: (i32, i32)) -> Option<&'a Tile> {
        for slot in &self.slots {
            if slot.0 == coord {
                return slot.1;
            }
        }
        let tile = self.tiles.get(&coord).map(|cached| &cached.tile);
        self.slots[self.next] = (coord, tile);
        self.next = 1 - self.next;
        tile
    }

    pub(crate) fn cell(&mut self, wx: i32, wz: i32) -> Option<Cell> {
        let tile = self.tile((wx.div_euclid(16), wz.div_euclid(16)))?;
        let cell = tile.cells[(wz.rem_euclid(16) * 16 + wx.rem_euclid(16)) as usize];
        (cell.height != UNKNOWN_HEIGHT).then_some(cell)
    }

    pub(crate) fn terrain_rgb(&mut self, wx: i32, wz: i32) -> [u8; 3] {
        let Some(cell) = self.cell(wx, wz) else {
            return [0, 0, 0];
        };
        let neighbour = self
            .cell(wx - 1, wz - 1)
            .map(|c| c.height)
            .unwrap_or(cell.height);
        shade_rgb(cell.rgb, cell.height, neighbour)
    }
}

/// The shared relief rule: brighten terrain rising toward the northwest.
pub(crate) fn shade_rgb(rgb: [u8; 3], height: i16, northwest: i16) -> [u8; 3] {
    let relief = (height as f32 - northwest as f32) * 0.035;
    let shade = (1.0 + relief).clamp(0.82, 1.16);
    rgb.map(|channel| (channel as f32 * shade).round().clamp(0.0, 255.0) as u8)
}

fn encode_tile(tile: &Tile) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 * 5);
    for cell in tile.cells {
        out.extend(cell.height.to_le_bytes());
        out.extend(cell.rgb);
    }
    out
}

fn decode_tile(bytes: &[u8]) -> Option<Tile> {
    if bytes.len() != 256 * 5 {
        return None;
    }
    let mut tile = Tile::default();
    for (cell, raw) in tile.cells.iter_mut().zip(bytes.chunks_exact(5)) {
        *cell = Cell {
            height: i16::from_le_bytes([raw[0], raw[1]]),
            rgb: [raw[2], raw[3], raw[4]],
        };
    }
    Some(tile)
}
