//! The expression language animator graphs are written in: conditions,
//! weights, play rates and procedural offsets as small formulas over the
//! inputs a driver publishes.
//!
//! ```text
//! grounded && speed > 0.1 && !sneaking
//! clamp(fall_distance / 3, 0.4, 1.1)
//! main_tool == "pickaxe" ? 1.1 : 1
//! spring(look_yaw_velocity * -0.02, 0.08)
//! ```
//!
//! Numbers are `f32`; a boolean is `1` or `0` and anything non-zero is true.
//! A string literal is an interned id ([`intern`]), so a driver publishes a
//! name as `intern("pickaxe")` and a formula compares it with `"pickaxe"`.
//! Names resolve to input slots when a formula COMPILES, so a misspelled input
//! is a load error, never a silent zero. Stateful functions (`smooth`,
//! `spring`, `spring2`, `rise`, `hold`, `since`) keep per-instance state in
//! slots the compiler hands out, one run of slots per call site. `&&`, `||`
//! and `?:` evaluate every operand (no short-circuit), so a stateful call
//! inside one advances on every evaluation.

use std::sync::{LazyLock, Mutex};

use rustc_hash::FxHashMap;

/// The deepest evaluation stack a formula may need.
const STACK: usize = 16;

/// The interned id of `name` — stable for this process.
pub fn intern(name: &str) -> f32 {
    static NAMES: LazyLock<Mutex<FxHashMap<String, u32>>> =
        LazyLock::new(|| Mutex::new(FxHashMap::default()));
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let next = names.len() as u32 + 1;
    *names.entry(name.to_string()).or_insert(next) as f32
}

/// A pure function over its (up to five) arguments.
type Body = fn(&[f32; 5]) -> f32;

/// The pure functions, `(name, arity, body)`; a call op indexes this table.
const FUNCS: &[(&str, usize, Body)] = &[
    ("min", 2, |a| a[0].min(a[1])),
    ("max", 2, |a| a[0].max(a[1])),
    ("clamp", 3, |a| a[0].max(a[1]).min(a[2])),
    ("abs", 1, |a| a[0].abs()),
    ("sign", 1, |a| {
        if a[0] > 0.0 {
            1.0
        } else if a[0] < 0.0 {
            -1.0
        } else {
            0.0
        }
    }),
    ("floor", 1, |a| a[0].floor()),
    ("ceil", 1, |a| a[0].ceil()),
    ("sqrt", 1, |a| a[0].max(0.0).sqrt()),
    ("sin", 1, |a| a[0].sin()),
    ("cos", 1, |a| a[0].cos()),
    ("pow", 2, |a| a[0].powf(a[1])),
    ("exp", 1, |a| a[0].exp()),
    ("lerp", 3, |a| a[0] + (a[1] - a[0]) * a[2]),
    ("step", 2, |a| if a[1] >= a[0] { 1.0 } else { 0.0 }),
    ("smoothstep", 3, |a| {
        let span = a[1] - a[0];
        let t = if span == 0.0 {
            if a[2] >= a[1] {
                1.0
            } else {
                0.0
            }
        } else {
            ((a[2] - a[0]) / span).clamp(0.0, 1.0)
        };
        t * t * (3.0 - 2.0 * t)
    }),
    ("remap", 5, |a| {
        let span = a[2] - a[1];
        let t = if span == 0.0 {
            0.0
        } else {
            ((a[0] - a[1]) / span).clamp(0.0, 1.0)
        };
        a[3] + (a[4] - a[3]) * t
    }),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stateful {
    /// `smooth(x, halflife)`: first-order lag.
    Smooth,
    /// `spring(x, halflife)`: critically damped spring (no overshoot).
    Spring,
    /// `spring2(x, frequency, damping)`: second-order system that may overshoot.
    Spring2,
    /// `rise(x)`: 1 on the evaluation `x` turns true, else 0.
    Rise,
    /// `hold(x, seconds)`: 1 while `x` is true and for `seconds` after.
    Hold,
    /// `since(x)`: seconds since `x` was last true.
    Since,
}

impl Stateful {
    fn slots(self) -> usize {
        match self {
            Stateful::Smooth | Stateful::Rise | Stateful::Hold | Stateful::Since => 1,
            Stateful::Spring | Stateful::Spring2 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Const(f32),
    Var(u16),
    Neg,
    Not,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Select,
    /// An index into [`FUNCS`].
    Call(u8),
    State(Stateful, u16),
}

/// A compiled formula.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    ops: Box<[Op]>,
    state: usize,
}

impl Expr {
    /// Compile `source`, resolving every name through `resolve` to an input
    /// slot. Stateful call sites take slots from `state_base` upward; the
    /// caller reserves [`state_slots`](Self::state_slots) of them.
    pub fn compile(
        source: &str,
        resolve: &dyn Fn(&str) -> Option<u16>,
        state_base: usize,
    ) -> Result<Expr, String> {
        let tokens = tokenize(source)?;
        let mut parser = Parser {
            tokens,
            at: 0,
            ops: Vec::new(),
            resolve,
            state: state_base,
        };
        parser.ternary()?;
        if parser.at != parser.tokens.len() {
            return Err(format!("`{source}`: unexpected {:?}", parser.tokens[parser.at]));
        }
        let expr = Expr {
            ops: parser.ops.into_boxed_slice(),
            state: parser.state - state_base,
        };
        if expr.depth() > STACK {
            return Err(format!("`{source}`: too deeply nested"));
        }
        Ok(expr)
    }

    /// A constant formula.
    pub fn constant(value: f32) -> Expr {
        Expr {
            ops: Box::new([Op::Const(value)]),
            state: 0,
        }
    }

    /// State slots this formula's stateful call sites occupy.
    pub fn state_slots(&self) -> usize {
        self.state
    }

    /// The constant this formula always answers, if it has no inputs or state.
    pub fn as_constant(&self) -> Option<f32> {
        match *self.ops {
            [Op::Const(v)] => Some(v),
            _ => None,
        }
    }

    fn depth(&self) -> usize {
        let mut depth = 0isize;
        let mut max = 0isize;
        for op in self.ops.iter() {
            depth += match op {
                Op::Const(_) | Op::Var(_) => 1,
                Op::Neg | Op::Not => 0,
                Op::Select => -2,
                Op::Call(f) => 1 - FUNCS[*f as usize].1 as isize,
                Op::State(s, _) => 1 - stateful_arity(*s) as isize,
                _ => -1,
            };
            max = max.max(depth);
        }
        max as usize
    }

    /// Evaluate over input values `vars`, advancing stateful call sites by
    /// `dt` seconds in `state`, which holds at least the caller's reserved
    /// [`state_slots`](Self::state_slots) past the compile-time base; a call
    /// site past its end answers 0 (and asserts in debug builds).
    pub fn eval(&self, vars: &[f32], state: &mut [f32], dt: f32) -> f32 {
        let mut stack = [0.0f32; STACK];
        let mut sp = 0usize;
        macro_rules! pop {
            () => {{
                sp -= 1;
                stack[sp]
            }};
        }
        let truth = |v: f32| v != 0.0;
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        for op in self.ops.iter() {
            let value = match *op {
                Op::Const(v) => v,
                Op::Var(slot) => vars.get(slot as usize).copied().unwrap_or(0.0),
                Op::Neg => -pop!(),
                Op::Not => flag(!truth(pop!())),
                Op::Select => {
                    let b = pop!();
                    let a = pop!();
                    if truth(pop!()) {
                        a
                    } else {
                        b
                    }
                }
                Op::Call(f) => {
                    let (_, n, body) = FUNCS[f as usize];
                    let mut args = [0.0f32; 5];
                    for i in (0..n).rev() {
                        args[i] = pop!();
                    }
                    body(&args)
                }
                Op::State(s, slot) => {
                    let mut args = [0.0f32; 3];
                    let n = stateful_arity(s);
                    for i in (0..n).rev() {
                        args[i] = pop!();
                    }
                    let slot = slot as usize;
                    match state.get_mut(slot..slot + s.slots()) {
                        Some(state) => advance(s, &args, state, dt),
                        None => {
                            debug_assert!(false, "state slot {slot} past the reserved {}", state.len());
                            0.0
                        }
                    }
                }
                binary => {
                    let b = pop!();
                    let a = pop!();
                    match binary {
                        Op::Add => a + b,
                        Op::Sub => a - b,
                        Op::Mul => a * b,
                        Op::Div => {
                            if b == 0.0 {
                                0.0
                            } else {
                                a / b
                            }
                        }
                        Op::Mod => {
                            if b == 0.0 {
                                0.0
                            } else {
                                a.rem_euclid(b)
                            }
                        }
                        Op::Lt => flag(a < b),
                        Op::Le => flag(a <= b),
                        Op::Gt => flag(a > b),
                        Op::Ge => flag(a >= b),
                        Op::Eq => flag(a == b),
                        Op::Ne => flag(a != b),
                        Op::And => flag(truth(a) && truth(b)),
                        Op::Or => flag(truth(a) || truth(b)),
                        _ => unreachable!("every unary op is matched above"),
                    }
                }
            };
            stack[sp] = value;
            sp += 1;
        }
        if sp == 0 {
            0.0
        } else {
            let v = stack[sp - 1];
            if v.is_finite() {
                v
            } else {
                0.0
            }
        }
    }
}

fn stateful_arity(s: Stateful) -> usize {
    match s {
        Stateful::Rise | Stateful::Since => 1,
        Stateful::Smooth | Stateful::Spring | Stateful::Hold => 2,
        Stateful::Spring2 => 3,
    }
}

/// One stateful call site's step. The springs integrate EXACTLY for a
/// constant target over `dt`, so a frame hitch or a split frame cannot change
/// where they end up.
fn advance(s: Stateful, a: &[f32; 3], state: &mut [f32], dt: f32) -> f32 {
    let dt = dt.max(0.0);
    match s {
        Stateful::Smooth => {
            let halflife = a[1].max(1e-4);
            let k = 1.0 - (-std::f32::consts::LN_2 * dt / halflife).exp();
            state[0] += (a[0] - state[0]) * k;
            state[0]
        }
        Stateful::Spring => {
            let halflife = a[1].max(1e-4);
            let y = (4.0 * std::f32::consts::LN_2) / halflife / 2.0;
            let j0 = state[0] - a[0];
            let j1 = state[1] + j0 * y;
            let e = (-y * dt).exp();
            state[0] = e * (j0 + j1 * dt) + a[0];
            state[1] = e * (state[1] - j1 * y * dt);
            state[0]
        }
        Stateful::Spring2 => {
            // x'' = w² (target - x) - 2ζw x', integrated in fixed substeps
            // small enough to stay stable at any frame rate.
            let w = std::f32::consts::TAU * a[1].max(0.0);
            let zeta = a[2].max(0.0);
            let steps = (dt / (1.0 / 240.0)).ceil().max(1.0) as usize;
            let h = dt / steps as f32;
            for _ in 0..steps {
                let accel = w * w * (a[0] - state[0]) - 2.0 * zeta * w * state[1];
                state[1] += accel * h;
                state[0] += state[1] * h;
            }
            state[0]
        }
        Stateful::Rise => {
            let now = a[0] != 0.0;
            let was = state[0] != 0.0;
            state[0] = if now { 1.0 } else { 0.0 };
            if now && !was {
                1.0
            } else {
                0.0
            }
        }
        Stateful::Hold => {
            if a[0] != 0.0 {
                state[0] = a[1].max(0.0);
                1.0
            } else {
                state[0] = (state[0] - dt).max(0.0);
                if state[0] > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
        Stateful::Since => {
            if a[0] != 0.0 {
                state[0] = 0.0;
            } else {
                state[0] += dt;
            }
            state[0]
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f32),
    Str(String),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
    Question,
    Colon,
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    const OPS: [&str; 16] = [
        "&&", "||", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "!", "=", "&",
    ];
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let text = &src[start..i];
            out.push(Token::Num(
                text.parse().map_err(|_| format!("`{src}`: bad number `{text}`"))?,
            ));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            out.push(Token::Name(src[start..i].to_string()));
        } else if c == '"' {
            let start = i + 1;
            let end = src[start..]
                .find('"')
                .ok_or_else(|| format!("`{src}`: unterminated string"))?;
            out.push(Token::Str(src[start..start + end].to_string()));
            i = start + end + 1;
        } else if c == '(' {
            out.push(Token::Open);
            i += 1;
        } else if c == ')' {
            out.push(Token::Close);
            i += 1;
        } else if c == ',' {
            out.push(Token::Comma);
            i += 1;
        } else if c == '?' {
            out.push(Token::Question);
            i += 1;
        } else if c == ':' {
            out.push(Token::Colon);
            i += 1;
        } else if let Some(op) = OPS.iter().find(|op| src[i..].starts_with(**op)) {
            if matches!(*op, "=" | "&") {
                return Err(format!("`{src}`: `{op}` is not an operator (use `==` / `&&`)"));
            }
            out.push(Token::Op(op));
            i += op.len();
        } else {
            return Err(format!("`{src}`: unexpected `{c}`"));
        }
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    ops: Vec<Op>,
    resolve: &'a dyn Fn(&str) -> Option<u16>,
    state: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: Token) -> Result<(), String> {
        if self.eat(&token) {
            Ok(())
        } else {
            Err(format!("expected {token:?}, found {:?}", self.peek()))
        }
    }

    fn ternary(&mut self) -> Result<(), String> {
        self.binary(0)?;
        if self.eat(&Token::Question) {
            self.ternary()?;
            self.expect(Token::Colon)?;
            self.ternary()?;
            self.ops.push(Op::Select);
        }
        Ok(())
    }

    fn binary(&mut self, min_level: u8) -> Result<(), String> {
        self.unary()?;
        loop {
            let Some(Token::Op(op)) = self.peek().cloned() else {
                return Ok(());
            };
            let Some((level, code)) = binary_op(op) else {
                return Ok(());
            };
            if level < min_level {
                return Ok(());
            }
            self.at += 1;
            self.binary(level + 1)?;
            self.ops.push(code);
        }
    }

    fn unary(&mut self) -> Result<(), String> {
        if self.eat(&Token::Op("-")) {
            self.unary()?;
            self.ops.push(Op::Neg);
            return Ok(());
        }
        if self.eat(&Token::Op("!")) {
            self.unary()?;
            self.ops.push(Op::Not);
            return Ok(());
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<(), String> {
        let token = self
            .tokens
            .get(self.at)
            .cloned()
            .ok_or("unexpected end of formula")?;
        self.at += 1;
        match token {
            Token::Num(v) => self.ops.push(Op::Const(v)),
            Token::Str(s) => self.ops.push(Op::Const(intern(&s))),
            Token::Open => {
                self.ternary()?;
                self.expect(Token::Close)?;
            }
            Token::Name(name) if self.peek() == Some(&Token::Open) => {
                self.at += 1;
                let mut args = 0;
                if !self.eat(&Token::Close) {
                    loop {
                        self.ternary()?;
                        args += 1;
                        if self.eat(&Token::Close) {
                            break;
                        }
                        self.expect(Token::Comma)?;
                    }
                }
                self.call(&name, args)?;
            }
            Token::Name(name) => match name.as_str() {
                "true" => self.ops.push(Op::Const(1.0)),
                "false" => self.ops.push(Op::Const(0.0)),
                _ => {
                    let slot = (self.resolve)(&name).ok_or_else(|| format!("unknown input `{name}`"))?;
                    self.ops.push(Op::Var(slot));
                }
            },
            other => return Err(format!("unexpected {other:?}")),
        }
        Ok(())
    }

    fn call(&mut self, name: &str, args: usize) -> Result<(), String> {
        if let Some((index, (_, arity, _))) = FUNCS.iter().enumerate().find(|(_, f)| f.0 == name) {
            if args != *arity {
                return Err(format!("`{name}` takes {arity} arguments, got {args}"));
            }
            self.ops.push(Op::Call(index as u8));
            return Ok(());
        }
        let stateful = match name {
            "smooth" => Stateful::Smooth,
            "spring" => Stateful::Spring,
            "spring2" => Stateful::Spring2,
            "rise" => Stateful::Rise,
            "hold" => Stateful::Hold,
            "since" => Stateful::Since,
            _ => return Err(format!("unknown function `{name}`")),
        };
        if args != stateful_arity(stateful) {
            return Err(format!(
                "`{name}` takes {} arguments, got {args}",
                stateful_arity(stateful)
            ));
        }
        let slot = u16::try_from(self.state).map_err(|_| "too many stateful call sites")?;
        self.ops.push(Op::State(stateful, slot));
        self.state += stateful.slots();
        Ok(())
    }
}

fn binary_op(op: &str) -> Option<(u8, Op)> {
    Some(match op {
        "||" => (1, Op::Or),
        "&&" => (2, Op::And),
        "==" => (3, Op::Eq),
        "!=" => (3, Op::Ne),
        "<" => (4, Op::Lt),
        "<=" => (4, Op::Le),
        ">" => (4, Op::Gt),
        ">=" => (4, Op::Ge),
        "+" => (5, Op::Add),
        "-" => (5, Op::Sub),
        "*" => (6, Op::Mul),
        "/" => (6, Op::Div),
        "%" => (6, Op::Mod),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
