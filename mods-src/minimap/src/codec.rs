//! Region-value codec: one storage value bundles a 4×4 grid of 16×16-cell
//! tiles (base tiles: one cell per block; mip tiles: one cell per 2×2
//! blocks). Bundling cuts storage keys 16× versus one-value-per-tile; the
//! packed encoding shrinks values ~3–5× on typical terrain.
//!
//! Layout: `[version]` then 16 sub-tiles in row-major (tz*4 + tx) order.
//! - `RAW_VERSION`: per sub-tile a marker byte (0 = empty, 1 = data); data =
//!   256 × (le i16 height + rgb). The trivially mirrorable form — test
//!   harnesses fabricate it; the decoder accepts it forever.
//! - `PACKED_VERSION`: same markers; data = a run-length stream of cells:
//!   `(run varint, Δheight zigzag varint, Δr, Δg, Δb zigzag varints)`, deltas
//!   against the previous cell in row-major order (starting from zero).
//!   Uniform areas (water, plains) collapse into runs; tint gradients emit
//!   1-byte deltas. Sub-tile marker 2 stores raw cells inside a packed
//!   value — the fallback when packing would exceed raw size, so
//!   pathological noise never inflates a value.
//!
//! Any malformed byte stream decodes to `None` — treated exactly like an
//! absent key, never a partial tile.

use crate::*;

pub(crate) const RAW_VERSION: u8 = 0;
pub(crate) const PACKED_VERSION: u8 = 1;
/// Sub-tiles per region edge; a region value covers 4×4 tiles.
pub(crate) const REGION_TILES: i32 = 4;

fn push_varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut out = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        out |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some(out);
        }
    }
    None
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

fn unzigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// Encode 16 optional sub-tiles (row-major within the region) as one packed
/// region value. `None` sub-tiles are empty (never explored).
pub(crate) fn encode_region(tiles: &[Option<&Tile>; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    out.push(PACKED_VERSION);
    let mut scratch = Vec::with_capacity(256 * 5);
    for tile in tiles {
        let Some(tile) = tile else {
            out.push(0);
            continue;
        };
        scratch.clear();
        let mut prev = Cell {
            height: 0,
            rgb: [0; 3],
        };
        let mut i = 0;
        while i < 256 {
            let cell = tile.cells[i];
            let mut run = 1usize;
            while i + run < 256 && tile.cells[i + run] == cell {
                run += 1;
            }
            push_varint(&mut scratch, run as u32);
            push_varint(&mut scratch, zigzag(cell.height as i32 - prev.height as i32));
            for channel in 0..3 {
                push_varint(
                    &mut scratch,
                    zigzag(cell.rgb[channel] as i32 - prev.rgb[channel] as i32),
                );
            }
            prev = cell;
            i += run;
        }
        if scratch.len() < 256 * 5 {
            out.push(1);
            out.extend_from_slice(&scratch);
        } else {
            // Pathological noise: raw is smaller.
            out.push(2);
            for cell in tile.cells {
                out.extend(cell.height.to_le_bytes());
                out.extend(cell.rgb);
            }
        }
    }
    out
}

/// Decode one region value into 16 optional sub-tiles. `None` result =
/// malformed (treat the whole value as absent).
pub(crate) fn decode_region(bytes: &[u8]) -> Option<[Option<Box<Tile>>; 16]> {
    let version = *bytes.first()?;
    let mut at = 1usize;
    let mut out: [Option<Box<Tile>>; 16] = Default::default();
    for slot in out.iter_mut() {
        let marker = *bytes.get(at)?;
        at += 1;
        match (version, marker) {
            (_, 0) => continue,
            (RAW_VERSION, 1) | (PACKED_VERSION, 2) => {
                let raw = bytes.get(at..at + 256 * 5)?;
                at += 256 * 5;
                let mut tile = Box::new(Tile::default());
                for (cell, chunk) in tile.cells.iter_mut().zip(raw.chunks_exact(5)) {
                    *cell = Cell {
                        height: i16::from_le_bytes([chunk[0], chunk[1]]),
                        rgb: [chunk[2], chunk[3], chunk[4]],
                    };
                }
                *slot = Some(tile);
            }
            (PACKED_VERSION, 1) => {
                let mut tile = Box::new(Tile::default());
                let mut prev = Cell {
                    height: 0,
                    rgb: [0; 3],
                };
                let mut i = 0usize;
                while i < 256 {
                    let run = read_varint(bytes, &mut at)? as usize;
                    if run == 0 || run > 256 - i {
                        return None;
                    }
                    let height = prev.height as i32 + unzigzag(read_varint(bytes, &mut at)?);
                    let height = i16::try_from(height).ok()?;
                    let mut rgb = [0u8; 3];
                    for (channel, byte) in rgb.iter_mut().enumerate() {
                        let v = prev.rgb[channel] as i32 + unzigzag(read_varint(bytes, &mut at)?);
                        *byte = u8::try_from(v).ok()?;
                    }
                    let cell = Cell { height, rgb };
                    tile.cells[i..i + run].fill(cell);
                    prev = cell;
                    i += run;
                }
                *slot = Some(tile);
            }
            _ => return None,
        }
    }
    (at == bytes.len()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(seed: i32, holes: bool) -> Box<Tile> {
        let mut tile = Box::new(Tile::default());
        for i in 0..256i32 {
            if holes && (i + seed).rem_euclid(11) == 0 {
                continue; // unknown cells survive the roundtrip too
            }
            tile.cells[i as usize] = Cell {
                height: ((i * 3 + seed).rem_euclid(120) - 40) as i16,
                rgb: [
                    (i + seed).rem_euclid(256) as u8,
                    (i * 7).rem_euclid(256) as u8,
                    ((i / 16) * 13).rem_euclid(256) as u8,
                ],
            };
        }
        tile
    }

    /// Smooth heights and a small color set — the realistic terrain shape
    /// the packing targets.
    fn terrain_tile(seed: i32) -> Box<Tile> {
        let palette = [[90, 140, 60], [110, 150, 70], [40, 90, 190], [120, 110, 90]];
        let mut tile = Box::new(Tile::default());
        for i in 0..256i32 {
            let (x, z) = (i % 16, i / 16);
            tile.cells[i as usize] = Cell {
                height: (60 + (x + z + seed) / 6) as i16,
                rgb: palette[((x / 5 + z / 4 + seed) % 4) as usize],
            };
        }
        tile
    }

    #[test]
    fn packed_region_roundtrips_and_noise_never_inflates() {
        // Adversarially noisy cells with holes: exact roundtrip, and the
        // per-tile raw fallback bounds the size at ~raw.
        let tiles: Vec<Option<Box<Tile>>> = (0..16)
            .map(|i| (i % 3 != 2).then(|| tile(i, i % 2 == 0)))
            .collect();
        let refs: [Option<&Tile>; 16] =
            std::array::from_fn(|i| tiles[i].as_deref());
        let encoded = encode_region(&refs);
        let decoded = decode_region(&encoded).expect("well-formed value decodes");
        for (i, (a, b)) in tiles.iter().zip(decoded.iter()).enumerate() {
            match (a, b) {
                (None, None) => {}
                (Some(a), Some(b)) => assert_eq!(a.cells[..], b.cells[..], "sub-tile {i}"),
                _ => panic!("sub-tile {i} presence diverged"),
            }
        }
        let raw_size = 1 + 16 + 11 * 256 * 5;
        assert!(
            encoded.len() <= raw_size,
            "noise stays bounded: packed {} vs raw {raw_size}",
            encoded.len()
        );
    }

    #[test]
    fn realistic_terrain_shrinks_severalfold() {
        let tiles: Vec<Box<Tile>> = (0..16).map(terrain_tile).collect();
        let refs: [Option<&Tile>; 16] = std::array::from_fn(|i| Some(tiles[i].as_ref()));
        let encoded = encode_region(&refs);
        let decoded = decode_region(&encoded).expect("decodes");
        assert_eq!(
            decoded[7].as_ref().unwrap().cells[..],
            tiles[7].cells[..]
        );
        let raw_size = 1 + 16 + 16 * 256 * 5;
        assert!(
            encoded.len() * 2 < raw_size,
            "terrain packs at least 2x: {} vs {raw_size}",
            encoded.len()
        );
    }

    #[test]
    fn uniform_terrain_collapses_into_runs() {
        let mut uniform = Box::new(Tile::default());
        uniform.cells.fill(Cell {
            height: 12,
            rgb: [30, 90, 200],
        });
        let refs: [Option<&Tile>; 16] = std::array::from_fn(|_| Some(uniform.as_ref()));
        let encoded = encode_region(&refs);
        assert!(encoded.len() < 200, "uniform region packs tiny: {}", encoded.len());
        assert!(decode_region(&encoded).is_some());
    }

    #[test]
    fn raw_version_decodes_for_test_harnesses() {
        let source = tile(5, true);
        let mut raw = vec![RAW_VERSION];
        for i in 0..16 {
            if i != 3 {
                raw.push(0);
                continue;
            }
            raw.push(1);
            for cell in source.cells {
                raw.extend(cell.height.to_le_bytes());
                raw.extend(cell.rgb);
            }
        }
        let decoded = decode_region(&raw).expect("raw mode decodes");
        assert_eq!(decoded[3].as_ref().unwrap().cells[..], source.cells[..]);
        assert!(decoded[0].is_none());
    }

    #[test]
    fn malformed_values_decode_to_none() {
        let refs: [Option<&Tile>; 16] = std::array::from_fn(|i| (i == 0).then(|| {
            Box::leak(tile(1, false)) as &Tile
        }));
        let good = encode_region(&refs);
        assert!(decode_region(&good).is_some());
        for truncate_at in [0, 1, 2, good.len() / 2, good.len() - 1] {
            assert!(
                decode_region(&good[..truncate_at]).is_none(),
                "truncation at {truncate_at} must not decode"
            );
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode_region(&trailing).is_none(), "trailing bytes rejected");
        assert!(decode_region(&[9]).is_none(), "unknown version rejected");
    }
}
