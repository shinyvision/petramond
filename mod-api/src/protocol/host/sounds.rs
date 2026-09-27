use crate::legality::prelude::*;

host_domain! {
    SoundCall {
        EmitSound {
            key: String,
            pos: Option<[f64; 3]>,
        } => legal(SERVER, Sim, Write),
        SoundPlayAt {
            key: String,
            pos: [f64; 3],
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
        SoundPlayOnMob {
            mob_id: u64,
            key: String,
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
        SoundStop {
            handle: u64,
        } => legal(SERVER, Sim, Write),
        EmitterBurst {
            key: String,
            pos: [f64; 3],
            intensity: f32,
            direction: Option<[f32; 3]>,
            texture: Option<crate::ParticleTexture>,
        } => legal(SERVER, Sim, Write),
        SoundSet {
            handle: u64,
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
    }
}
