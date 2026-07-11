//! Exploration cache: host surface sampling, the 16×16 explored-tile
//! store with its LRU working set, persistence codec, and relief shading.

use crate::*;

pub(crate) const SAMPLE_RADIUS: i32 = 96;
pub(crate) const SAMPLE_STEP: i32 = 8;
const TILE_PREFIX: &str = "minimap:tile:";
const TILE_CACHE_MAX: usize = 2048;
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
        let samples = client_surface([center.0, center.1], SAMPLE_RADIUS as u16);
        let side = SAMPLE_RADIUS * 2 + 1;
        let mut changed = BTreeSet::new();
        let mut changed_full_tiles = BTreeSet::new();
        for dz in -SAMPLE_RADIUS..=SAMPLE_RADIUS {
            for dx in -SAMPLE_RADIUS..=SAMPLE_RADIUS {
                let i = ((dz + SAMPLE_RADIUS) * side + dx + SAMPLE_RADIUS) as usize;
                let Some(sample) = samples.get(i).copied().flatten() else {
                    continue;
                };
                let wx = center.0 + dx;
                let wz = center.1 + dz;
                let tc = (wx.div_euclid(16), wz.div_euclid(16));
                let cell = Cell {
                    height: sample.height,
                    rgb: sample.rgb,
                };
                let tile = &mut self
                    .tiles
                    .entry(tc)
                    .or_insert_with(|| CachedTile {
                        tile: Tile::default(),
                        last_used: self.frame,
                    })
                    .tile;
                let index = (wz.rem_euclid(16) * 16 + wx.rem_euclid(16)) as usize;
                if tile.cells[index] != cell {
                    tile.cells[index] = cell;
                    changed.insert(tc);
                    changed_full_tiles.insert(full_tile_coord(wx, wz));
                    changed_full_tiles.insert(full_tile_coord(wx + 1, wz + 1));
                }
            }
        }
        if !changed.is_empty() {
            let entries = changed
                .iter()
                .filter_map(|&(cx, cz)| {
                    self.tiles.get(&(cx, cz)).map(|cached| {
                        (format!("{TILE_PREFIX}{cx}:{cz}"), encode_tile(&cached.tile))
                    })
                })
                .collect();
            client_storage_set_many(entries);
            for coord in changed_full_tiles {
                self.invalidate_full_tile(coord);
            }
        }
    }

    fn cell(&self, wx: i32, wz: i32) -> Option<Cell> {
        let cached = self.tiles.get(&(wx.div_euclid(16), wz.div_euclid(16)))?;
        let cell = cached.tile.cells[(wz.rem_euclid(16) * 16 + wx.rem_euclid(16)) as usize];
        (cell.height != UNKNOWN_HEIGHT).then_some(cell)
    }

    pub(crate) fn terrain_rgb(&self, wx: i32, wz: i32) -> [u8; 3] {
        let Some(cell) = self.cell(wx, wz) else {
            return [0, 0, 0];
        };
        let neighbour = self
            .cell(wx - 1, wz - 1)
            .map(|c| c.height)
            .unwrap_or(cell.height);
        let relief = (cell.height as f32 - neighbour as f32) * 0.035;
        let shade = (1.0 + relief).clamp(0.82, 1.16);
        cell.rgb
            .map(|channel| (channel as f32 * shade).round().clamp(0.0, 255.0) as u8)
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
                    },
                );
            }
        }
        for coord in needed {
            if let Some(cached) = self.tiles.get_mut(coord) {
                cached.last_used = self.frame;
            }
        }
        while self.tiles.len() > TILE_CACHE_MAX {
            let Some(coord) = self
                .tiles
                .iter()
                .filter(|(coord, _)| !needed.contains(coord))
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(coord, _)| *coord)
            else {
                break;
            };
            self.tiles.remove(&coord);
        }
    }
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
