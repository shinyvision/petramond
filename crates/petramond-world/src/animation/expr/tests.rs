use super::{intern, Expr};

const INPUTS: [&str; 5] = ["speed", "grounded", "sneaking", "tool", "x"];

fn names(name: &str) -> Option<u16> {
    INPUTS.iter().position(|n| *n == name).map(|i| i as u16)
}

fn eval(src: &str, vars: &[f32]) -> f32 {
    let e = Expr::compile(src, &names, 0).unwrap_or_else(|err| panic!("{src}: {err}"));
    let mut state = vec![0.0; e.state_slots()];
    e.eval(vars, &mut state, 0.016)
}

#[test]
fn precedence_logic_and_the_ternary_follow_c() {
    let v = [2.0, 1.0, 0.0, 0.0, 0.0];
    assert_eq!(eval("1 + 2 * 3", &v), 7.0);
    assert_eq!(eval("(1 + 2) * 3", &v), 9.0);
    assert_eq!(eval("-2 * -3", &v), 6.0);
    assert_eq!(eval("grounded && speed > 1.5 && !sneaking", &v), 1.0);
    assert_eq!(eval("sneaking || speed < 1", &v), 0.0);
    assert_eq!(eval("speed > 1 ? 10 : 20", &v), 10.0);
    assert_eq!(eval("speed > 5 ? 10 : sneaking ? 30 : 40", &v), 40.0);
    assert_eq!(eval("7 % 3 == 1", &v), 1.0);
    assert_eq!(
        eval("1 / 0", &v),
        0.0,
        "division by zero reads 0, never inf"
    );
}

#[test]
fn functions_compute_and_strings_compare_as_interned_ids() {
    let v = [0.0, 0.0, 0.0, intern("pickaxe"), 0.25];
    assert_eq!(eval("clamp(x * 8, 0.4, 1.1)", &v), 1.1);
    assert_eq!(eval("lerp(10, 20, x)", &v), 12.5);
    assert_eq!(eval("remap(x, 0, 0.5, 100, 200)", &v), 150.0);
    assert_eq!(eval("smoothstep(0, 1, 0.5)", &v), 0.5);
    assert_eq!(eval("tool == \"pickaxe\"", &v), 1.0);
    assert_eq!(eval("tool == \"axe\"", &v), 0.0);
}

#[test]
fn a_misspelled_input_or_a_bad_call_refuses_to_compile() {
    for bad in [
        "sped > 1",
        "clamp(1, 2)",
        "wobble(1)",
        "1 +",
        "(1",
        "speed = 1",
        "\"open",
    ] {
        assert!(Expr::compile(bad, &names, 0).is_err(), "{bad}");
    }
}

#[test]
fn stateful_call_sites_take_their_own_slots_from_the_base() {
    let e = Expr::compile(
        "smooth(x, 0.1) + spring(x, 0.1) + rise(grounded)",
        &names,
        5,
    )
    .expect("compiles");
    assert_eq!(e.state_slots(), 4);
    let mut state = vec![9.0; 9];
    e.eval(&[0.0, 1.0, 0.0, 0.0, 1.0], &mut state, 0.1);
    assert_eq!(&state[..5], &[9.0; 5], "slots below the base are untouched");
}

#[test]
fn a_spring_lands_the_same_however_the_frames_are_cut() {
    let e = Expr::compile("spring(x, 0.15)", &names, 0).expect("compiles");
    let vars = [0.0, 0.0, 0.0, 0.0, 1.0];
    let run = |frames: usize| {
        let mut s = vec![0.0; e.state_slots()];
        let mut v = 0.0;
        for _ in 0..frames {
            v = e.eval(&vars, &mut s, 0.6 / frames as f32);
        }
        v
    };
    let (coarse, fine) = (run(3), run(300));
    assert!((coarse - fine).abs() < 1e-3, "{coarse} vs {fine}");
    assert!(
        fine > 0.9 && fine <= 1.0 + 1e-4,
        "critically damped: approaches without overshoot ({fine})"
    );
}

#[test]
fn a_lightly_damped_second_order_spring_overshoots() {
    let e = Expr::compile("spring2(x, 3, 0.3)", &names, 0).expect("compiles");
    let vars = [0.0, 0.0, 0.0, 0.0, 1.0];
    let mut s = vec![0.0; e.state_slots()];
    let peak = (0..60)
        .map(|_| e.eval(&vars, &mut s, 1.0 / 60.0))
        .fold(0.0f32, f32::max);
    assert!(peak > 1.05, "peak {peak}");
}

#[test]
fn rise_fires_once_per_edge_and_since_counts_from_the_last_true() {
    let e = Expr::compile("rise(grounded)", &names, 0).expect("compiles");
    let mut s = vec![0.0; e.state_slots()];
    let g = |on: bool| [0.0, if on { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
    assert_eq!(e.eval(&g(false), &mut s, 0.1), 0.0);
    assert_eq!(e.eval(&g(true), &mut s, 0.1), 1.0);
    assert_eq!(e.eval(&g(true), &mut s, 0.1), 0.0);
    assert_eq!(e.eval(&g(false), &mut s, 0.1), 0.0);
    assert_eq!(e.eval(&g(true), &mut s, 0.1), 1.0);

    let since = Expr::compile("since(grounded)", &names, 0).expect("compiles");
    let mut s = vec![0.0; since.state_slots()];
    since.eval(&g(true), &mut s, 0.1);
    since.eval(&g(false), &mut s, 0.25);
    assert!((since.eval(&g(false), &mut s, 0.25) - 0.5).abs() < 1e-6);
}
