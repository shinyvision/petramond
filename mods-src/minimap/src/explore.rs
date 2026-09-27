//! Exploration cache. Revision-gated host surface sampling, a 16×16 explored-tile
//! store packed into 4×4-tile region storage values, a write-time mip store the
//! outermost zoom renders from, async ticket-based loading, relief shading.
//!
//! Storage layout (value format is in `codec.rs`):
//! - base region `minimap:r:{rx}:{rz}`: 4×4 tiles = 64×64 blocks, one cell per
//!   block.
//! - mip region `minimap:m:{mx}:{mz}`: 4×4 mip tiles = 128×128 blocks, one cell
//!   per 2×2 blocks. Colors get HSL-averaged at write time, when a dirty base
//!   tile flushes and recomputes its mip cells. Outermost zoom just copies these
//!   cells and touches 4× fewer keys.
//!
//! All bulk reads go through async storage tickets, so a slow disk delays data
//! but never the frame. Residency is region-granular: a region's 16 tiles get
//! inserted and evicted together, so a flush always has the full value in memory
//! and never has to read-modify-write mid-frame.

use crate::*;

pub(crate) const SAMPLE_RADIUS: i32 = 96;
pub(crate) const SAMPLE_STEP: i32 = 8;
const BASE_PREFIX: &str = "minimap:r:";
const MIP_PREFIX: &str = "minimap:m:";
const BASE_REGION_CACHE_MAX: usize = 448;
const MIP_REGION_CACHE_MAX: usize = 448;
const REGION_CACHE_SLACK: usize = 16;
pub(crate) const FLUSH_INTERVAL: u64 = 120;
pub(crate) const UNKNOWN_HEIGHT: i16 = i16::MIN;
const LOAD_KEYS_PER_TICKET: usize = 96;
const LOAD_TICKETS_IN_FLIGHT: usize = 4;
const PREFETCH_TICKETS_IN_FLIGHT: usize = 2;
const DECODE_BUDGET_PER_FRAME: usize = 64;
const PREFETCH_DECODE_BUDGET_PER_FRAME: usize = 16;
const PREFETCH_ISSUE_UNDECODED_MAX: usize = 16;

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

impl Default for Tile {
    fn default() -> Self {
        Self {
            cells: [Cell::default(); 256],
        }
    }
}

#[derive(Clone)]
pub(crate) struct CachedTile {
    pub(crate) tile: Box<Tile>,
    pub(crate) watermark: u64,
    pub(crate) dirty: bool,
}

impl CachedTile {
    pub(crate) fn new(tile: Box<Tile>) -> Self {
        Self {
            tile,
            watermark: 0,
            dirty: false,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RegionKind {
    Base,
    Mip,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd)]
pub(crate) enum LoadTier {
    Sample,
    Visible,
    Prefetch,
}

pub(crate) struct RegionArrival {
    pub(crate) kind: RegionKind,
    pub(crate) coord: (i32, i32),
    pub(crate) had_data: bool,
}

struct QueuedLoad {
    tier: LoadTier,
    /// Sampling needs default tiles even if storage has nothing, since it writes into tiles
    /// directly.
    materialize: bool,
}

struct InFlightLoad {
    ticket: u64,
    urgent: bool,
    entries: Vec<QueuedEntry>,
}

type QueuedEntry = ((i32, i32), RegionKind, bool, LoadTier);

type Undecoded = ((i32, i32), RegionKind, bool, Option<Vec<u8>>);

#[derive(Default)]
pub(crate) struct TileStore {
    pub(crate) ephemeral: bool,
    pub(crate) tiles: HashMap<(i32, i32), CachedTile>,
    pub(crate) mips: HashMap<(i32, i32), CachedTile>,
    base_regions: HashMap<(i32, i32), u64>,
    mip_regions: HashMap<(i32, i32), u64>,
    base_absent: HashSet<(i32, i32)>,
    mip_absent: HashSet<(i32, i32)>,
    queued: HashMap<(RegionKind, (i32, i32)), QueuedLoad>,
    in_flight: Vec<InFlightLoad>,
    pending: HashSet<(RegionKind, (i32, i32))>,
    undecoded_urgent: std::collections::VecDeque<Undecoded>,
    undecoded_prefetch: std::collections::VecDeque<Undecoded>,
    hsl_memo: HashMap<[u8; 3], (f32, f32, f32)>,
    frame: u64,
}

fn region_key(kind: RegionKind, (rx, rz): (i32, i32)) -> String {
    match kind {
        RegionKind::Base => format!("{BASE_PREFIX}{rx}:{rz}"),
        RegionKind::Mip => format!("{MIP_PREFIX}{rx}:{rz}"),
    }
}

pub(crate) fn region_block_rect(kind: RegionKind, (rx, rz): (i32, i32)) -> [i32; 4] {
    let span = match kind {
        RegionKind::Base => codec::REGION_TILES * 16,
        RegionKind::Mip => codec::REGION_TILES * 32,
    };
    [rx * span, rz * span, (rx + 1) * span, (rz + 1) * span]
}

impl TileStore {
    pub(crate) fn ephemeral() -> Self {
        Self {
            ephemeral: true,
            ..Self::default()
        }
    }

    pub(crate) fn begin_frame(&mut self, frame: u64) {
        self.frame = frame;
    }

    fn regions_of(&self, kind: RegionKind) -> &HashMap<(i32, i32), u64> {
        match kind {
            RegionKind::Base => &self.base_regions,
            RegionKind::Mip => &self.mip_regions,
        }
    }

    pub(crate) fn region_resident(&self, kind: RegionKind, coord: (i32, i32)) -> bool {
        self.regions_of(kind).contains_key(&coord)
    }

    pub(crate) fn region_absent(&self, kind: RegionKind, coord: (i32, i32)) -> bool {
        match kind {
            RegionKind::Base => self.base_absent.contains(&coord),
            RegionKind::Mip => self.mip_absent.contains(&coord),
        }
    }

    pub(crate) fn touch_region(&mut self, kind: RegionKind, coord: (i32, i32)) {
        let frame = self.frame;
        match kind {
            RegionKind::Base => self.base_regions.get_mut(&coord).map(|t| *t = frame),
            RegionKind::Mip => self.mip_regions.get_mut(&coord).map(|t| *t = frame),
        };
    }

    pub(crate) fn region_pending(&self, kind: RegionKind, coord: (i32, i32)) -> bool {
        self.pending.contains(&(kind, coord))
    }

    pub(crate) fn request_region(&mut self, kind: RegionKind, coord: (i32, i32), tier: LoadTier) {
        if self.region_resident(kind, coord) {
            self.touch_region(kind, coord);
            return;
        }
        if self.ephemeral {
            match kind {
                RegionKind::Base => self.base_absent.insert(coord),
                RegionKind::Mip => self.mip_absent.insert(coord),
            };
        }
        let materialize = tier == LoadTier::Sample;
        if self.region_absent(kind, coord) {
            if materialize {
                self.materialize_region(kind, coord);
            }
            return;
        }
        if self.pending.contains(&(kind, coord)) && !materialize {
            return;
        }
        match self.queued.entry((kind, coord)) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let queued = entry.get_mut();
                if tier < queued.tier {
                    queued.tier = tier;
                }
                queued.materialize |= materialize;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                if self.pending.contains(&(kind, coord)) {
                    return;
                }
                self.pending.insert((kind, coord));
                entry.insert(QueuedLoad { tier, materialize });
            }
        }
    }

    fn materialize_region(&mut self, kind: RegionKind, coord: (i32, i32)) {
        let frame = self.frame;
        let (regions, tiles) = match kind {
            RegionKind::Base => (&mut self.base_regions, &mut self.tiles),
            RegionKind::Mip => (&mut self.mip_regions, &mut self.mips),
        };
        if regions.insert(coord, frame).is_none() {
            for tz in 0..codec::REGION_TILES {
                for tx in 0..codec::REGION_TILES {
                    tiles.insert(
                        (
                            coord.0 * codec::REGION_TILES + tx,
                            coord.1 * codec::REGION_TILES + tz,
                        ),
                        CachedTile::new(Box::default()),
                    );
                }
            }
        }
    }

    fn install_region(
        &mut self,
        kind: RegionKind,
        coord: (i32, i32),
        decoded: [Option<Box<Tile>>; 16],
    ) {
        let frame = self.frame;
        let (regions, tiles) = match kind {
            RegionKind::Base => (&mut self.base_regions, &mut self.tiles),
            RegionKind::Mip => (&mut self.mip_regions, &mut self.mips),
        };
        if regions.insert(coord, frame).is_some() {
            return;
        }
        for (i, tile) in decoded.into_iter().enumerate() {
            let (tx, tz) = (
                i as i32 % codec::REGION_TILES,
                i as i32 / codec::REGION_TILES,
            );
            tiles.insert(
                (
                    coord.0 * codec::REGION_TILES + tx,
                    coord.1 * codec::REGION_TILES + tz,
                ),
                CachedTile::new(tile.unwrap_or_default()),
            );
        }
    }

    pub(crate) fn pump_loads(&mut self) -> Vec<RegionArrival> {
        let mut still_in_flight = Vec::new();
        for load in std::mem::take(&mut self.in_flight) {
            match client_storage_read_poll(ClientStorageScope::World, load.ticket) {
                None => still_in_flight.push(load),
                Some(values) => {
                    for ((coord, kind, materialize, tier), value) in
                        load.entries.into_iter().zip(values)
                    {
                        let queue = if tier == LoadTier::Prefetch {
                            &mut self.undecoded_prefetch
                        } else {
                            &mut self.undecoded_urgent
                        };
                        queue.push_back((coord, kind, materialize, value));
                    }
                }
            }
        }
        self.in_flight = still_in_flight;

        let mut arrivals = Vec::new();
        let mut budget = DECODE_BUDGET_PER_FRAME;
        while budget > 0 {
            let Some(entry) = self.undecoded_urgent.pop_front() else {
                break;
            };
            budget -= 1;
            self.decode_arrival(entry, &mut arrivals);
        }
        let mut prefetch_budget = budget.min(PREFETCH_DECODE_BUDGET_PER_FRAME);
        while prefetch_budget > 0 {
            let Some(entry) = self.undecoded_prefetch.pop_front() else {
                break;
            };
            prefetch_budget -= 1;
            self.decode_arrival(entry, &mut arrivals);
        }

        loop {
            let prefetch_in_flight = self.in_flight.iter().filter(|load| !load.urgent).count();
            let undecoded = self.undecoded_urgent.len() + self.undecoded_prefetch.len();
            let batch = plan_issue_batch(
                &self.queued,
                self.in_flight.len(),
                prefetch_in_flight,
                undecoded,
            );
            if batch.is_empty() {
                break;
            }
            let urgent = batch.iter().any(|&(.., tier)| tier != LoadTier::Prefetch);
            let keys = batch
                .iter()
                .map(|&(coord, kind, ..)| {
                    self.queued.remove(&(kind, coord));
                    region_key(kind, coord)
                })
                .collect();
            let ticket = client_storage_read_begin(ClientStorageScope::World, keys);
            self.in_flight.push(InFlightLoad {
                ticket,
                urgent,
                entries: batch,
            });
        }
        arrivals
    }

    fn decode_arrival(
        &mut self,
        (coord, kind, materialize, value): Undecoded,
        arrivals: &mut Vec<RegionArrival>,
    ) {
        self.pending.remove(&(kind, coord));
        let decoded = value.as_deref().and_then(codec::decode_region);
        let had_data = decoded.is_some();
        match decoded {
            Some(tiles) => self.install_region(kind, coord, tiles),
            None => {
                match kind {
                    RegionKind::Base => self.base_absent.insert(coord),
                    RegionKind::Mip => self.mip_absent.insert(coord),
                };
                if materialize {
                    self.materialize_region(kind, coord);
                }
            }
        }
        arrivals.push(RegionArrival {
            kind,
            coord,
            had_data,
        });
    }

    pub(crate) fn drop_queued_outside(&mut self, keep: impl Fn(RegionKind, (i32, i32)) -> bool) {
        let dropped: Vec<(RegionKind, (i32, i32))> = self
            .queued
            .iter()
            .filter(|(&(kind, coord), load)| load.tier != LoadTier::Sample && !keep(kind, coord))
            .map(|(&key, _)| key)
            .collect();
        for key in dropped {
            self.queued.remove(&key);
            self.pending.remove(&key);
        }
    }

    /// Redo the 8x8 mip cells this dirty tile covers. Every 2x2 group of blocks gets their mean
    /// height and the HSL average of whichever blocks we know. Only call with the mip tile loaded.
    fn merge_tile_into_mip(&mut self, tile_coord: (i32, i32)) {
        let Some(base) = self.tiles.get(&tile_coord) else {
            return;
        };
        let base_cells = base.tile.cells;
        let mip_coord = (tile_coord.0.div_euclid(2), tile_coord.1.div_euclid(2));
        let quad = (
            tile_coord.0.rem_euclid(2) as usize * 8,
            tile_coord.1.rem_euclid(2) as usize * 8,
        );
        let Some(mip) = self.mips.get_mut(&mip_coord) else {
            return;
        };
        for mz in 0..8usize {
            for mx in 0..8usize {
                let mut colors = [[0u8; 3]; 4];
                let mut known = 0usize;
                let mut height_sum = 0i32;
                for dz in 0..2 {
                    for dx in 0..2 {
                        let cell = base_cells[(mz * 2 + dz) * 16 + mx * 2 + dx];
                        if cell.height != UNKNOWN_HEIGHT {
                            colors[known] = cell.rgb;
                            height_sum += cell.height as i32;
                            known += 1;
                        }
                    }
                }
                let merged = if known == 0 {
                    Cell::default()
                } else {
                    Cell {
                        height: (height_sum / known as i32) as i16,
                        rgb: average_rgb_hsl_memo(&mut self.hsl_memo, &colors[..known]),
                    }
                };
                let at = (quad.1 + mz) * 16 + quad.0 + mx;
                if mip.tile.cells[at] != merged {
                    mip.tile.cells[at] = merged;
                    mip.dirty = true;
                }
            }
        }
    }

    pub(crate) fn flush_dirty(&mut self) {
        let dirty_tiles: Vec<(i32, i32)> = self
            .tiles
            .iter()
            .filter(|(_, cached)| cached.dirty)
            .map(|(&coord, _)| coord)
            .collect();
        if dirty_tiles.is_empty() && self.mips.values().all(|m| !m.dirty) {
            return;
        }
        let mut base_regions: BTreeSet<(i32, i32)> = BTreeSet::new();
        for &(tx, tz) in &dirty_tiles {
            base_regions.insert((
                tx.div_euclid(codec::REGION_TILES),
                tz.div_euclid(codec::REGION_TILES),
            ));
        }

        let mut entries = Vec::new();
        let mut flushed_mip_regions: BTreeSet<(i32, i32)> = BTreeSet::new();
        for region in base_regions {
            let mip_region = (region.0.div_euclid(2), region.1.div_euclid(2));
            if !self.region_resident(RegionKind::Mip, mip_region) {
                self.request_region(RegionKind::Mip, mip_region, LoadTier::Sample);
                continue;
            }
            for tz in 0..codec::REGION_TILES {
                for tx in 0..codec::REGION_TILES {
                    let coord = (
                        region.0 * codec::REGION_TILES + tx,
                        region.1 * codec::REGION_TILES + tz,
                    );
                    if self.tiles.get(&coord).is_some_and(|t| t.dirty) {
                        self.merge_tile_into_mip(coord);
                    }
                }
            }
            if !self.ephemeral {
                entries.push((
                    region_key(RegionKind::Base, region),
                    self.encode_resident_region(RegionKind::Base, region),
                ));
            }
            self.base_absent.remove(&region);
            for tz in 0..codec::REGION_TILES {
                for tx in 0..codec::REGION_TILES {
                    let coord = (
                        region.0 * codec::REGION_TILES + tx,
                        region.1 * codec::REGION_TILES + tz,
                    );
                    if let Some(tile) = self.tiles.get_mut(&coord) {
                        tile.dirty = false;
                    }
                }
            }
            flushed_mip_regions.insert(mip_region);
        }
        for mip_region in flushed_mip_regions {
            if !self.ephemeral {
                entries.push((
                    region_key(RegionKind::Mip, mip_region),
                    self.encode_resident_region(RegionKind::Mip, mip_region),
                ));
            }
            self.mip_absent.remove(&mip_region);
            for tz in 0..codec::REGION_TILES {
                for tx in 0..codec::REGION_TILES {
                    let coord = (
                        mip_region.0 * codec::REGION_TILES + tx,
                        mip_region.1 * codec::REGION_TILES + tz,
                    );
                    if let Some(tile) = self.mips.get_mut(&coord) {
                        tile.dirty = false;
                    }
                }
            }
        }
        if !entries.is_empty() {
            let entries = entries.into_iter().map(|(k, v)| (k, Some(v))).collect();
            if let Err(why) = client_storage_set_many(ClientStorageScope::World, entries) {
                log(&format!("minimap: explored map not saved: {why}"));
            }
        }
    }

    fn encode_resident_region(&self, kind: RegionKind, region: (i32, i32)) -> Vec<u8> {
        let tiles = match kind {
            RegionKind::Base => &self.tiles,
            RegionKind::Mip => &self.mips,
        };
        let members: [Option<&Tile>; 16] = std::array::from_fn(|i| {
            let coord = (
                region.0 * codec::REGION_TILES + i as i32 % codec::REGION_TILES,
                region.1 * codec::REGION_TILES + i as i32 / codec::REGION_TILES,
            );
            tiles
                .get(&coord)
                .map(|cached| cached.tile.as_ref())
                .filter(|tile| tile_has_data(tile))
        });
        codec::encode_region(&members)
    }

    pub(crate) fn trim_caches(&mut self, protect: impl Fn(RegionKind, (i32, i32)) -> bool) {
        for kind in [RegionKind::Base, RegionKind::Mip] {
            let (cap, regions) = match kind {
                RegionKind::Base => (BASE_REGION_CACHE_MAX, &self.base_regions),
                RegionKind::Mip => (MIP_REGION_CACHE_MAX, &self.mip_regions),
            };
            if regions.len() <= cap + REGION_CACHE_SLACK {
                continue;
            }
            let mut candidates: Vec<(u64, (i32, i32))> = regions
                .iter()
                .filter(|(&coord, _)| !protect(kind, coord))
                .map(|(&coord, &last_used)| (last_used, coord))
                .collect();
            candidates.sort_unstable();
            let surplus = (regions.len() - cap).min(candidates.len());
            for &(_, region) in &candidates[..surplus] {
                let members: Vec<(i32, i32)> = (0..16)
                    .map(|i| {
                        (
                            region.0 * codec::REGION_TILES + i % codec::REGION_TILES,
                            region.1 * codec::REGION_TILES + i / codec::REGION_TILES,
                        )
                    })
                    .collect();
                let tiles = match kind {
                    RegionKind::Base => &self.tiles,
                    RegionKind::Mip => &self.mips,
                };
                if members
                    .iter()
                    .any(|coord| tiles.get(coord).is_some_and(|t| t.dirty))
                {
                    continue;
                }
                let (regions, tiles) = match kind {
                    RegionKind::Base => (&mut self.base_regions, &mut self.tiles),
                    RegionKind::Mip => (&mut self.mip_regions, &mut self.mips),
                };
                regions.remove(&region);
                for coord in members {
                    tiles.remove(&coord);
                }
            }
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
        for rz in min.1.div_euclid(codec::REGION_TILES)..=max.1.div_euclid(codec::REGION_TILES) {
            for rx in min.0.div_euclid(codec::REGION_TILES)..=max.0.div_euclid(codec::REGION_TILES)
            {
                self.store
                    .request_region(RegionKind::Base, (rx, rz), LoadTier::Sample);
            }
        }
        let queries: Vec<ClientSurfaceQuery> = (min.1..=max.1)
            .flat_map(|cz| (min.0..=max.0).map(move |cx| (cx, cz)))
            .filter_map(|(cx, cz)| {
                self.store
                    .tiles
                    .get(&(cx, cz))
                    .map(|tile| ClientSurfaceQuery {
                        coord: [cx, cz],
                        revision: tile.watermark,
                    })
            })
            .collect();
        if queries.is_empty() {
            return;
        }
        let replies = client_surface_columns(queries.clone());

        let mut any_changed = false;
        let mut dirty_rects: Vec<[i32; 4]> = Vec::new();
        for (query, reply) in queries.iter().zip(replies) {
            let (cx, cz) = (query.coord[0], query.coord[1]);
            let Some(cached) = self.store.tiles.get_mut(&(cx, cz)) else {
                continue;
            };
            let Some(column) = reply else {
                cached.watermark = 0;
                continue;
            };
            let Some(bytes) = column.cells else {
                continue;
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
            cached.watermark = if complete { column.revision } else { 0 };
            if let Some([lx0, lz0, lx1, lz1]) = changed {
                cached.dirty = true;
                any_changed = true;
                self.store.request_region(
                    RegionKind::Mip,
                    (
                        cx.div_euclid(2 * codec::REGION_TILES),
                        cz.div_euclid(2 * codec::REGION_TILES),
                    ),
                    LoadTier::Sample,
                );
                dirty_rects.push([
                    cx * 16 + lx0,
                    cz * 16 + lz0,
                    cx * 16 + lx1 + 1,
                    cz * 16 + lz1 + 1,
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

    /// The once-per-frame store heartbeat: poll/issue async loads, repaint
    /// whatever arrived, and trim the caches. Trimming runs with the full
    /// map OPEN too — its visible+prefetch rect is protected instead of
    /// deferring eviction wholesale, keeping cache use bounded during a long
    /// browse of a large explored world.
    pub(crate) fn pump_store(&mut self) {
        for arrival in self.store.pump_loads() {
            if arrival.had_data {
                self.mark_full_tiles_dirty(region_block_rect(arrival.kind, arrival.coord));
            }
        }
        let sample_center = self
            .last_sample
            .unwrap_or((self.player[0].floor() as i32, self.player[2].floor() as i32));
        let map_open = self.open_canvas.as_deref() == Some(FULL_CANVAS);
        let view = map_open.then(|| self.full_view_world_rect());
        let adjacent = map_open.then(|| self.adjacent_zoom_world_rect()).flatten();
        let source = fullmap::source_kind(self.zoom);
        self.store.trim_caches(|kind, region| {
            let rect = region_block_rect(kind, region);
            let intersects =
                |v: [i32; 4]| rect[2] > v[0] && rect[0] < v[2] && rect[3] > v[1] && rect[1] < v[3];
            let pad = SAMPLE_RADIUS + 16;
            let sampled = rect[2] > sample_center.0 - pad
                && rect[0] < sample_center.0 + pad
                && rect[3] > sample_center.1 - pad
                && rect[1] < sample_center.1 + pad;
            sampled
                || (kind == source && view.is_some_and(intersects))
                || adjacent.is_some_and(|(k, v)| k == kind && intersects(v))
        });
    }
}

/// Picks the next ticket's loads, or nothing. Sample and visible loads take any open slot and skip
/// backpressure, since the neighborhood and viewport already cap how many there are. Prefetch
/// waits until nothing urgent is queued and the arrival queue is shallow, and only uses its own
/// slots, so warming up never holds the viewport back.
fn plan_issue_batch(
    queued: &HashMap<(RegionKind, (i32, i32)), QueuedLoad>,
    in_flight: usize,
    prefetch_in_flight: usize,
    undecoded: usize,
) -> Vec<QueuedEntry> {
    if in_flight >= LOAD_TICKETS_IN_FLIGHT {
        return Vec::new();
    }
    let pick = |tiers: &[LoadTier], room: usize| -> Vec<QueuedEntry> {
        let mut picked: Vec<QueuedEntry> = queued
            .iter()
            .filter(|(_, load)| tiers.contains(&load.tier))
            .map(|(&(kind, coord), load)| (coord, kind, load.materialize, load.tier))
            .collect();
        picked.sort_unstable_by_key(|&(coord, kind, _, tier)| {
            (
                tier == LoadTier::Visible,
                kind == RegionKind::Mip,
                coord.1,
                coord.0,
            )
        });
        picked.truncate(room);
        picked
    };
    let urgent = pick(&[LoadTier::Sample, LoadTier::Visible], LOAD_KEYS_PER_TICKET);
    if !urgent.is_empty() {
        return urgent;
    }
    if prefetch_in_flight >= PREFETCH_TICKETS_IN_FLIGHT || undecoded > PREFETCH_ISSUE_UNDECODED_MAX
    {
        return Vec::new();
    }
    pick(&[LoadTier::Prefetch], LOAD_KEYS_PER_TICKET)
}

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
        let tile = self.tiles.get(&coord).map(|cached| &*cached.tile);
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

pub(crate) fn tile_has_data(tile: &Tile) -> bool {
    tile.cells.iter().any(|cell| cell.height != UNKNOWN_HEIGHT)
}

pub(crate) fn shade_rgb(rgb: [u8; 3], height: i16, northwest: i16) -> [u8; 3] {
    const SHADE_LUT: [u16; 13] = {
        let mut lut = [0u16; 13];
        let mut i = 0;
        while i < 13 {
            let shade = 1.0 + (i as f64 - 6.0) * 0.035;
            let shade = if shade < 0.82 {
                0.82
            } else if shade > 1.16 {
                1.16
            } else {
                shade
            };
            lut[i] = (shade * 256.0 + 0.5) as u16;
            i += 1;
        }
        lut
    };
    let delta = (height as i32 - northwest as i32).clamp(-6, 6);
    let multiplier = SHADE_LUT[(delta + 6) as usize] as u32;
    rgb.map(|channel| (((channel as u32 * multiplier) + 128) >> 8).min(255) as u8)
}

#[cfg(test)]
pub(crate) fn shade_rgb_reference(rgb: [u8; 3], height: i16, northwest: i16) -> [u8; 3] {
    let relief = (height as f32 - northwest as f32) * 0.035;
    let shade = (1.0 + relief).clamp(0.82, 1.16);
    rgb.map(|channel| (channel as f32 * shade).round().clamp(0.0, 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_region(kind: RegionKind, region: (i32, i32)) -> TileStore {
        let mut store = TileStore::default();
        store.materialize_region(kind, region);
        store
    }

    fn storage_calls() -> (
        mod_sdk::testing::HostGuard,
        std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    ) {
        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = std::rc::Rc::clone(&calls);
        let guard = mod_sdk::testing::install_host(move |call| {
            let (kind, ret) = match call {
                HostCall::Client(ClientCall::ClientStorageSetMany { .. }) => {
                    ("write", HostRet::ClientStorageWrite(1))
                }
                HostCall::Client(ClientCall::ClientStorageReadBegin { .. }) => {
                    ("read", HostRet::U64(1))
                }
                HostCall::Client(ClientCall::ClientStorageReadPoll { .. }) => {
                    ("read", HostRet::ClientStorageRead(None))
                }
                other => panic!("unexpected host call {other:?}"),
            };
            seen.borrow_mut().push(kind);
            ret
        });
        (guard, calls)
    }

    #[test]
    fn an_ephemeral_store_explores_without_touching_storage() {
        let (_host, calls) = storage_calls();
        let mut store = TileStore::ephemeral();
        store.request_region(RegionKind::Base, (0, 0), LoadTier::Sample);
        assert!(store.region_resident(RegionKind::Base, (0, 0)));
        store.tiles.get_mut(&(0, 0)).unwrap().tile.cells[0] = Cell {
            height: 64,
            rgb: [10, 200, 10],
        };
        store.tiles.get_mut(&(0, 0)).unwrap().dirty = true;
        for _ in 0..2 {
            let _ = store.pump_loads();
            store.flush_dirty();
        }
        assert!(!store.tiles[&(0, 0)].dirty, "flushed, so it can trim");
        assert!(store.mips[&(0, 0)].tile.cells[0].height != UNKNOWN_HEIGHT);
        assert!(!calls.borrow().contains(&"write"));
        assert!(!calls.borrow().contains(&"read"));

        let mut own = store_with_region(RegionKind::Base, (0, 0));
        own.materialize_region(RegionKind::Mip, (0, 0));
        own.tiles.get_mut(&(0, 0)).unwrap().tile.cells[0].height = 64;
        own.tiles.get_mut(&(0, 0)).unwrap().dirty = true;
        own.flush_dirty();
        assert!(calls.borrow().contains(&"write"));
    }

    #[test]
    fn a_region_is_resident_wholesale_and_evicts_wholesale() {
        let mut store = store_with_region(RegionKind::Base, (2, -1));
        assert!(store.region_resident(RegionKind::Base, (2, -1)));
        for tz in -4..0 {
            for tx in 8..12 {
                assert!(store.tiles.contains_key(&(tx, tz)), "member ({tx},{tz})");
            }
        }
        store.trim_caches(|_, _| false);
        assert!(store.region_resident(RegionKind::Base, (2, -1)));
        for i in 0..(BASE_REGION_CACHE_MAX + REGION_CACHE_SLACK) as i32 + 4 {
            store.materialize_region(RegionKind::Base, (100 + i, 0));
        }
        store.tiles.get_mut(&(8, -4)).unwrap().dirty = true;
        store.trim_caches(|_, region| region == (100, 0));
        assert!(
            store.region_resident(RegionKind::Base, (100, 0)),
            "protected region stays"
        );
        assert!(
            store.region_resident(RegionKind::Base, (2, -1)),
            "dirty-member region stays"
        );
        assert!(store.base_regions.len() <= BASE_REGION_CACHE_MAX + 2);
        for &(rx, rz) in store.base_regions.keys() {
            for i in 0..16 {
                let member = (
                    rx * codec::REGION_TILES + i % codec::REGION_TILES,
                    rz * codec::REGION_TILES + i / codec::REGION_TILES,
                );
                assert!(store.tiles.contains_key(&member));
            }
        }
    }

    #[test]
    fn an_open_full_map_trims_out_of_view_regions() {
        let mut mm = crate::Minimap {
            open_canvas: Some(FULL_CANVAS.to_string()),
            ..Default::default()
        };
        mm.store.materialize_region(RegionKind::Base, (0, 0));
        for i in 0..(BASE_REGION_CACHE_MAX + REGION_CACHE_SLACK) as i32 + 8 {
            mm.store
                .materialize_region(RegionKind::Base, (1000 + i, 500));
        }
        mm.pump_store();
        assert!(
            mm.store.region_resident(RegionKind::Base, (0, 0)),
            "the viewport's region survives the trim"
        );
        assert!(
            mm.store.base_regions.len() <= BASE_REGION_CACHE_MAX + 4,
            "far regions trim while the map is open, got {}",
            mm.store.base_regions.len()
        );
    }

    /// Loader must never let a prefetch backlog delay urgent loads. Urgent keys
    /// fill the next ticket by themselves, prefetch only goes into its reserved
    /// slots when idle, and arrival backpressure hits prefetch only. If this
    /// regresses you get the seconds-long blank screen when dragging zoomed out.
    #[test]
    fn urgent_loads_issue_ahead_of_a_prefetch_backlog() {
        let mut queued: HashMap<(RegionKind, (i32, i32)), QueuedLoad> = HashMap::new();
        for i in 0..300 {
            queued.insert(
                (RegionKind::Base, (i, 0)),
                QueuedLoad {
                    tier: LoadTier::Prefetch,
                    materialize: false,
                },
            );
        }
        for i in 0..3 {
            queued.insert(
                (RegionKind::Mip, (i, 5)),
                QueuedLoad {
                    tier: LoadTier::Visible,
                    materialize: false,
                },
            );
        }
        let batch = plan_issue_batch(&queued, 0, 0, 500);
        assert_eq!(
            batch.len(),
            3,
            "urgent keys issue alone, backpressure-exempt"
        );
        assert!(batch.iter().all(|&(.., tier)| tier == LoadTier::Visible));

        for &(coord, kind, ..) in &batch {
            queued.remove(&(kind, coord));
        }
        assert!(
            plan_issue_batch(&queued, 1, 0, 500).is_empty(),
            "prefetch never issues into a deep arrival queue"
        );
        let prefetch = plan_issue_batch(&queued, 1, 0, 0);
        assert_eq!(prefetch.len(), LOAD_KEYS_PER_TICKET);
        assert!(
            plan_issue_batch(&queued, 2, PREFETCH_TICKETS_IN_FLIGHT, 0).is_empty(),
            "prefetch stays within its reserved ticket slots"
        );
        assert!(
            plan_issue_batch(&queued, LOAD_TICKETS_IN_FLIGHT, 0, 0).is_empty(),
            "no slots, no ticket"
        );

        queued.insert(
            (RegionKind::Base, (999, 9)),
            QueuedLoad {
                tier: LoadTier::Sample,
                materialize: true,
            },
        );
        let batch = plan_issue_batch(&queued, 0, 0, 0);
        assert_eq!(
            (batch[0].1, batch[0].0),
            (RegionKind::Base, (999, 9)),
            "sampling sorts ahead of everything"
        );
    }

    #[test]
    fn a_pan_restamp_drops_stale_queued_loads() {
        let mut store = TileStore::default();
        store.request_region(RegionKind::Mip, (0, 0), LoadTier::Visible);
        store.request_region(RegionKind::Mip, (50, 0), LoadTier::Visible);
        store.request_region(RegionKind::Base, (7, 7), LoadTier::Sample);
        store.drop_queued_outside(|_, coord| coord == (0, 0));
        assert!(store.region_pending(RegionKind::Mip, (0, 0)), "kept");
        assert!(
            !store.region_pending(RegionKind::Mip, (50, 0)),
            "stale visible load dropped from queue and pending"
        );
        assert!(
            store.region_pending(RegionKind::Base, (7, 7)),
            "sampling loads never drop"
        );
    }

    #[test]
    fn dirty_tiles_merge_into_their_mip_at_flush_granularity() {
        let mut store = store_with_region(RegionKind::Base, (0, 0));
        store.materialize_region(RegionKind::Mip, (0, 0));
        let tile = store.tiles.get_mut(&(1, 1)).unwrap();
        tile.tile.cells[0] = Cell {
            height: 10,
            rgb: [100, 0, 0],
        };
        tile.tile.cells[1] = Cell {
            height: 20,
            rgb: [100, 0, 0],
        };
        store.merge_tile_into_mip((1, 1));
        let mip = store.mips.get(&(0, 0)).expect("mip tile resident");
        let merged = mip.tile.cells[8 * 16 + 8];
        assert_eq!(merged.height, 15, "mean of the known heights");
        assert_eq!(merged.rgb, [100, 0, 0], "average of identical colors");
        assert_eq!(
            mip.tile.cells[8 * 16 + 9],
            Cell::default(),
            "fully unknown groups stay unknown"
        );
        assert!(mip.dirty);
    }

    #[test]
    fn shade_lut_matches_the_float_reference_within_rounding() {
        for delta in -12i16..=12 {
            for channel in [0u8, 1, 17, 100, 200, 255] {
                let fast = shade_rgb([channel; 3], 40 + delta, 40);
                let reference = shade_rgb_reference([channel; 3], 40 + delta, 40);
                for i in 0..3 {
                    assert!(
                        fast[i].abs_diff(reference[i]) <= 1,
                        "delta {delta}, channel {channel}: {fast:?} vs {reference:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn region_block_rects_cover_their_kind_span() {
        assert_eq!(region_block_rect(RegionKind::Base, (0, 0)), [0, 0, 64, 64]);
        assert_eq!(
            region_block_rect(RegionKind::Base, (-1, 2)),
            [-64, 128, 0, 192]
        );
        assert_eq!(
            region_block_rect(RegionKind::Mip, (1, -1)),
            [128, -128, 256, 0]
        );
    }
}
