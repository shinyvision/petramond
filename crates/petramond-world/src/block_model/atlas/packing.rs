//! Deterministic shelf packing, independent of model and texture storage.

pub(super) struct Layout {
    pub size: [u32; 2],
    pub origins: Vec<[u32; 2]>,
}

pub(super) fn pack(sizes: &[[u32; 2]], limit: u32) -> Result<Layout, String> {
    if sizes
        .iter()
        .any(|s| s.contains(&0) || s.iter().any(|&v| v > limit))
    {
        return Err(format!(
            "model texture dimensions must be within 1..={limit}"
        ));
    }
    let mut order: Vec<_> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(sizes[i][1]),
            std::cmp::Reverse(sizes[i][0]),
            i,
        )
    });
    let area: u64 = sizes
        .iter()
        .map(|s| u64::from(s[0]) * u64::from(s[1]))
        .sum();
    let widest = sizes.iter().map(|s| s[0]).max().unwrap_or(1);
    let mut width = widest
        .max((area as f64).sqrt().ceil() as u32)
        .next_power_of_two()
        .min(limit);
    loop {
        let mut origins = vec![[0, 0]; sizes.len()];
        let (mut x, mut y, mut shelf) = (0, 0, 0);
        for &i in &order {
            let [w, h] = sizes[i];
            if x + w > width {
                y += shelf;
                x = 0;
                shelf = 0;
            }
            origins[i] = [x, y];
            x += w;
            shelf = shelf.max(h);
        }
        let height = (y + shelf).max(1);
        if height <= limit {
            return Ok(Layout {
                size: [width, height],
                origins,
            });
        }
        if width == limit {
            return Err(format!(
                "model textures exceed the {limit}×{limit} atlas capacity"
            ));
        }
        width = (width * 2).min(limit);
    }
}

#[cfg(test)]
mod tests;
