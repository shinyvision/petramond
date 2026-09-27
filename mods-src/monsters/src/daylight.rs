const TRANSITION: f32 = 0.04;

pub fn daylight(t: f32) -> f32 {
    let h = (core::f32::consts::PI * TRANSITION).sin();
    smoothstep(-h, h, (core::f32::consts::TAU * t).sin())
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
