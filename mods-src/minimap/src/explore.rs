//! Exploration cache: revision-gated host surface sampling, the 16×16
//! explored-tile store bundled into 4×4-tile REGION storage values, the
//! write-time mip store the outermost zoom renders from, asynchronous
//! ticket-based loading, and relief shading.
//!
//! Storage layout (see `codec.rs` for the value format):
//! - base region `minimap:r:{rx}:{rz}`: 4×4 tiles = 64×64 blocks, one cell
//!   per block;
//! - mip region `minimap:m:{mx}:{mz}`: 4×4 MIP tiles = 128×128 blocks, one
//!   cell per 2×2 blocks, colors HSL-averaged at WRITE time (the flush that
//!   persists a dirty base tile recomputes its mip cells), so the outermost
//!   zoom renders with plain copies and 4× fewer keys.
//!
//! All bulk reads go through async storage tickets: a slow disk delays data,
//! never the frame. Residency is REGION-granular — a region's 16 member
//! tiles are inserted and evicted together, so a flush always has the whole
//! value in memory and never read-modifies-writes on the frame.

use crate::*;

pub(crate) const SAMPLE_RADIUS: i32 = 96;
pub(crate) const SAMPLE_STEP: i32 = 8;
const BASE_PREFIX: &str = "minimap:r:";
const MIP_PREFIX: &str = "minimap:m:";
/// Resident-region caps (a base region ≈ 25 KB of cells). Eviction defers
/// while the full map is open; see `trim_caches`.
const BASE_REGION_CACHE_MAX: usize = 448;
const MIP_REGION_CACHE_MAX: usize = 448;
const REGION_CACHE_SLACK: usize = 16;
/// Frames between persistence flushes of changed tiles (~2 s at 60 fps): a
/// frontier tile fills over many samples but is written once per interval.
pub(crate) const FLUSH_INTERVAL: u64 = 120;
pub(crate) const UNKNOWN_HEIGHT: i16 = i16::MIN;
/// Async read batching: keys per ticket and tickets kept in flight (the host
/// caps outstanding tickets at 8; leave headroom for same-frame edits).
const LOAD_KEYS_PER_TICKET: usize = 96;
const LOAD_TICKETS_IN_FLIGHT: usize = 4;
/// Region values DECODED per frame: arrivals beyond this wait in a queue, so
/// a burst of completed tickets can never spend a frame's budget unpacking
/// cells.
const DECODE_BUDGET_PER_FRAME: usize = 12;

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
    /// Boxed: the cache holds thousands of entries; an inline 1.5 KB payload
    /// would put the hash table's buckets (plus its rehash doubling spike)
    /// at tens of MB.
    pub(crate) tile: Box<Tile>,
    /// Host column revision this tile was last seen COMPLETE at (0 = never):
    /// echoed back so an unchanged column costs no cell bytes. Session-local
    /// replica state — never persisted with the tile. (Base tiles only.)
    pub(crate) watermark: u64,
    /// Changed since the last persistence flush.
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

/// Which store a region load belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RegionKind {
    Base,
    Mip,
}

/// Load priority: sampling feeds live exploration, visible feeds the open
/// map, prefetch warms what panning/zooming will need next.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd)]
pub(crate) enum LoadTier {
    Sample,
    Visible,
    Prefetch,
}

/// One completed region load the caller may need to repaint.
pub(crate) struct RegionArrival {
    pub(crate) kind: RegionKind,
    pub(crate) coord: (i32, i32),
    pub(crate) had_data: bool,
}

struct QueuedLoad {
    tier: LoadTier,
    /// Sampling needs mutation targets, so its loads materialize default
    /// tiles even when storage has nothing.
    materialize: bool,
}

struct InFlightLoad {
    ticket: u64,
    entries: Vec<((i32, i32), RegionKind, bool)>,
}

/// The tile/mip caches plus the async region loader.
#[derive(Default)]
pub(crate) struct TileStore {
    /// Base tiles by 16-block tile coord (one cell per block).
    pub(crate) tiles: HashMap<(i32, i32), CachedTile>,
    /// Mip tiles by MIP-tile coord (16×16 cells of 2×2 blocks = 32×32
    /// blocks per tile).
    pub(crate) mips: HashMap<(i32, i32), CachedTile>,
    /// Resident regions (each implies all 16 member tiles resident) with
    /// their LRU stamps.
    base_regions: HashMap<(i32, i32), u64>,
    mip_regions: HashMap<(i32, i32), u64>,
    /// Regions storage had nothing for — never re-requested until written.
    base_absent: HashSet<(i32, i32)>,
    mip_absent: HashSet<(i32, i32)>,
    queued: HashMap<(RegionKind, (i32, i32)), QueuedLoad>,
    in_flight: Vec<InFlightLoad>,
    /// Every coord currently queued, in flight, or awaiting decode — the
    /// paint-once readiness check.
    pending: HashSet<(RegionKind, (i32, i32))>,
    /// Arrived values awaiting their decode-budget slot.
    undecoded: std::collections::VecDeque<((i32, i32), RegionKind, bool, Option<Vec<u8>>)>,
    /// rgb→hsl work on the mip write path is memoized: terrain colors repeat
    /// massively.
    hsl_memo: HashMap<[u8; 3], (f32, f32, f32)>,
    frame: u64,
}

fn region_key(kind: RegionKind, (rx, rz): (i32, i32)) -> String {
    match kind {
        RegionKind::Base => format!("{BASE_PREFIX}{rx}:{rz}"),
        RegionKind::Mip => format!("{MIP_PREFIX}{rx}:{rz}"),
    }
}

/// World-block rect `[x0, z0, x1, z1)` one region covers.
pub(crate) fn region_block_rect(kind: RegionKind, (rx, rz): (i32, i32)) -> [i32; 4] {
    let span = match kind {
        RegionKind::Base => codec::REGION_TILES * 16,
        RegionKind::Mip => codec::REGION_TILES * 32,
    };
    [rx * span, rz * span, (rx + 1) * span, (rz + 1) * span]
}

impl TileStore {
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

    /// Whether the region's data is not yet available (queued, in flight, or
    /// awaiting decode) — a raster over it would paint holes it will have to
    /// repaint. Resident and absent regions are both READY.
    pub(crate) fn region_pending(&self, kind: RegionKind, coord: (i32, i32)) -> bool {
        self.pending.contains(&(kind, coord))
    }

    /// Queue one region load (idempotent; a stronger tier upgrades a weaker
    /// queued entry). Absent-memo'd regions never re-request: sampling
    /// materializes them, everything else skips.
    pub(crate) fn request_region(&mut self, kind: RegionKind, coord: (i32, i32), tier: LoadTier) {
        if self.region_resident(kind, coord) {
            self.touch_region(kind, coord);
            return;
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
                    // Already in flight or awaiting decode; the sampling
                    // upgrade only matters for materialize-on-absent, which
                    // request_region re-runs after the result lands.
                    return;
                }
                self.pending.insert((kind, coord));
                entry.insert(QueuedLoad { tier, materialize });
            }
        }
    }

    /// Insert a region as resident with default (never-explored) tiles — the
    /// mutation target for fresh exploration of storage-absent ground.
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
            return; // raced by a materialize: live samples win over storage
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

    /// Poll in-flight tickets and issue new ones from the queue: the once-
    /// per-frame loader heartbeat. Returns completed loads so the map can
    /// repaint what arrived. Decoding is budgeted — a burst of completed
    /// tickets unpacks over several frames instead of spiking one.
    pub(crate) fn pump_loads(&mut self) -> Vec<RegionArrival> {
        let mut still_in_flight = Vec::new();
        for load in std::mem::take(&mut self.in_flight) {
            match client_storage_read_poll(load.ticket) {
                None => still_in_flight.push(load),
                Some(values) => {
                    for ((coord, kind, materialize), value) in
                        load.entries.into_iter().zip(values)
                    {
                        self.undecoded.push_back((coord, kind, materialize, value));
                    }
                }
            }
        }
        self.in_flight = still_in_flight;

        let mut arrivals = Vec::new();
        for _ in 0..DECODE_BUDGET_PER_FRAME {
            let Some((coord, kind, materialize, value)) = self.undecoded.pop_front() else {
                break;
            };
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

        // Backpressure: don't stack more completed values behind the decode
        // budget than a couple of frames can drain.
        while self.in_flight.len() < LOAD_TICKETS_IN_FLIGHT
            && !self.queued.is_empty()
            && self.undecoded.len() < DECODE_BUDGET_PER_FRAME * 2
        {
            let mut batch: Vec<_> = Vec::new();
            for tier in [LoadTier::Sample, LoadTier::Visible, LoadTier::Prefetch] {
                if batch.len() >= LOAD_KEYS_PER_TICKET {
                    break;
                }
                let mut picked: Vec<_> = self
                    .queued
                    .iter()
                    .filter(|(_, load)| load.tier == tier)
                    .map(|(&(kind, coord), load)| (coord, kind, load.materialize))
                    .collect();
                picked.sort_unstable_by_key(|&(coord, kind, _)| (kind == RegionKind::Mip, coord.1, coord.0));
                picked.truncate(LOAD_KEYS_PER_TICKET - batch.len());
                for entry in picked {
                    self.queued.remove(&(entry.1, entry.0));
                    batch.push(entry);
                }
            }
            if batch.is_empty() {
                break;
            }
            let keys = batch
                .iter()
                .map(|&(coord, kind, _)| region_key(kind, coord))
                .collect();
            let ticket = client_storage_read_begin(keys);
            self.in_flight.push(InFlightLoad {
                ticket,
                entries: batch,
            });
        }
        arrivals
    }

    /// Recompute the 8×8 mip cells one dirty base tile covers, from its own
    /// 16×16 block cells: color = HSL average of the known blocks in each
    /// 2×2 group, height = their mean. The owning mip tile must be resident.
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

    /// Write every dirty tile's region (and its recomputed mip region) as one
    /// storage batch. A base region whose mip region is not yet resident
    /// keeps its dirty flags and retries next interval — mip values never go
    /// stale relative to committed base values.
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
                // Queued by the sampling path when the tile went dirty;
                // retry once it lands.
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
            entries.push((
                region_key(RegionKind::Base, region),
                self.encode_resident_region(RegionKind::Base, region),
            ));
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
            entries.push((
                region_key(RegionKind::Mip, mip_region),
                self.encode_resident_region(RegionKind::Mip, mip_region),
            ));
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
            client_storage_set_many(entries);
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

    /// Region-granular LRU trim, deferred while the full map is open (its
    /// visible working set legitimately exceeds the caps when zoomed out).
    /// Regions with unflushed members are skipped; they trim after their
    /// next flush.
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
        // Request the covering regions; sample only tiles already resident
        // (a region still loading answers next frame — the watermark gate
        // makes retrying free).
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
                self.store.tiles.get(&(cx, cz)).map(|tile| ClientSurfaceQuery {
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
                // Flush will merge this tile into its mip region: have that
                // region on the way before the flush interval fires.
                self.store.request_region(
                    RegionKind::Mip,
                    (
                        cx.div_euclid(2 * codec::REGION_TILES),
                        cz.div_euclid(2 * codec::REGION_TILES),
                    ),
                    LoadTier::Sample,
                );
                // The exact changed cells (+1: exclusive bound); the zoom-
                // dependent southeast relief fringe is added by
                // mark_full_tiles_dirty itself.
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
    /// whatever arrived, and (with the map closed) trim the caches.
    pub(crate) fn pump_store(&mut self) {
        for arrival in self.store.pump_loads() {
            if arrival.had_data {
                self.mark_full_tiles_dirty(region_block_rect(arrival.kind, arrival.coord));
            }
        }
        if self.open_canvas.as_deref() != Some(FULL_CANVAS) {
            let sample_center = self.last_sample.unwrap_or((
                self.player[0].floor() as i32,
                self.player[2].floor() as i32,
            ));
            self.store.trim_caches(|kind, region| {
                // Protect the live sampling neighborhood.
                let rect = region_block_rect(kind, region);
                let pad = SAMPLE_RADIUS + 16;
                rect[2] > sample_center.0 - pad
                    && rect[0] < sample_center.0 + pad
                    && rect[3] > sample_center.1 - pad
                    && rect[1] < sample_center.1 + pad
            });
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

/// The shared relief rule: brighten terrain rising toward the northwest.
/// Integer throughout — a fixed-point multiplier LUT over the clamped height
/// delta keeps the raster hot loop free of float math. `shade_rgb_reference`
/// pins the intended curve.
pub(crate) fn shade_rgb(rgb: [u8; 3], height: i16, northwest: i16) -> [u8; 3] {
    // clamp(1 + delta*0.035, 0.82, 1.16): saturated for |delta| >= 6.
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

/// The float formulation of the relief rule, kept as the LUT's test oracle.
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

    #[test]
    fn a_region_is_resident_wholesale_and_evicts_wholesale() {
        let mut store = store_with_region(RegionKind::Base, (2, -1));
        assert!(store.region_resident(RegionKind::Base, (2, -1)));
        for tz in -4..0 {
            for tx in 8..12 {
                assert!(store.tiles.contains_key(&(tx, tz)), "member ({tx},{tz})");
            }
        }
        // Under the cap: nothing trims.
        store.trim_caches(|_, _| false);
        assert!(store.region_resident(RegionKind::Base, (2, -1)));
        // Over the cap: whole regions leave together, except protected or
        // dirty ones.
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
        for (&(rx, rz), _) in &store.base_regions {
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
    fn dirty_tiles_merge_into_their_mip_at_flush_granularity() {
        let mut store = store_with_region(RegionKind::Base, (0, 0));
        store.materialize_region(RegionKind::Mip, (0, 0));
        let tile = store.tiles.get_mut(&(1, 1)).unwrap();
        // Two known blocks in one 2×2 group + a fully unknown group.
        tile.tile.cells[0] = Cell { height: 10, rgb: [100, 0, 0] };
        tile.tile.cells[1] = Cell { height: 20, rgb: [100, 0, 0] };
        store.merge_tile_into_mip((1, 1));
        let mip = store.mips.get(&(0, 0)).expect("mip tile resident");
        // Tile (1,1) covers blocks 16..32: its quadrant starts at mip cell
        // (8, 8) in mip tile (0, 0).
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
