use super::interval::{self, Interval};
use super::scan::{column_op, reserve, table_rows, tabulated_op, vertical_op};
use super::HeightTable;
use super::{apply, evaluate, Formula, Inputs, Noises, Op, MAX_NODES};
use std::sync::Arc;

/// A dependency partition lets several parameter sets share the same spatial work.
pub struct BatchFormula<'a> {
    formula: &'a Formula,
    shared_column: Vec<usize>,
    shared_vertical: Vec<usize>,
    varied_column: Vec<usize>,
    varied_vertical: Vec<usize>,
    /// Member-independent operands of the first output's conjunction: a
    /// column where one of them is false at every height has no accepting
    /// member, and its members are never evaluated.
    shared_conjuncts: Vec<usize>,
    /// Whether the first output is a predicate whose members may be skipped
    /// once interval arithmetic proves it non-positive at every height.
    predicate: bool,
}

impl Formula {
    /// Select the input lanes that vary between members; Y remains the scan axis.
    pub fn batch(&self, varying: &[usize]) -> BatchFormula<'_> {
        let mut batch = self.batch_predicate(varying);
        batch.shared_conjuncts.clear();
        batch.predicate = false;
        batch
    }

    /// [`Self::batch`] for a formula whose first output is a predicate
    /// conjunction: a column where a member-independent conjunct is false at
    /// every height skips its members, every output reading exactly zero.
    pub fn batch_predicate(&self, varying: &[usize]) -> BatchFormula<'_> {
        assert!(varying.iter().all(|&i| i < 10 && i != 1));
        let mask = varying.iter().fold(0u16, |mask, &i| mask | (1 << i));
        let split = |ops: &[usize]| {
            ops.iter()
                .copied()
                .partition::<Vec<_>, _>(|&i| self.nodes[i].dependencies & mask == 0)
        };
        let (shared_column, varied_column) = split(&self.column_ops);
        let (shared_vertical, varied_vertical) = split(&self.vertical_ops);
        let mut shared_conjuncts = Vec::new();
        if let Some(&root) = self
            .outputs
            .first()
            .filter(|&&r| self.nodes[r].op == Op::And)
        {
            let mut pending = vec![root];
            while let Some(i) = pending.pop() {
                let node = &self.nodes[i];
                if node.op == Op::And {
                    pending.extend(&node.args[..2]);
                } else if node.dependencies & mask == 0 && !matches!(node.op, Op::Value(_)) {
                    shared_conjuncts.push(i);
                }
            }
        }
        BatchFormula {
            formula: self,
            shared_column,
            shared_vertical,
            varied_column,
            varied_vertical,
            shared_conjuncts,
            predicate: true,
        }
    }
}

struct Member {
    inputs: Inputs,
    column: Vec<f64>,
}

pub struct BatchColumn<'a, 'f> {
    batch: &'a BatchFormula<'f>,
    shared: Inputs,
    seed: u32,
    values: [f64; MAX_NODES],
    members: Vec<Member>,
    noises: Noises,
}

impl BatchFormula<'_> {
    /// Every member must agree with `shared` outside the declared varying lanes.
    pub fn column(&self, shared: Inputs, members: &[Inputs]) -> BatchColumn<'_, '_> {
        self.column_seeded(0, shared, members)
    }

    pub fn column_seeded(
        &self,
        seed: u32,
        shared: Inputs,
        members: &[Inputs],
    ) -> BatchColumn<'_, '_> {
        let noises = self.formula.seeded_noises(seed);
        let mut values = [0.0; MAX_NODES];
        for &i in &self.shared_column {
            values[i] = evaluate(&self.formula.nodes[i], &values, shared, seed, &noises);
        }
        let members = members
            .iter()
            .map(|&inputs| {
                for &i in &self.varied_column {
                    values[i] = evaluate(&self.formula.nodes[i], &values, inputs, seed, &noises);
                }
                Member {
                    inputs,
                    column: self.varied_column.iter().map(|&i| values[i]).collect(),
                }
            })
            .collect();
        BatchColumn {
            batch: self,
            shared,
            seed,
            values,
            members,
            noises,
        }
    }

    /// Reusable scratch for scanning whole columns of every member at once.
    pub fn scan(&self, seed: u32) -> BatchScan<'_, '_> {
        let mut varied = vec![false; self.formula.nodes.len()];
        for &i in self.varied_column.iter().chain(&self.varied_vertical) {
            varied[i] = true;
        }
        let noises = self.formula.seeded_noises(seed);
        let table = self.formula.height_table(seed, &noises);
        BatchScan {
            batch: self,
            seed,
            noises,
            table,
            rows: Vec::new(),
            values: Vec::new(),
            outputs: Vec::new(),
            lanes: 0,
            members: 0,
            varied,
            scalar: Vec::new(),
            scalar_member: None,
            ys: Vec::new(),
            ranges: Vec::new(),
        }
    }
}

impl BatchColumn<'_, '_> {
    fn prepare(&mut self, y: f64) {
        let formula = self.batch.formula;
        self.shared.0[1] = y;
        for &i in &self.batch.shared_vertical {
            self.values[i] = evaluate(
                &formula.nodes[i],
                &self.values,
                self.shared,
                self.seed,
                &self.noises,
            );
        }
    }

    fn member<const N: usize>(&mut self, member_index: usize) -> [f64; N] {
        let formula = self.batch.formula;
        assert_eq!(formula.outputs.len(), N);
        let member = &mut self.members[member_index];
        member.inputs.0[1] = self.shared.0[1];
        for (&i, &value) in self.batch.varied_column.iter().zip(&member.column) {
            self.values[i] = value;
        }
        for &i in &self.batch.varied_vertical {
            self.values[i] = evaluate(
                &formula.nodes[i],
                &self.values,
                member.inputs,
                self.seed,
                &self.noises,
            );
        }
        std::array::from_fn(|i| self.values[formula.outputs[i]])
    }

    pub fn at<const N: usize>(&mut self, y: f64, mut visit: impl FnMut(usize, [f64; N])) {
        self.prepare(y);
        for i in 0..self.members.len() {
            visit(i, self.member(i));
        }
    }

    /// Evaluate members in caller order, stopping once the consumer has an answer.
    pub fn find_map<const N: usize, R>(
        &mut self,
        y: f64,
        mut visit: impl FnMut(usize, [f64; N]) -> Option<R>,
    ) -> Option<R> {
        self.prepare(y);
        for i in 0..self.members.len() {
            if let Some(answer) = visit(i, self.member(i)) {
                return Some(answer);
            }
        }
        None
    }
}

/// [`BatchColumn`] over every height of a column at once: the shared work
/// runs once per column instead of once per cell, and each member's varied
/// work runs once per column as a short vector.
pub struct BatchScan<'a, 'f> {
    batch: &'a BatchFormula<'f>,
    seed: u32,
    noises: Noises,
    values: Vec<f64>,
    /// `members × lanes × outputs`, member-major.
    outputs: Vec<f64>,
    lanes: usize,
    members: usize,
    /// Per node, whether it belongs to the varied partition: a lane-wise
    /// member read keeps those as scalars beside the shared lanes.
    varied: Vec<bool>,
    scalar: Vec<f64>,
    scalar_member: Option<usize>,
    ys: Vec<f64>,
    /// Per node, its range over the lanes of the last run (shared nodes) or
    /// the current member's value (varied nodes), for the pruning pass.
    ranges: Vec<Interval>,
    table: Option<Arc<HeightTable>>,
    rows: Vec<usize>,
}

impl BatchScan<'_, '_> {
    /// Evaluate every member at every height of `ys`. Every member must
    /// agree with `shared` outside the declared varying lanes.
    pub fn run(&mut self, shared: Inputs, members: &[Inputs], ys: &[f64]) {
        self.run_until(shared, members, ys, |_, _| false);
    }

    /// [`Self::run`] handing each member's outputs — lane-major, one value
    /// per output per lane — to `done` as soon as they exist. Once `done`
    /// answers `true` the members after it are not evaluated and read as
    /// rejections; a member proven dead is not offered.
    pub fn run_until(
        &mut self,
        shared: Inputs,
        members: &[Inputs],
        ys: &[f64],
        mut done: impl FnMut(usize, &[f64]) -> bool,
    ) {
        let formula = self.batch.formula;
        let lanes = ys.len();
        self.lanes = lanes;
        self.members = members.len();
        let n = formula.outputs.len();
        self.outputs.clear();
        if lanes == 0 || members.is_empty() {
            return;
        }
        reserve(&mut self.values, formula.nodes.len() * lanes);
        for &i in &self.batch.shared_column {
            column_op(
                &formula.nodes[i],
                i,
                &mut self.values,
                lanes,
                shared,
                self.seed,
                &self.noises,
                formula.broadcast[i],
            );
        }
        let tabulated = table_rows(self.table.as_ref(), ys, &mut self.rows);
        for &i in &self.batch.shared_vertical {
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
                shared,
                self.seed,
                &self.noises,
            );
        }
        self.outputs.resize(members.len() * lanes * n, 0.0);
        if !self.alive() {
            // The conjunction is exactly zero at every height for every member.
            self.outputs.fill(0.0);
            return;
        }
        let prune = self.batch.predicate && !self.batch.varied_vertical.is_empty();
        if prune {
            self.shared_ranges(ys);
        }
        for (m, &inputs) in members.iter().enumerate() {
            for &i in &self.batch.varied_column {
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
            if prune && self.member_dead(inputs) {
                // Proven non-positive at every height: the member cannot
                // accept a cell, so its outputs read as a rejection.
                let at = m * lanes * n;
                self.outputs[at..at + lanes * n].fill(0.0);
                continue;
            }
            for &i in &self.batch.varied_vertical {
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
            let at = m * lanes * n;
            for (k, &o) in formula.outputs.iter().enumerate() {
                for l in 0..lanes {
                    self.outputs[at + l * n + k] = self.values[o * lanes + formula.lane_of(o, l)];
                }
            }
            if done(m, &self.outputs[at..at + lanes * n]) {
                self.outputs[at + lanes * n..].fill(0.0);
                return;
            }
        }
    }

    /// Whether the first output is provably never positive at each height
    /// of `ys` for every column and member of a box: interval arithmetic over
    /// the recipe with each input bounded over the box (`None` = unknown)
    /// and the height a point per lane.
    pub fn dead_lanes(&self, inputs: &[Option<[f64; 2]>; 10], ys: &[f64], out: &mut Vec<bool>) {
        let formula = self.batch.formula;
        out.clear();
        let Some(&root) = formula.outputs.first() else {
            out.resize(ys.len(), false);
            return;
        };
        let bound = |k: usize| {
            inputs[k].map_or(Interval::UNKNOWN, |[lo, hi]| {
                Interval::EMPTY.hull_point(lo).hull_point(hi)
            })
        };
        let range = |i: usize, ranges: &[Interval], y: Interval| {
            let node = &formula.nodes[i];
            match node.op {
                Op::Value(n) => Interval::point(n),
                Op::Input(1) => y,
                Op::Input(k) => bound(k),
                Op::Mul if node.args[0] == node.args[1] => interval::square(ranges[node.args[0]]),
                op => interval::apply(op, node.args.map(|a| ranges[a])),
            }
        };
        let mut ranges = vec![Interval::UNKNOWN; formula.nodes.len()];
        for &i in &formula.column_ops {
            ranges[i] = range(i, &ranges, Interval::UNKNOWN);
        }
        for &y in ys {
            let y = Interval::point(y);
            for &i in &formula.vertical_ops {
                ranges[i] = range(i, &ranges, y);
            }
            out.push(ranges[root].never_positive());
        }
    }

    /// The range of every shared node a varied height-dependent node reads,
    /// over the lanes of the last run.
    fn shared_ranges(&mut self, ys: &[f64]) {
        let formula = self.batch.formula;
        let lanes = self.lanes;
        self.ranges.clear();
        self.ranges.resize(formula.nodes.len(), Interval::UNKNOWN);
        self.ranges[1] = ys
            .iter()
            .fold(Interval::EMPTY, |range, &y| range.hull_point(y));
        for &i in &self.batch.varied_vertical {
            let node = &formula.nodes[i];
            for &a in &node.args[..node.arity] {
                if self.varied[a] || a == 1 {
                    continue;
                }
                let mut range = Interval::EMPTY;
                for l in 0..lanes {
                    range = range.hull_point(self.values[a * lanes + formula.lane_of(a, l)]);
                }
                self.ranges[a] = range;
            }
        }
    }

    /// Interval arithmetic over the member's varied height-dependent nodes,
    /// with its column values as points and the shared nodes as their lane
    /// ranges: true when the first output is provably never positive.
    fn member_dead(&mut self, inputs: Inputs) -> bool {
        let formula = self.batch.formula;
        let lanes = self.lanes;
        let Some(&root) = formula.outputs.first() else {
            return false;
        };
        if !self.varied[root] {
            return false;
        }
        for &i in &self.batch.varied_column {
            self.ranges[i] = Interval::point(self.values[i * lanes]);
        }
        for &i in &self.batch.varied_vertical {
            let node = &formula.nodes[i];
            self.ranges[i] = match node.op {
                Op::Value(n) => Interval::point(n),
                Op::Input(1) => self.ranges[1],
                Op::Input(k) => Interval::point(inputs.0[k]),
                op => interval::apply(op, node.args.map(|a| self.ranges[a])),
            };
        }
        self.ranges[root].never_positive()
    }

    /// Whether any lane can still accept after the shared work: false once a
    /// shared conjunct of the first output is zero (or not a number) in
    /// every lane, which makes that output exactly zero for every member.
    fn alive(&self) -> bool {
        let formula = self.batch.formula;
        let conjuncts = &self.batch.shared_conjuncts;
        if conjuncts.is_empty() {
            return true;
        }
        (0..self.lanes).any(|l| {
            conjuncts
                .iter()
                .all(|&c| self.values[c * self.lanes + formula.lane_of(c, l)] > 0.0)
        })
    }

    /// Only the shared work over `ys`; members are then read one lane at a
    /// time with [`Self::member_lane`], which suits a search over heights
    /// that stops after a few probes.
    pub fn run_shared(&mut self, shared: Inputs, ys: &[f64]) {
        let formula = self.batch.formula;
        let lanes = ys.len();
        self.lanes = lanes;
        self.members = 0;
        self.outputs.clear();
        self.scalar_member = None;
        self.ys.clear();
        self.ys.extend_from_slice(ys);
        if lanes == 0 {
            return;
        }
        reserve(&mut self.values, formula.nodes.len() * lanes);
        for &i in &self.batch.shared_column {
            column_op(
                &formula.nodes[i],
                i,
                &mut self.values,
                lanes,
                shared,
                self.seed,
                &self.noises,
                formula.broadcast[i],
            );
        }
        let tabulated = table_rows(self.table.as_ref(), ys, &mut self.rows);
        for &i in &self.batch.shared_vertical {
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
                shared,
                self.seed,
                &self.noises,
            );
        }
    }

    /// One member's outputs at `lane` of the last [`Self::run_shared`]. The
    /// member's height-independent work is kept between consecutive reads
    /// of the same `member` index, so a search pays only the vertical part.
    pub fn member_lane<const N: usize>(
        &mut self,
        member: usize,
        inputs: Inputs,
        lane: usize,
    ) -> [f64; N] {
        let formula = self.batch.formula;
        debug_assert_eq!(N, formula.outputs.len());
        debug_assert!(lane < self.lanes);
        let lanes = self.lanes;
        reserve(&mut self.scalar, formula.nodes.len());
        if self.scalar_member != Some(member) {
            self.scalar_member = Some(member);
            for &i in &self.batch.varied_column {
                let node = &formula.nodes[i];
                self.scalar[i] = match node.op {
                    Op::Value(n) => n,
                    Op::Input(k) => inputs.0[k],
                    op => apply(
                        op,
                        node.args.map(|a| {
                            if self.varied[a] {
                                self.scalar[a]
                            } else {
                                self.values[a * lanes]
                            }
                        }),
                        self.seed,
                        &self.noises,
                    ),
                };
            }
        }
        let mut inputs = inputs;
        inputs.0[1] = self.ys[lane];
        for &i in &self.batch.varied_vertical {
            let node = &formula.nodes[i];
            self.scalar[i] = match node.op {
                Op::Value(n) => n,
                Op::Input(k) => inputs.0[k],
                op => apply(
                    op,
                    node.args.map(|a| {
                        if self.varied[a] {
                            self.scalar[a]
                        } else {
                            self.values[a * lanes + formula.lane_of(a, lane)]
                        }
                    }),
                    self.seed,
                    &self.noises,
                ),
            };
        }
        std::array::from_fn(|k| {
            let o = formula.outputs[k];
            if self.varied[o] {
                self.scalar[o]
            } else {
                self.values[o * lanes + formula.lane_of(o, lane)]
            }
        })
    }

    /// The outputs of `member` at the height `ys[lane]` of the last run.
    #[inline]
    pub fn output<const N: usize>(&self, member: usize, lane: usize) -> [f64; N] {
        debug_assert_eq!(N, self.batch.formula.outputs.len());
        debug_assert!(member < self.members && lane < self.lanes);
        let at = (member * self.lanes + lane) * N;
        std::array::from_fn(|k| self.outputs[at + k])
    }
}
