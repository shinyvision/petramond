//! Bounded arithmetic recipes compiled once into registers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const MAX_NODES: usize = 512;

/// Heights a table of height-only nodes covers: the world's, with room for
/// the padding a batch or tile asks about beyond it.
const TABLE_Y0: i32 = -128;
const TABLE_LEN: usize = 640;

/// The values of every node that reads nothing but the height, over every
/// height a scan can ask about, for one seed. A geology pattern is mostly a
/// chain of height bands; per column only the nodes touching the column's
/// own inputs are left to evaluate.
#[derive(Debug)]
pub struct HeightTable {
    /// Per height-only node (in its `slot`), `TABLE_LEN` values.
    values: Box<[f64]>,
    /// Per node, its row in `values`, or `u16::MAX` for nodes not tabulated.
    slot: Box<[u16]>,
}

impl HeightTable {
    /// The row index of height `y`, when it is an integer the table covers.
    #[inline]
    fn index(y: f64) -> Option<usize> {
        let k = y - f64::from(TABLE_Y0);
        (k >= 0.0 && k < TABLE_LEN as f64 && k.fract() == 0.0).then_some(k as usize)
    }

    /// The node's values over the table's heights, if tabulated.
    #[inline]
    fn row(&self, node: usize) -> Option<&[f64]> {
        let slot = self.slot[node];
        (slot != u16::MAX).then(|| &self.values[usize::from(slot) * TABLE_LEN..][..TABLE_LEN])
    }
}

mod batch;
mod interval;
mod noise;
mod scan;
pub use batch::{BatchFormula, BatchScan};
pub use noise::{NoiseExpression, Perlin};
pub use scan::Scan;

type Noises = Vec<std::sync::Arc<crate::density::noise::ReferenceDoublePerlin>>;

/// Parameters supplied by a positioned field or a surface-rule caller.
#[derive(Clone, Copy, Debug)]
pub struct Inputs(pub [f64; 10]);

const INPUTS: [&str; 10] = [
    "x", "y", "z", "center_x", "center_y", "center_z", "radius", "height", "surface", "sea",
];

/// Expressions use numbers, previously declared names, or `[operator, operands…]`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Expression {
    Number(f64),
    Name(String),
    Operation(Vec<Expression>),
    Sample(Box<NoiseExpression>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Value(f64),
    Input(usize),
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Abs,
    Sqrt,
    Pow,
    Trunc,
    Round,
    Ceil,
    Clamp,
    SmoothMin,
    Step,
    Less,
    LessEqual,
    Greater,
    Equal,
    And,
    Or,
    Select,
    Noise2,
    Noise3,
    Random,
    Perlin(usize),
}

impl Op {
    /// Sampling operators cost far more than arithmetic, and a scan often
    /// asks them the same question on consecutive cells.
    #[inline]
    fn samples(self) -> bool {
        matches!(
            self,
            Self::Noise2 | Self::Noise3 | Self::Random | Self::Perlin(_)
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    op: Op,
    args: [usize; 4],
    y_dependent: bool,
    dependencies: u16,
    arity: usize,
}

/// A validated acyclic expression graph, independent of any habitat or block.
#[derive(Clone, Debug)]
pub struct Formula {
    nodes: Box<[Node]>,
    noises: Vec<Perlin>,
    outputs: Box<[usize]>,
    column_ops: Box<[usize]>,
    vertical_ops: Box<[usize]>,
    /// Per node, whether a height-dependent node reads it: only those
    /// height-independent values are laid out per lane in a scan.
    broadcast: Box<[bool]>,
    /// Whether any height-dependent node reads only the height (and
    /// constants): those are tabulated per seed instead of evaluated.
    tabulated: bool,
    /// Per node: tabulated, and read by something that is not — an output
    /// or a node touching the column. The tabulated nodes under it are
    /// never materialised: the table already holds what they fed.
    frontier: Box<[bool]>,
    tables: Arc<Mutex<BTreeMap<u32, Arc<HeightTable>>>>,
}

impl Formula {
    pub(crate) fn uses_any(&self, inputs: &[usize]) -> bool {
        let mask = inputs.iter().fold(0u16, |mask, &i| mask | (1 << i));
        self.outputs
            .iter()
            .any(|&i| self.nodes[i].dependencies & mask != 0)
    }

    pub fn compile(
        bindings: &[(String, Expression)],
        outputs: &[Expression],
    ) -> Result<Self, String> {
        let mut noises = Vec::new();
        let mut nodes: Vec<Node> = INPUTS
            .iter()
            .enumerate()
            .map(|(i, _)| Node {
                op: Op::Input(i),
                args: [0; 4],
                y_dependent: i == 1,
                dependencies: 1 << i,
                arity: 0,
            })
            .collect();
        let mut names: BTreeMap<String, usize> = INPUTS
            .iter()
            .enumerate()
            .map(|(i, name)| ((*name).into(), i))
            .collect();
        for (name, expression) in bindings {
            if names.contains_key(name) {
                return Err(format!("duplicate formula name '{name}'"));
            }
            let id = compile(expression, &mut nodes, &names, &mut noises, 0)?;
            names.insert(name.clone(), id);
        }
        let outputs = outputs
            .iter()
            .map(|e| compile(e, &mut nodes, &names, &mut noises, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let mut used = vec![false; nodes.len()];
        let mut pending = outputs.clone();
        while let Some(i) = pending.pop() {
            if used[i] {
                continue;
            }
            used[i] = true;
            if !matches!(nodes[i].op, Op::Value(_) | Op::Input(_)) {
                pending.extend(&nodes[i].args[..nodes[i].arity]);
            }
        }
        let (vertical_ops, column_ops): (Vec<_>, Vec<_>) = (0..nodes.len())
            .filter(|&i| used[i])
            .partition(|&i| nodes[i].y_dependent);
        let mut broadcast = vec![false; nodes.len()];
        for &i in &vertical_ops {
            for &a in &nodes[i].args[..nodes[i].arity] {
                broadcast[a] = !nodes[a].y_dependent;
            }
        }
        let height_only = |i: usize| nodes[i].dependencies == 1 << 1;
        let tabulated = vertical_ops
            .iter()
            .any(|&i| height_only(i) && !matches!(nodes[i].op, Op::Input(_)));
        let mut frontier = vec![false; nodes.len()];
        for &i in &outputs {
            frontier[i] = height_only(i);
        }
        for &i in &vertical_ops {
            if height_only(i) {
                continue;
            }
            for &a in &nodes[i].args[..nodes[i].arity] {
                frontier[a] |= height_only(a);
            }
        }
        Ok(Self {
            nodes: nodes.into(),
            noises,
            outputs: outputs.into(),
            column_ops: column_ops.into(),
            vertical_ops: vertical_ops.into(),
            broadcast: broadcast.into(),
            tabulated,
            frontier: frontier.into(),
            tables: Arc::default(),
        })
    }

    /// The height-only nodes' table for `seed`, built on first use.
    fn height_table(&self, seed: u32, noises: &Noises) -> Option<Arc<HeightTable>> {
        if !self.tabulated {
            return None;
        }
        let mut tables = self
            .tables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(table) = tables.get(&seed) {
            return Some(Arc::clone(table));
        }
        let mut slot = vec![u16::MAX; self.nodes.len()];
        let rows: Vec<usize> = self
            .vertical_ops
            .iter()
            .copied()
            .filter(|&i| self.nodes[i].dependencies == 1 << 1)
            .collect();
        for (row, &i) in rows.iter().enumerate() {
            slot[i] = row as u16;
        }
        let mut values = vec![0.0; rows.len() * TABLE_LEN];
        let mut registers = [0.0; MAX_NODES];
        let mut inputs = Inputs([0.0; 10]);
        // A height-only node can read constants and other height-only nodes.
        for &i in &self.column_ops {
            if self.nodes[i].dependencies == 0 {
                registers[i] = evaluate(&self.nodes[i], &registers, inputs, seed, noises);
            }
        }
        for k in 0..TABLE_LEN {
            inputs.0[1] = f64::from(TABLE_Y0 + k as i32);
            for (row, &i) in rows.iter().enumerate() {
                registers[i] = evaluate(&self.nodes[i], &registers, inputs, seed, noises);
                values[row * TABLE_LEN + k] = registers[i];
            }
        }
        let table = Arc::new(HeightTable {
            values: values.into(),
            slot: slot.into(),
        });
        tables.insert(seed, Arc::clone(&table));
        Some(table)
    }

    /// Cache everything that is constant along this vertical column.
    pub fn column(&self, inputs: Inputs) -> Evaluation<'_> {
        self.column_seeded(0, inputs)
    }

    pub fn sample<const N: usize>(&self, inputs: Inputs) -> [f64; N] {
        self.column(inputs).at(inputs.0[1])
    }

    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    /// Whether a height-dependent node steps, rounds or selects — a recipe
    /// whose value jumps along a column rather than varying smoothly.
    pub fn discontinuous_in_height(&self) -> bool {
        self.vertical_ops.iter().any(|&i| {
            matches!(
                self.nodes[i].op,
                Op::Step | Op::Trunc | Op::Round | Op::Ceil | Op::Select | Op::Equal
            )
        })
    }

    /// The lane holding node `i`'s value for lane `lane`: height-independent
    /// values no scan lays out per lane live in lane 0 only.
    #[inline]
    fn lane_of(&self, i: usize, lane: usize) -> usize {
        if self.nodes[i].y_dependent || self.broadcast[i] {
            lane
        } else {
            0
        }
    }

    fn seeded_noises(&self, seed: u32) -> Noises {
        self.noises.iter().map(|n| n.seeded(seed)).collect()
    }
}

/// Mutable scratch belongs to a query, while the compiled recipe is shared.
pub struct Evaluation<'a> {
    formula: &'a Formula,
    inputs: Inputs,
    seed: u32,
    values: [f64; MAX_NODES],
    noises: Noises,
    table: Option<Arc<HeightTable>>,
}

impl Evaluation<'_> {
    /// Move the evaluation to another column: the height-independent work is
    /// redone in place, without a new register file or noise lookup.
    pub fn rebind(&mut self, inputs: Inputs) {
        self.inputs = inputs;
        for &i in &self.formula.column_ops {
            self.values[i] = evaluate(
                &self.formula.nodes[i],
                &self.values,
                inputs,
                self.seed,
                &self.noises,
            );
        }
    }

    pub fn at<const N: usize>(&mut self, y: f64) -> [f64; N] {
        assert_eq!(N, self.formula.outputs.len());
        self.inputs.0[1] = y;
        let row = self.table.as_ref().zip(HeightTable::index(y));
        for &i in &self.formula.vertical_ops {
            if let Some((table, k)) = row {
                if let Some(values) = table.row(i) {
                    if self.formula.frontier[i] {
                        self.values[i] = values[k];
                    }
                    continue;
                }
            }
            self.values[i] = evaluate(
                &self.formula.nodes[i],
                &self.values,
                self.inputs,
                self.seed,
                &self.noises,
            );
        }
        std::array::from_fn(|i| self.values[self.formula.outputs[i]])
    }
}

fn compile(
    e: &Expression,
    nodes: &mut Vec<Node>,
    names: &BTreeMap<String, usize>,
    noises: &mut Vec<Perlin>,
    depth: usize,
) -> Result<usize, String> {
    if depth > 48 {
        return Err("formula exceeds its complexity limit".into());
    }
    let (op, args, dependent, arity) = match e {
        Expression::Number(n) if n.is_finite() => (Op::Value(*n), [0; 4], false, 0),
        Expression::Number(_) => return Err("non-finite formula literal".into()),
        Expression::Name(name) => {
            return names
                .get(name)
                .copied()
                .ok_or_else(|| format!("unknown formula name '{name}'"))
        }
        Expression::Sample(sample) => {
            sample.perlin.validate()?;
            let mut args = [0; 4];
            let mut dependent = false;
            for (i, part) in sample.at.iter().enumerate() {
                args[i] = compile(part, nodes, names, noises, depth + 1)?;
                dependent |= nodes[args[i]].y_dependent;
            }
            let noise = noises
                .iter()
                .position(|n| *n == sample.perlin)
                .unwrap_or_else(|| {
                    let index = noises.len();
                    noises.push(sample.perlin.clone());
                    index
                });
            (Op::Perlin(noise), args, dependent, 3)
        }
        Expression::Operation(parts) => {
            let Some(Expression::Name(name)) = parts.first() else {
                return Err("formula needs an operator".into());
            };
            let (op, arity) = match name.as_str() {
                "add" => (Op::Add, 2),
                "sub" => (Op::Sub, 2),
                "mul" => (Op::Mul, 2),
                "div" => (Op::Div, 2),
                "min" => (Op::Min, 2),
                "max" => (Op::Max, 2),
                "abs" => (Op::Abs, 1),
                "sqrt" => (Op::Sqrt, 1),
                "pow" => (Op::Pow, 2),
                "ceil" => (Op::Ceil, 1),
                "trunc" => (Op::Trunc, 1),
                "round" => (Op::Round, 1),
                "clamp" => (Op::Clamp, 3),
                "smooth_min" => (Op::SmoothMin, 3),
                "step" => (Op::Step, 2),
                "lt" => (Op::Less, 2),
                "le" => (Op::LessEqual, 2),
                "gt" => (Op::Greater, 2),
                "eq" => (Op::Equal, 2),
                "and" => (Op::And, 2),
                "or" => (Op::Or, 2),
                "select" => (Op::Select, 3),
                "noise2" => (Op::Noise2, 3),
                "noise3" => (Op::Noise3, 4),
                "random" => (Op::Random, 4),
                _ => return Err(format!("unknown formula operator '{name}'")),
            };
            if parts.len() != arity + 1 {
                return Err(format!("'{name}' needs {arity} arguments"));
            }
            let mut args = [0; 4];
            let mut dependent = false;
            for (i, part) in parts[1..].iter().enumerate() {
                args[i] = compile(part, nodes, names, noises, depth + 1)?;
                dependent |= nodes[args[i]].y_dependent;
            }
            (op, args, dependent, arity)
        }
    };
    if let Some(id) = nodes.iter().position(|n| n.op == op && n.args == args) {
        return Ok(id);
    }
    let dependencies = args[..arity]
        .iter()
        .fold(0, |mask, &i| mask | nodes[i].dependencies);
    let mut node = Node {
        op,
        args,
        y_dependent: dependent,
        dependencies,
        arity,
    };
    if !matches!(op, Op::Input(_) | Op::Value(_) | Op::Perlin(_) | Op::Random)
        && args[..arity]
            .iter()
            .all(|&i| matches!(nodes[i].op, Op::Value(_)))
    {
        let mut values = [0.0; MAX_NODES];
        for &i in &args[..arity] {
            if let Op::Value(n) = nodes[i].op {
                values[i] = n;
            }
        }
        node = Node {
            op: Op::Value(evaluate(&node, &values, Inputs([0.0; 10]), 0, &[])),
            args: [0; 4],
            y_dependent: false,
            dependencies: 0,
            arity: 0,
        };
    }
    if let Some(id) = nodes
        .iter()
        .position(|n| n.op == node.op && n.args == node.args)
    {
        return Ok(id);
    }
    if nodes.len() >= MAX_NODES {
        return Err("formula exceeds its complexity limit".into());
    }
    let id = nodes.len();
    nodes.push(node);
    Ok(id)
}

#[inline]
fn evaluate(
    node: &Node,
    values: &[f64],
    inputs: Inputs,
    seed: u32,
    noises: &[std::sync::Arc<crate::density::noise::ReferenceDoublePerlin>],
) -> f64 {
    match node.op {
        Op::Value(n) => n,
        Op::Input(i) => inputs.0[i],
        op => apply(op, node.args.map(|i| values[i]), seed, noises),
    }
}

/// One operator over its operand values. Every evaluation path — a point, a
/// column, a lane scan — reaches the same arithmetic through here, so they
/// cannot disagree on a cell.
#[inline]
fn apply(
    op: Op,
    [a, b, c, d]: [f64; 4],
    seed: u32,
    noises: &[std::sync::Arc<crate::density::noise::ReferenceDoublePerlin>],
) -> f64 {
    let truth = |v: bool| if v { 1.0 } else { 0.0 };
    match op {
        Op::Value(n) => n,
        Op::Input(_) => unreachable!("inputs are read by the evaluation, not applied"),
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => a * b,
        Op::Div => a / b,
        Op::Min => a.min(b),
        Op::Max => a.max(b),
        Op::Abs => a.abs(),
        Op::Sqrt => a.sqrt(),
        Op::Pow => a.powf(b),
        Op::Trunc => a.trunc(),
        Op::Ceil => a.ceil(),
        Op::Round => (a + 0.5).floor(),
        Op::Clamp => a.max(b).min(c),
        Op::SmoothMin => {
            let h = (c - (a - b).abs()).max(0.0) / c;
            a.min(b) - h * h * c * 0.25
        }
        Op::Step => ((((a * b * b).trunc() / b) + 0.5).floor() / b).clamp(0.0, 1.0),
        Op::Equal => truth(a == b),
        Op::Less => truth(a < b),
        Op::LessEqual => truth(a <= b),
        Op::Greater => truth(a > b),
        Op::And => truth(a > 0.0 && b > 0.0),
        Op::Or => truth(a > 0.0 || b > 0.0),
        Op::Select => {
            if a > 0.0 {
                b
            } else {
                c
            }
        }
        Op::Perlin(i) => noises[i].sample(a, b, c),
        Op::Noise2 => petramond_math::noise::scaled_simplex2(a as f32, b as f32, c as f32) as f64,
        Op::Noise3 => {
            petramond_math::noise::scaled_simplex3([a as f32, b as f32, c as f32], d as f32) as f64
        }
        Op::Random => {
            crate::rng::FeatureRng::positional(seed, a as u64, b as i32, c as i32, d as i32)
                .next_f32() as f64
        }
    }
}

#[cfg(test)]
mod tests;
