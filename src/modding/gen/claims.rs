//! Columns a feature keeps for itself, asked for a tile at a time and remembered for the session.

use std::sync::{Arc, RwLock};

use mod_api::{ColumnBox, ColumnMask, GenClaims};
use rustc_hash::FxHashMap;

/// Claims are asked for per aligned tile of `TILE` × `TILE` columns.
pub(super) const TILE: i32 = 128;
const MAX_CLAIMS: usize = 256;
const MAX_CLAIM_COLUMNS: i128 = 1 << 20;

pub(super) type TileClaims = Arc<[Arc<ColumnMask>]>;

#[derive(Default)]
pub(super) struct ClaimTiles {
    tiles: RwLock<FxHashMap<[i32; 2], TileClaims>>,
}

impl ClaimTiles {
    pub(super) fn get(&self, tile: [i32; 2]) -> Option<TileClaims> {
        let tiles = self.tiles.read().unwrap_or_else(|e| e.into_inner());
        tiles.get(&tile).cloned()
    }

    pub(super) fn put(&self, tile: [i32; 2], claims: TileClaims) {
        let mut tiles = self.tiles.write().unwrap_or_else(|e| e.into_inner());
        tiles.insert(tile, claims);
    }
}

pub(super) fn tile_box([tx, tz]: [i32; 2]) -> ColumnBox {
    ColumnBox {
        min: [tx * TILE, tz * TILE],
        max: [tx * TILE + TILE - 1, tz * TILE + TILE - 1],
    }
}

/// A reply's claims, or `None` when it deferred.
pub(super) fn validated(reply: GenClaims) -> Result<Option<TileClaims>, String> {
    if reply.deferred {
        if !super::super::has_pending_key() {
            return Err("deferred its claims without a pending memo claim to wait on".into());
        }
        return Ok(None);
    }
    if reply.claims.len() > MAX_CLAIMS {
        return Err("claims reply exceeds its budget".into());
    }
    for mask in &reply.claims {
        let side = |a: usize| i128::from(mask.bounds.max[a]) - i128::from(mask.bounds.min[a]) + 1;
        if !mask.is_well_formed() || side(0) * side(1) > MAX_CLAIM_COLUMNS {
            return Err(format!(
                "claim over {:?}..={:?} is malformed or too large",
                mask.bounds.min, mask.bounds.max
            ));
        }
    }
    Ok(Some(reply.claims.into_iter().map(Arc::new).collect()))
}
