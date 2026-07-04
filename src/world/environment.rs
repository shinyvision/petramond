//! Sim-owned visual shader parameters.
//!
//! Neutral data only — no render types. Mods write this on the tick through
//! HostCalls; `game::environment` snapshots it per frame for the renderer.
//! NOT persisted: it resets to defaults every time a world opens — the owning
//! mod re-applies it (its persistence is the Phase 3 world KV).

use std::collections::BTreeMap;
use std::sync::Arc;

pub type ShaderParamMap = BTreeMap<String, [f32; 4]>;

/// The world's presentation-environment state.
#[derive(Clone, Debug)]
pub struct WorldEnvironment {
    /// Named visual shader parameters. Shader packs map names onto fixed GPU
    /// slots; mods write only their own namespace through the host API.
    shader_params: Arc<ShaderParamMap>,
}

impl Default for WorldEnvironment {
    fn default() -> Self {
        Self {
            shader_params: Arc::new(BTreeMap::new()),
        }
    }
}

impl WorldEnvironment {
    #[inline]
    pub fn shader_params(&self) -> &Arc<ShaderParamMap> {
        &self.shader_params
    }

    pub fn set_shader_param(&mut self, key: String, value: [f32; 4]) {
        if !value.iter().all(|v| v.is_finite()) {
            return;
        }
        let mut next = (*self.shader_params).clone();
        next.insert(key, value);
        self.shader_params = Arc::new(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_params_snapshot_and_reject_non_finite_values() {
        let mut env = WorldEnvironment::default();
        env.set_shader_param("daynight:sky".into(), [0.5, 0.25, 0.0, 1.0]);
        let first = env.shader_params().clone();
        assert_eq!(first["daynight:sky"], [0.5, 0.25, 0.0, 1.0]);

        env.set_shader_param("daynight:sky".into(), [f32::NAN, 0.0, 0.0, 0.0]);
        assert!(Arc::ptr_eq(&first, env.shader_params()));

        env.set_shader_param("daynight:sky".into(), [0.75, 0.0, 0.0, 0.0]);
        assert!(!Arc::ptr_eq(&first, env.shader_params()));
        assert_eq!(env.shader_params()["daynight:sky"][0], 0.75);
    }
}
