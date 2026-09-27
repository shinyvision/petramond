pub const WALK: &str = "walk";
pub const SNEAK: &str = "sneak";
pub const HURT: &str = "hurt";
pub const AMBIENT: &str = "ambient";
pub const IDLE_PREFIX: &str = "idle_";

pub fn is_idle(name: &str) -> bool {
    name.starts_with(IDLE_PREFIX)
}
