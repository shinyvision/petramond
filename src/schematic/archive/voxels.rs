use super::bytes::{self, Reader};

fn bits(n: usize) -> usize {
    (usize::BITS - (n.saturating_sub(1)).leading_zeros()) as usize
}
fn packed(values: impl IntoIterator<Item = usize>, bits: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pending = 0u64;
    let mut used = 0;
    for value in values {
        pending |= (value as u64) << used;
        used += bits;
        while used >= 8 {
            out.push(pending as u8);
            pending >>= 8;
            used -= 8;
        }
    }
    if used != 0 {
        out.push(pending as u8);
    }
    out
}
fn unpacked<R: std::io::Read>(
    r: &mut Reader<R>,
    count: usize,
    bits: usize,
    max: usize,
) -> Result<impl Iterator<Item = Result<usize, String>>, String> {
    let bytes = r.take((count * bits).div_ceil(8))?;
    let tail = count * bits % 8;
    if tail != 0 && bytes.last().is_some_and(|b| b >> tail != 0) {
        return Err("Nonzero voxel padding".into());
    }
    let mut pending = 0u64;
    let mut used = 0;
    let mut input = bytes.into_iter();
    Ok((0..count).map(move |_| {
        while used < bits {
            pending |= u64::from(input.next().expect("validated packed length")) << used;
            used += 8;
        }
        let value = (pending & ((1 << bits) - 1)) as usize;
        pending >>= bits;
        used -= bits;
        if value > max {
            Err("Invalid voxel palette reference".into())
        } else {
            Ok(value)
        }
    }))
}

pub(super) fn encode(cells: &[(usize, usize)], palette_len: usize, volume: usize) -> Vec<u8> {
    let mut sparse = vec![0];
    let mut next = 0;
    for &(pos, _) in cells {
        bytes::uint(&mut sparse, pos - next);
        next = pos + 1;
    }
    sparse.extend(packed(cells.iter().map(|c| c.1), bits(palette_len)));
    let mut runs = vec![1];
    let mut i = 0;
    let mut next = 0;
    while i < cells.len() {
        let (pos, value) = cells[i];
        let mut end = i + 1;
        while end < cells.len() && cells[end] == (pos + end - i, value) {
            end += 1;
        }
        bytes::uint(&mut runs, pos - next);
        bytes::uint(&mut runs, end - i);
        bytes::uint(&mut runs, value);
        next = pos + end - i;
        i = end;
    }
    let mut best = if sparse.len() <= runs.len() {
        sparse
    } else {
        runs
    };
    let width = bits(palette_len + 1);
    if 1 + (volume * width).div_ceil(8) < best.len() {
        let mut selected = cells.iter().peekable();
        let mut dense = vec![2];
        dense.extend(packed(
            (0..volume).map(|p| {
                if selected.peek().is_some_and(|c| c.0 == p) {
                    selected.next().unwrap().1 + 1
                } else {
                    0
                }
            }),
            width,
        ));
        best = dense;
    }
    best
}

pub(super) fn decode<R: std::io::Read>(
    r: &mut Reader<R>,
    count: usize,
    palette_len: usize,
    volume: usize,
) -> Result<Vec<(usize, usize)>, String> {
    let mut cells = Vec::with_capacity(count);
    match r.byte()? {
        0 => {
            let mut next = 0;
            for _ in 0..count {
                let pos = next + r.uint(volume)?;
                if pos >= volume {
                    return Err("Voxel is outside schematic bounds".into());
                }
                cells.push((pos, 0));
                next = pos + 1;
            }
            for (cell, value) in
                cells
                    .iter_mut()
                    .zip(unpacked(r, count, bits(palette_len), palette_len - 1)?)
            {
                cell.1 = value?;
            }
        }
        1 => {
            let mut next = 0;
            while cells.len() < count {
                let pos = next + r.uint(volume)?;
                let len = r.uint(count - cells.len())?;
                let value = r.uint(palette_len - 1)?;
                if len == 0 || pos + len > volume {
                    return Err("Invalid schematic run".into());
                }
                cells.extend((pos..pos + len).map(|p| (p, value)));
                next = pos + len;
            }
        }
        2 => {
            for (pos, value) in unpacked(r, volume, bits(palette_len + 1), palette_len)?.enumerate()
            {
                let value = value?;
                if value != 0 {
                    if cells.len() == count {
                        return Err("Schematic cell count mismatch".into());
                    }
                    cells.push((pos, value - 1));
                }
            }
            if cells.len() != count {
                return Err("Schematic cell count mismatch".into());
            }
        }
        _ => return Err("Unknown schematic voxel encoding".into()),
    }
    Ok(cells)
}
