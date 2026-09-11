//! Conservative interval arithmetic over the recipe operators: a bound on
//! what a node can produce over a column's heights, used to skip members a
//! scan would otherwise evaluate lane by lane. Every rule must contain the
//! true value; where no rule is known the result is unknown and the member
//! is kept.

use super::Op;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub const UNKNOWN: Self = Self {
        lo: f64::NEG_INFINITY,
        hi: f64::INFINITY,
    };
    pub const EMPTY: Self = Self {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
    };
    const TRUTH: Self = Self { lo: 0.0, hi: 1.0 };
    const FALSE: Self = Self { lo: 0.0, hi: 0.0 };
    const TRUE: Self = Self { lo: 1.0, hi: 1.0 };

    pub fn point(v: f64) -> Self {
        if v.is_nan() {
            Self::UNKNOWN
        } else {
            Self { lo: v, hi: v }
        }
    }

    fn new(lo: f64, hi: f64) -> Self {
        if lo.is_nan() || hi.is_nan() {
            Self::UNKNOWN
        } else {
            Self { lo, hi }
        }
    }

    pub fn hull_point(self, v: f64) -> Self {
        if v.is_nan() {
            return Self::UNKNOWN;
        }
        Self::new(self.lo.min(v), self.hi.max(v))
    }

    fn hull(self, other: Self) -> Self {
        Self::new(self.lo.min(other.lo), self.hi.max(other.hi))
    }

    fn unknown(self) -> bool {
        self.lo.is_infinite() || self.hi.is_infinite()
    }

    /// A predicate value that can never be `> 0.0`.
    pub fn never_positive(self) -> bool {
        self.hi <= 0.0
    }

    fn is_point(self) -> bool {
        self.lo == self.hi
    }

    fn monotone(self, f: impl Fn(f64) -> f64) -> Self {
        Self::new(f(self.lo), f(self.hi))
    }
}

fn truth(v: bool) -> Interval {
    if v {
        Interval::TRUE
    } else {
        Interval::FALSE
    }
}

fn mul(a: Interval, b: Interval) -> Interval {
    let products = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    if products.iter().any(|p| p.is_nan()) {
        return Interval::UNKNOWN;
    }
    products
        .iter()
        .fold(Interval::EMPTY, |range, &p| range.hull_point(p))
}

/// `a * a`: never negative whatever the sign of `a`, which a product of two
/// independent ranges cannot know.
pub(super) fn square(a: Interval) -> Interval {
    if a.unknown() {
        return Interval::new(0.0, f64::INFINITY);
    }
    let product = mul(a, a);
    if a.lo <= 0.0 && a.hi >= 0.0 {
        Interval::new(0.0, product.hi)
    } else {
        product
    }
}

pub(super) fn apply(op: Op, [a, b, c, _]: [Interval; 4]) -> Interval {
    use Interval as I;
    match op {
        Op::Value(n) => I::point(n),
        Op::Input(_) => I::UNKNOWN,
        Op::Add => I::new(a.lo + b.lo, a.hi + b.hi),
        Op::Sub => I::new(a.lo - b.hi, a.hi - b.lo),
        Op::Mul => mul(a, b),
        Op::Div => {
            if (b.lo <= 0.0 && b.hi >= 0.0) || b.unknown() {
                I::UNKNOWN
            } else {
                mul(a, I::new(1.0 / b.hi, 1.0 / b.lo))
            }
        }
        Op::Min => I::new(a.lo.min(b.lo), a.hi.min(b.hi)),
        Op::Max => I::new(a.lo.max(b.lo), a.hi.max(b.hi)),
        Op::Abs => {
            if a.lo >= 0.0 {
                a
            } else if a.hi <= 0.0 {
                I::new(-a.hi, -a.lo)
            } else {
                I::new(0.0, (-a.lo).max(a.hi))
            }
        }
        Op::Sqrt => {
            if a.lo < 0.0 {
                I::UNKNOWN
            } else {
                a.monotone(f64::sqrt)
            }
        }
        Op::Pow => {
            if a.is_point() && b.is_point() {
                I::point(a.lo.powf(b.lo))
            } else {
                I::UNKNOWN
            }
        }
        Op::Trunc => a.monotone(f64::trunc),
        Op::Ceil => a.monotone(f64::ceil),
        Op::Round => a.monotone(|x| (x + 0.5).floor()),
        Op::Clamp => {
            let low = I::new(a.lo.max(b.lo), a.hi.max(b.hi));
            I::new(low.lo.min(c.lo), low.hi.min(c.hi))
        }
        Op::SmoothMin => {
            if c.lo <= 0.0 || c.unknown() {
                I::UNKNOWN
            } else {
                I::new(a.lo.min(b.lo) - c.hi * 0.25, a.hi.min(b.hi))
            }
        }
        Op::Step => {
            if a.is_point() && b.is_point() {
                I::point(super::apply(op, [a.lo, b.lo, 0.0, 0.0], 0, &[]))
            } else {
                I::TRUTH
            }
        }
        Op::Equal => {
            if a.is_point() && b.is_point() {
                truth(a.lo == b.lo)
            } else if a.hi < b.lo || b.hi < a.lo {
                I::FALSE
            } else {
                I::TRUTH
            }
        }
        Op::Less => {
            if a.hi < b.lo {
                I::TRUE
            } else if a.lo >= b.hi {
                I::FALSE
            } else {
                I::TRUTH
            }
        }
        Op::LessEqual => {
            if a.hi <= b.lo {
                I::TRUE
            } else if a.lo > b.hi {
                I::FALSE
            } else {
                I::TRUTH
            }
        }
        Op::Greater => {
            if a.lo > b.hi {
                I::TRUE
            } else if a.hi <= b.lo {
                I::FALSE
            } else {
                I::TRUTH
            }
        }
        Op::And => {
            if a.hi <= 0.0 || b.hi <= 0.0 {
                I::FALSE
            } else if a.lo > 0.0 && b.lo > 0.0 {
                I::TRUE
            } else {
                I::TRUTH
            }
        }
        Op::Or => {
            if a.lo > 0.0 || b.lo > 0.0 {
                I::TRUE
            } else if a.hi <= 0.0 && b.hi <= 0.0 {
                I::FALSE
            } else {
                I::TRUTH
            }
        }
        Op::Select => {
            if a.lo > 0.0 {
                b
            } else if a.hi <= 0.0 {
                c
            } else {
                b.hull(c)
            }
        }
        // Simplex noise stays within one either way and a seeded uniform draw
        // within [0, 1); a Perlin stack's bound is not derived here.
        Op::Noise2 | Op::Noise3 => I::new(-1.0, 1.0),
        Op::Random => I::new(0.0, 1.0),
        Op::Perlin(_) => I::UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rule must contain the operator's value at every point of its
    /// operand intervals, edges included; a violated bound would skip a
    /// member that carves.
    #[test]
    fn every_rule_contains_the_pointwise_values() {
        let samples = [-3.5, -1.0, -0.25, 0.0, 0.5, 1.0, 2.75];
        let ops = [
            Op::Add,
            Op::Sub,
            Op::Mul,
            Op::Div,
            Op::Min,
            Op::Max,
            Op::Abs,
            Op::Sqrt,
            Op::Pow,
            Op::Trunc,
            Op::Ceil,
            Op::Round,
            Op::Clamp,
            Op::SmoothMin,
            Op::Step,
            Op::Equal,
            Op::Less,
            Op::LessEqual,
            Op::Greater,
            Op::And,
            Op::Or,
            Op::Select,
        ];
        for op in ops {
            for (&a0, &a1) in samples.iter().zip(samples.iter().skip(1)) {
                for (&b0, &b1) in samples.iter().zip(samples.iter().skip(2)) {
                    for &c0 in &samples {
                        let (a, b, c) = (
                            Interval::new(a0, a1),
                            Interval::new(b0, b1),
                            Interval::new(c0, c0 + 1.0),
                        );
                        let range = apply(op, [a, b, c, Interval::point(0.0)]);
                        for &x in &[a0, (a0 + a1) * 0.5, a1] {
                            for &y in &[b0, (b0 + b1) * 0.5, b1] {
                                for &z in &[c0, c0 + 0.5, c0 + 1.0] {
                                    let v = super::super::apply(op, [x, y, z, 0.0], 0, &[]);
                                    if v.is_nan() {
                                        continue;
                                    }
                                    assert!(
                                        range.lo <= v && v <= range.hi,
                                        "{op:?} at ({x},{y},{z}) = {v} outside {range:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
