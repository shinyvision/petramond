use std::f32::consts::TAU;

/// A closed loop of cells around `center`, every consecutive pair (and last→first) sharing a
/// face, so a wall laid on it has no diagonal gap. `radius_at(cos th, sin th)` gives the
/// outline's radius in the direction at angle `th` (measured from +X toward +Z); `mean` sizes
/// the sampling.
pub fn closed_ring(
    center: [i32; 2],
    mean: f32,
    radius_at: impl Fn(f32, f32) -> f32,
) -> Vec<[i32; 2]> {
    let [cx, cz] = center.map(|v| v as f32);
    let samples = (TAU * mean.max(1.0) * 3.0).ceil() as usize;
    let step = TAU / samples as f32;
    let (step_cos, step_sin) = (step.cos(), step.sin());
    let (mut cos, mut sin) = (1.0f32, 0.0f32);
    let mut raw: Vec<[i32; 2]> = Vec::with_capacity(samples);
    for i in 0..samples {
        // Stepping the direction by rotation drifts; re-anchoring it now and then bounds that.
        if i % 64 == 0 {
            let th = i as f32 * step;
            (cos, sin) = (th.cos(), th.sin());
        }
        let r = radius_at(cos, sin);
        let p = [
            (cx + r * cos).round_ties_even() as i32,
            (cz + r * sin).round_ties_even() as i32,
        ];
        if raw.last() != Some(&p) {
            raw.push(p);
        }
        (cos, sin) = (
            cos * step_cos - sin * step_sin,
            sin * step_cos + cos * step_sin,
        );
    }
    while raw.len() > 1 && raw.first() == raw.last() {
        raw.pop();
    }
    let err = |x: i32, z: i32| {
        let (dx, dz) = (x as f32 - cx, z as f32 - cz);
        let len = (dx * dx + dz * dz).sqrt();
        let (cos, sin) = if len > 0.0 {
            (dx / len, dz / len)
        } else {
            (1.0, 0.0)
        };
        (len - radius_at(cos, sin)).abs()
    };
    let mut cells = Vec::with_capacity(raw.len() * 2);
    for (i, &a) in raw.iter().enumerate() {
        let b = raw[(i + 1) % raw.len()];
        cells.push(a);
        let [mut x, mut z] = a;
        while (b[0] - x).abs() + (b[1] - z).abs() > 1 {
            let (sx, sz) = ((b[0] - x).signum(), (b[1] - z).signum());
            if sx != 0 && sz != 0 {
                if err(x + sx, z) <= err(x, z + sz) {
                    x += sx;
                } else {
                    z += sz;
                }
            } else {
                x += sx;
                z += sz;
            }
            cells.push([x, z]);
        }
    }
    simplify(cells)
}

/// Removes repeats and back-and-forth spurs until no cell appears twice.
fn simplify(mut cells: Vec<[i32; 2]>) -> Vec<[i32; 2]> {
    let (lo, hi) = bounds(&cells);
    let width = (hi[0] - lo[0] + 1) as usize;
    let mut first_seen = vec![usize::MAX; width * (hi[1] - lo[1] + 1) as usize];
    let slot = |c: [i32; 2]| (c[1] - lo[1]) as usize * width + (c[0] - lo[0]) as usize;
    loop {
        let mut out: Vec<[i32; 2]> = Vec::with_capacity(cells.len());
        for c in cells {
            let n = out.len();
            if n > 0 && out[n - 1] == c {
                continue;
            }
            if n > 1 && out[n - 2] == c {
                out.pop();
                continue;
            }
            out.push(c);
        }
        while out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        let mut cut = None;
        for (i, c) in out.iter().enumerate() {
            let seen = &mut first_seen[slot(*c)];
            if *seen != usize::MAX {
                cut = Some((*seen, i));
                break;
            }
            *seen = i;
        }
        for c in &out {
            first_seen[slot(*c)] = usize::MAX;
        }
        match cut {
            Some((first, i)) => {
                out.drain(first..i);
                cells = out;
            }
            None => return out,
        }
    }
}

fn bounds(cells: &[[i32; 2]]) -> ([i32; 2], [i32; 2]) {
    let mut lo = [i32::MAX; 2];
    let mut hi = [i32::MIN; 2];
    for c in cells {
        for a in 0..2 {
            lo[a] = lo[a].min(c[a]);
            hi[a] = hi[a].max(c[a]);
        }
    }
    if cells.is_empty() {
        ([0, 0], [0, 0])
    } else {
        (lo, hi)
    }
}

/// Distance in face steps from a closed ring into the area it encloses, with the ring index
/// each interior cell's distance came from. Ring cells have depth 0.
pub struct RingField {
    min: [i32; 2],
    size: [i32; 2],
    depth: Vec<i16>,
    source: Vec<i32>,
}

pub fn depths_inside(ring: &[[i32; 2]], seed_inside: [i32; 2]) -> RingField {
    let (lo, hi) = bounds(ring);
    let (lo, hi) = ([lo[0] - 1, lo[1] - 1], [hi[0] + 1, hi[1] + 1]);
    let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1];
    let n = (size[0] * size[1]) as usize;
    // Worked on a copy framed by walls, so a step to a neighbour never needs a bounds check.
    const WALL: u8 = 1;
    const INSIDE: u8 = 2;
    let w = size[0] as usize + 2;
    let framed = w * (size[1] as usize + 2);
    let at = |[x, z]: [i32; 2]| (z - lo[1] + 1) as usize * w + (x - lo[0] + 1) as usize;
    let mut cell = vec![0u8; framed];
    for x in 0..w {
        cell[x] = WALL;
        cell[framed - w + x] = WALL;
    }
    for z in 1..=size[1] as usize {
        cell[z * w] = WALL;
        cell[z * w + w - 1] = WALL;
    }
    let mut depth = vec![-1i16; framed];
    let mut source = vec![-1i32; framed];
    let mut queue: Vec<u32> = Vec::with_capacity(n);
    for (i, &c) in ring.iter().enumerate() {
        let k = at(c);
        if depth[k] < 0 {
            depth[k] = 0;
            source[k] = i as i32;
            queue.push(k as u32);
        }
        cell[k] = WALL;
    }
    let inside_field = (0..2).all(|a| (lo[a]..=hi[a]).contains(&seed_inside[a]));
    if inside_field && cell[at(seed_inside)] == 0 {
        cell[at(seed_inside)] = INSIDE;
        let mut stack = vec![at(seed_inside) as u32];
        while let Some(k) = stack.pop() {
            let k = k as usize;
            for nk in [k + 1, k - 1, k + w, k - w] {
                if cell[nk] == 0 {
                    cell[nk] = INSIDE;
                    stack.push(nk as u32);
                }
            }
        }
    }
    let mut head = 0;
    while let Some(&k) = queue.get(head) {
        head += 1;
        let k = k as usize;
        let (d, s) = (depth[k] + 1, source[k]);
        for nk in [k + 1, k - 1, k + w, k - w] {
            if cell[nk] == INSIDE && depth[nk] < 0 {
                depth[nk] = d;
                source[nk] = s;
                queue.push(nk as u32);
            }
        }
    }
    let mut field = RingField {
        min: lo,
        size,
        depth: Vec::with_capacity(n),
        source: Vec::with_capacity(n),
    };
    for z in 1..=size[1] as usize {
        let row = z * w + 1..z * w + 1 + size[0] as usize;
        field.depth.extend_from_slice(&depth[row.clone()]);
        field.source.extend_from_slice(&source[row]);
    }
    field
}

impl RingField {
    #[inline]
    fn index(&self, [x, z]: [i32; 2]) -> Option<usize> {
        let (lx, lz) = (x - self.min[0], z - self.min[1]);
        (lx >= 0 && lz >= 0 && lx < self.size[0] && lz < self.size[1])
            .then(|| (lz * self.size[0] + lx) as usize)
    }

    /// The inclusive column box the field covers: the ring's bounds plus one.
    pub fn bounds(&self) -> ([i32; 2], [i32; 2]) {
        (
            self.min,
            [
                self.min[0] + self.size[0] - 1,
                self.min[1] + self.size[1] - 1,
            ],
        )
    }

    /// The field's depths row by row along x from [`bounds`](RingField::bounds)' minimum: `0` on
    /// the ring, above it inside, negative outside.
    pub fn depth_rows(&self) -> std::slice::Chunks<'_, i16> {
        self.depth.chunks(self.size[0].max(1) as usize)
    }

    /// Face steps from the ring: `Some(0)` on the ring, `Some(d > 0)` inside, `None` outside.
    #[inline]
    pub fn depth(&self, c: [i32; 2]) -> Option<i32> {
        let d = self.depth[self.index(c)?];
        (d >= 0).then_some(d as i32)
    }

    #[inline]
    pub fn inside(&self, c: [i32; 2]) -> bool {
        self.depth(c).is_some_and(|d| d > 0)
    }

    /// The ring index an interior cell's depth was measured from.
    #[inline]
    pub fn source(&self, c: [i32; 2]) -> Option<usize> {
        let s = self.source[self.index(c)?];
        (s >= 0).then_some(s as usize)
    }

    pub fn interior(&self) -> impl Iterator<Item = [i32; 2]> + '_ {
        let [w, _] = self.size;
        self.depth
            .iter()
            .enumerate()
            .filter(|(_, &d)| d > 0)
            .map(move |(i, _)| [self.min[0] + i as i32 % w, self.min[1] + i as i32 / w])
    }
}

/// A cell of a straight band between two columns: `t` is its progress from `a` (0) to `b` (1)
/// and `offset` its signed distance from the centre line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentCell {
    pub pos: [i32; 2],
    pub t: f32,
    pub offset: f32,
}

pub fn segment(a: [i32; 2], b: [i32; 2], half_width: f32) -> Vec<SegmentCell> {
    let (dx, dz) = ((b[0] - a[0]) as f32, (b[1] - a[1]) as f32);
    let len = (dx * dx + dz * dz).sqrt().max(1e-3);
    let (ux, uz) = (dx / len, dz / len);
    let pad = half_width.ceil() as i32 + 1;
    let mut out = Vec::new();
    for z in a[1].min(b[1]) - pad..=a[1].max(b[1]) + pad {
        for x in a[0].min(b[0]) - pad..=a[0].max(b[0]) + pad {
            let (rx, rz) = ((x - a[0]) as f32, (z - a[1]) as f32);
            let along = rx * ux + rz * uz;
            let offset = rz * ux - rx * uz;
            if along < -0.3 || along > len + 0.3 || offset.abs() > half_width {
                continue;
            }
            out.push(SegmentCell {
                pos: [x, z],
                t: (along / len).clamp(0.0, 1.0),
                offset,
            });
        }
    }
    out
}
