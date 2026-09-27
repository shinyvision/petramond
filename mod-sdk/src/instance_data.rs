pub const TOOL_OVERRIDE_KEY: &str = "petramond:tool";

pub const OVERLAY_DATA_KEY: &str = "petramond:overlay";

pub fn tool_override_json(tier: u8, speed: f32, damage: [f32; 2], knockback: f32) -> String {
    format!(
        "{{\"tier\":{tier},\"speed\":{speed:.4},\"damage\":[{:.4},{:.4}],\"knockback\":{knockback:.4}}}",
        damage[0], damage[1]
    )
}
