//! Chains: three single-cell rows — `furniture:chain` (vertical, the
//! item-linked base), `furniture:chain_ns`, `furniture:chain_ew` — sharing
//! ONE custom shape (`shapes.json` + the bakes in `lib.rs`); the axis
//! is block IDENTITY (the ladder-row pattern), so the bake orients each cell
//! from its block id alone and placement needs no per-cell state. The
//! placement plan picks the sibling row from the clicked face's normal and
//! returns it as the plan's block override. Placement is fully
//! deterministic, so the ENGINE predicts it whole: the client runs the same
//! plan + gates against its replica and ghosts the exact write — the mod
//! ships no placement predictor of its own.

use mod_sdk::*;

/// The chain family: the shared shape-kind id and its three axis rows —
/// vertical (the item-linked base), north/south, east/west.
pub(super) struct Chains {
    pub(super) shape: u8,
    pub(super) rows: [BlockId; 3],
}

/// The plate pair per axis (matching `Chains::rows`): two crossing
/// 2/16-thick, 3/16-wide plates — the vanilla chain geometry. ONE geometry
/// source for the sim and render bakes so collision, selection, and the
/// drawn boxes can't drift; the mesher alpha-cuts the plates out of the
/// row's link tiles.
pub(super) const PLATES: [[ShapeAabb; 2]; 3] = [
    // vertical
    [
        ShapeAabb {
            min: [6.5 / 16.0, 0.0, 7.0 / 16.0],
            max: [9.5 / 16.0, 1.0, 9.0 / 16.0],
        },
        ShapeAabb {
            min: [7.0 / 16.0, 0.0, 6.5 / 16.0],
            max: [9.0 / 16.0, 1.0, 9.5 / 16.0],
        },
    ],
    // north/south
    [
        ShapeAabb {
            min: [6.5 / 16.0, 7.0 / 16.0, 0.0],
            max: [9.5 / 16.0, 9.0 / 16.0, 1.0],
        },
        ShapeAabb {
            min: [7.0 / 16.0, 6.5 / 16.0, 0.0],
            max: [9.0 / 16.0, 9.5 / 16.0, 1.0],
        },
    ],
    // east/west
    [
        ShapeAabb {
            min: [0.0, 6.5 / 16.0, 7.0 / 16.0],
            max: [1.0, 9.5 / 16.0, 9.0 / 16.0],
        },
        ShapeAabb {
            min: [0.0, 7.0 / 16.0, 6.5 / 16.0],
            max: [1.0, 9.0 / 16.0, 9.5 / 16.0],
        },
    ],
];

impl Chains {
    /// The row for a clicked face's normal: top/bottom hangs a vertical
    /// chain, a side face lays it along that face's axis (vanilla rule).
    pub(super) fn row_for_normal(&self, n: [i32; 3]) -> BlockId {
        if n[1] != 0 {
            self.rows[0]
        } else if n[0] != 0 {
            self.rows[2]
        } else {
            self.rows[1]
        }
    }

    /// The plate pair for a placed chain cell, oriented by its block id
    /// (the bake's whole orientation input — a pure function of the cell).
    pub(super) fn plates_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let axis = self.rows.iter().position(|&r| r == block).unwrap_or(0);
        PLATES[axis].to_vec()
    }
}

/// Resolve the chain family at init: the shared shape kind and its three
/// axis rows (registry-only calls, legal on any instance — the bakes and the
/// placement plan run on both). `None` when the pack content didn't load (a
/// row renamed or removed) — the chair half of the mod keeps working and
/// chains fall back to the row's static (cube) shape.
pub(super) fn resolve_chains() -> Option<Chains> {
    Some(Chains {
        shape: resolve_shape("furniture:chain")?,
        rows: [
            resolve_block("furniture:chain")?,
            resolve_block("furniture:chain_ns")?,
            resolve_block("furniture:chain_ew")?,
        ],
    })
}
