//! The terrain graph's y-invariant climate at a world column, memoized once
//! for everything that reads it: the surface (height lattice, biome climate)
//! and the caves (depth, habitats, pools). All of them sample the same
//! world-anchored 4-block grid, so one evaluation per column serves them all.

use std::sync::Arc;

use super::terrain::{channels, TerrainDensityGraph};
use crate::cache::local::{self, LocalTable};
use crate::cache::GenCaches;
use crate::graph::{NodeId, SamplePoint};

/// Temperature, humidity, continentality, erosion, variance, base height.
pub type ColumnClimate = [f64; 6];

const CHANNELS: [&str; 6] = [
    channels::TEMPERATURE,
    channels::HUMIDITY,
    channels::CONTINENTALITY,
    channels::EROSION,
    channels::VARIANCE,
    channels::BASE_HEIGHT,
];

/// Columns are sampled on a 4-block grid and memoized [`TILE`]² at a time.
const STEP: i32 = 4;
const TILE: i32 = 4;

/// A tile of columns, `(z * TILE + x)` in grid steps.
pub(crate) type Tile = [ColumnClimate; (TILE * TILE) as usize];

/// Which terrain graph instance ([`crate::graph::ScalarGraph::id`]) and which tile.
pub(crate) type Key = (u64, [i32; 2]);

thread_local! {
    static LOCAL: LocalTable<Key, Arc<Tile>> = LocalTable::new(&local::TERRAIN_COLUMNS);
}

#[derive(Clone)]
pub(crate) struct Columns {
    graph: Arc<TerrainDensityGraph>,
    id: u64,
    nodes: [NodeId; 6],
    caches: Arc<GenCaches>,
}

impl Columns {
    pub(crate) fn new(graph: Arc<TerrainDensityGraph>, caches: Arc<GenCaches>) -> Self {
        let nodes = CHANNELS.map(|channel| {
            graph
                .graph()
                .channel_node(channel)
                .expect("terrain climate channel")
        });
        Self {
            id: graph.graph().id(),
            graph,
            nodes,
            caches,
        }
    }

    pub(crate) fn with_caches(mut self, caches: Arc<GenCaches>) -> Self {
        self.caches = caches;
        self
    }

    /// The channels at `(x, 0, z)`, exactly as the graph evaluates them.
    /// Every caller samples the 4-block grid.
    pub(crate) fn at(&self, x: i32, z: i32) -> ColumnClimate {
        debug_assert!(
            x % STEP == 0 && z % STEP == 0,
            "column off the 4-block grid"
        );
        let span = STEP * TILE;
        let tile = [x.div_euclid(span), z.div_euclid(span)];
        let key = (self.id, tile);
        let hash = (tile[0] as u32 as u64) ^ (tile[1] as u32 as u64).rotate_left(31) ^ self.id;
        let columns = LOCAL.with(|table| {
            table.get_or_insert_with(local::spread(hash), key, || {
                self.caches
                    .terrain
                    .columns
                    .get_or_compute_unlocked(key, || Arc::new(self.evaluate(tile)))
            })
        });
        columns[(z.rem_euclid(span) / STEP * TILE + x.rem_euclid(span) / STEP) as usize]
    }

    fn evaluate(&self, [tx, tz]: [i32; 2]) -> Tile {
        let points: [SamplePoint; (TILE * TILE) as usize] = std::array::from_fn(|i| {
            let (gx, gz) = (i as i32 % TILE, i as i32 / TILE);
            SamplePoint::new(
                f64::from((tx * TILE + gx) * STEP),
                0.0,
                f64::from((tz * TILE + gz) * STEP),
            )
        });
        let mut out = [[0.0; 6]; (TILE * TILE) as usize];
        self.graph
            .graph()
            .evaluate_nodes_batch(self.nodes, &points, &mut out);
        out
    }

    /// The base-height channel's node: it is y-invariant, so a height lattice
    /// may take its value from [`Columns::at`].
    pub(crate) fn base_height_node(&self) -> NodeId {
        self.nodes[5]
    }

    pub(crate) fn graph(&self) -> &TerrainDensityGraph {
        &self.graph
    }
}

impl std::fmt::Debug for Columns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Columns")
            .field("graph", &self.id)
            .finish_non_exhaustive()
    }
}
