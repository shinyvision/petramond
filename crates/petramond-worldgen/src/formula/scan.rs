//! Column scans: every requested height of one column evaluated together.
//!
//! A cell-by-cell evaluation re-dispatches every operator per cell. A scan
//! keeps one lane per height, so each operator runs once over a short vector,
//! and a sampling operator asked the same question on consecutive cells (a
//! noise sampled at a banded height, a column-constant seed) answers once.
//! The arithmetic per lane is exactly the point evaluation's, in the same
//! order, so a scan and a point never disagree on a cell.

use super::{apply, Formula, HeightTable, Inputs, Node, Noises, Op};
use std::sync::Arc;

/// Reusable scratch for scanning one formula's columns.
pub struct Scan<'a> {
    formula: &'a Formula,
    seed: u32,
    noises: Noises,
    values: Vec<f64>,
    table: Option<Arc<HeightTable>>,
    rows: Vec<usize>,
    /// Register file of the lattice pass of [`Self::run_lattice`].
    corners: Vec<f64>,
}

impl Formula {
    pub fn scan(&self, seed: u32) -> Scan<'_> {
        let noises = self.seeded_noises(seed);
        let table = self.height_table(seed, &noises);
        Scan {
            formula: self,
            seed,
            noises,
            values: Vec::new(),
            table,
            rows: Vec::new(),
            corners: Vec::new(),
        }
    }
}

/// The table rows of every height in `ys`, when the table covers them all.
pub(super) fn table_rows(
    table: Option<&Arc<HeightTable>>,
    ys: &[f64],
    rows: &mut Vec<usize>,
) -> bool {
    rows.clear();
    let Some(_) = table else {
        return false;
    };
    for &y in ys {
        match HeightTable::index(y) {
            Some(k) => rows.push(k),
            None => return false,
        }
    }
    true
}

/// Lay a tabulated node out per lane — only when something else reads it —
/// and say whether the node was tabulated at all.
#[inline]
pub(super) fn tabulated_op(
    formula: &Formula,
    table: &HeightTable,
    index: usize,
    values: &mut [f64],
    rows: &[usize],
) -> bool {
    let Some(row) = table.row(index) else {
        return false;
    };
    if formula.frontier[index] {
        let lanes = rows.len();
        let out = &mut values[index * lanes..index * lanes + lanes];
        for (o, &k) in out.iter_mut().zip(rows) {
            *o = row[k];
        }
    }
    true
}

impl Scan<'_> {
    /// The outputs at each height of `ys` in one column.
    pub fn run<const N: usize>(&mut self, inputs: Inputs, ys: &[f64], out: &mut Vec<[f64; N]>) {
        self.run_with(inputs, ys, None, out);
    }

    /// [`Self::run`] with every height-dependent sampling operator sampled
    /// at heights `step` apart and interpolated between them: a noise wider
    /// than the step reads the same for a fraction of the samples. `ys`
    /// must ascend.
    pub fn run_lattice<const N: usize>(
        &mut self,
        inputs: Inputs,
        ys: &[f64],
        step: i32,
        out: &mut Vec<[f64; N]>,
    ) {
        self.run_with(inputs, ys, Some(step), out);
    }

    fn run_with<const N: usize>(
        &mut self,
        inputs: Inputs,
        ys: &[f64],
        lattice: Option<i32>,
        out: &mut Vec<[f64; N]>,
    ) {
        let formula = self.formula;
        assert_eq!(N, formula.outputs.len());
        out.clear();
        let lanes = ys.len();
        if lanes == 0 {
            return;
        }
        let lattice = lattice.and_then(|step| {
            let (Some(&lo), Some(&hi)) = (ys.first(), ys.last()) else {
                return None;
            };
            let base = (lo / f64::from(step)).floor() as i32 * step;
            let top = (hi / f64::from(step)).ceil() as i32 * step;
            let count = ((top - base) / step + 1) as usize;
            if count < 2 || count >= lanes {
                return None;
            }
            reserve(&mut self.corners, formula.nodes.len() * count);
            for &i in &formula.column_ops {
                column_op(
                    &formula.nodes[i],
                    i,
                    &mut self.corners,
                    count,
                    inputs,
                    self.seed,
                    &self.noises,
                    formula.broadcast[i],
                );
            }
            let heights: Vec<f64> = (0..count)
                .map(|k| f64::from(base + k as i32 * step))
                .collect();
            for &i in &formula.vertical_ops {
                vertical_op(
                    &formula.nodes[i],
                    i,
                    &mut self.corners,
                    count,
                    &heights,
                    inputs,
                    self.seed,
                    &self.noises,
                );
            }
            Some((f64::from(base), f64::from(step), count))
        });
        reserve(&mut self.values, formula.nodes.len() * lanes);
        for &i in &formula.column_ops {
            column_op(
                &formula.nodes[i],
                i,
                &mut self.values,
                lanes,
                inputs,
                self.seed,
                &self.noises,
                formula.broadcast[i],
            );
        }
        let tabulated = table_rows(self.table.as_ref(), ys, &mut self.rows);
        for &i in &formula.vertical_ops {
            if let Some((base, step, count)) = lattice {
                if formula.nodes[i].op.samples() {
                    for (l, &y) in ys.iter().enumerate() {
                        let f = (y - base) / step;
                        let k = (f.floor().max(0.0) as usize).min(count - 2);
                        let t = f - k as f64;
                        let a = self.corners[i * count + k];
                        let b = self.corners[i * count + k + 1];
                        self.values[i * lanes + l] = a + (b - a) * t;
                    }
                    continue;
                }
            }
            if tabulated
                && tabulated_op(
                    formula,
                    self.table.as_deref().expect("rows imply a table"),
                    i,
                    &mut self.values,
                    &self.rows,
                )
            {
                continue;
            }
            vertical_op(
                &formula.nodes[i],
                i,
                &mut self.values,
                lanes,
                ys,
                inputs,
                self.seed,
                &self.noises,
            );
        }
        out.extend((0..lanes).map(|l| {
            std::array::from_fn(|k| {
                let o = formula.outputs[k];
                self.values[o * lanes + formula.lane_of(o, l)]
            })
        }));
    }
}

/// Grow the register file without clearing it: every used node is written
/// over all its lanes before anything reads it, so stale lanes are never seen.
#[inline]
pub(super) fn reserve(values: &mut Vec<f64>, len: usize) {
    if values.len() < len {
        values.resize(len, 0.0);
    }
}

/// A height-independent node: one value, laid out per lane only when a
/// height-dependent node reads it.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn column_op(
    node: &Node,
    index: usize,
    values: &mut [f64],
    lanes: usize,
    inputs: Inputs,
    seed: u32,
    noises: &Noises,
    broadcast: bool,
) {
    let value = match node.op {
        Op::Value(n) => n,
        Op::Input(i) => inputs.0[i],
        op => apply(op, node.args.map(|a| values[a * lanes]), seed, noises),
    };
    if broadcast {
        values[index * lanes..(index + 1) * lanes].fill(value);
    } else {
        values[index * lanes] = value;
    }
}

/// A height-dependent node over every lane.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn vertical_op(
    node: &Node,
    index: usize,
    values: &mut [f64],
    lanes: usize,
    ys: &[f64],
    inputs: Inputs,
    seed: u32,
    noises: &Noises,
) {
    let base = index * lanes;
    let truth = |v: bool| if v { 1.0 } else { 0.0 };
    match node.op {
        Op::Value(n) => values[base..base + lanes].fill(n),
        Op::Input(1) => values[base..base + lanes].copy_from_slice(ys),
        Op::Input(i) => values[base..base + lanes].fill(inputs.0[i]),
        op if op.samples() => {
            let [a, b, c, d] = node.args.map(|i| i * lanes);
            let mut last: Option<([f64; 4], f64)> = None;
            for l in 0..lanes {
                let args = [values[a + l], values[b + l], values[c + l], values[d + l]];
                let value = match last {
                    Some((saved, value)) if saved.map(f64::to_bits) == args.map(f64::to_bits) => {
                        value
                    }
                    _ => apply(op, args, seed, noises),
                };
                last = Some((args, value));
                values[base + l] = value;
            }
        }
        op => {
            // Arguments always precede their node, so the node's lanes and
            // its arguments' lanes are disjoint halves of the register file.
            let (args, out) = values.split_at_mut(base);
            let out = &mut out[..lanes];
            let lane = |i: usize| &args[node.args[i] * lanes..node.args[i] * lanes + lanes];
            let (a, b, c) = (lane(0), lane(1), lane(2));
            macro_rules! unary {
                (|$x:ident| $e:expr) => {
                    for (o, &$x) in out.iter_mut().zip(a) {
                        *o = $e;
                    }
                };
            }
            macro_rules! binary {
                (|$x:ident, $y:ident| $e:expr) => {
                    for ((o, &$x), &$y) in out.iter_mut().zip(a).zip(b) {
                        *o = $e;
                    }
                };
            }
            macro_rules! ternary {
                (|$x:ident, $y:ident, $z:ident| $e:expr) => {
                    for (((o, &$x), &$y), &$z) in out.iter_mut().zip(a).zip(b).zip(c) {
                        *o = $e;
                    }
                };
            }
            match op {
                Op::Add => binary!(|x, y| x + y),
                Op::Sub => binary!(|x, y| x - y),
                Op::Mul => binary!(|x, y| x * y),
                Op::Div => binary!(|x, y| x / y),
                Op::Min => binary!(|x, y| x.min(y)),
                Op::Max => binary!(|x, y| x.max(y)),
                Op::Abs => unary!(|x| x.abs()),
                Op::Sqrt => unary!(|x| x.sqrt()),
                Op::Pow => binary!(|x, y| x.powf(y)),
                Op::Trunc => unary!(|x| x.trunc()),
                Op::Ceil => unary!(|x| x.ceil()),
                Op::Round => unary!(|x| (x + 0.5).floor()),
                Op::Clamp => ternary!(|x, y, z| x.max(y).min(z)),
                Op::SmoothMin => ternary!(|x, y, z| {
                    let h = (z - (x - y).abs()).max(0.0) / z;
                    x.min(y) - h * h * z * 0.25
                }),
                Op::Step => {
                    binary!(|x, y| ((((x * y * y).trunc() / y) + 0.5).floor() / y).clamp(0.0, 1.0))
                }
                Op::Equal => binary!(|x, y| truth(x == y)),
                Op::Less => binary!(|x, y| truth(x < y)),
                Op::LessEqual => binary!(|x, y| truth(x <= y)),
                Op::Greater => binary!(|x, y| truth(x > y)),
                Op::And => binary!(|x, y| truth(x > 0.0 && y > 0.0)),
                Op::Or => binary!(|x, y| truth(x > 0.0 || y > 0.0)),
                Op::Select => ternary!(|x, y, z| if x > 0.0 { y } else { z }),
                Op::Value(_)
                | Op::Input(_)
                | Op::Noise2
                | Op::Noise3
                | Op::Random
                | Op::Perlin(_) => unreachable!("handled above"),
            }
        }
    }
}
