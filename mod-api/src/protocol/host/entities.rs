use crate::data::EntityRef;
use crate::ids::{MobId, PlayerId};
use crate::legality::prelude::*;

host_domain! {
    EntityCall {
        /// Spawn a mob by species key at `pos` (feet), facing `yaw`. Unchecked, it spawns wherever
        /// you asked. Checked, it only spawns if the whole body fits there. That's the one to use
        /// when a player places a mob: if the call fails nothing changed, so you can hand the item
        /// back. You get the new mob's id, or `None` if the key was wrong or it didn't fit.
        /// → [`HostRet::SpawnedMob`](crate::HostRet::SpawnedMob).
        SpawnMob {
            key: String,
            pos: [f64; 3],
            yaw: f32,
            checked: bool,
        } => legal(SERVER, Sim, Write),
        MobsInRadius {
            pos: [f64; 3],
            radius: f32,
        } => legal(SERVER, Sim, Read),
        DamageMob {
            mob_id: u64,
            amount: f32,
            origin: Option<[f64; 3]>,
            feedback: Option<crate::events::MobDamageFeedback>,
            attacker: Option<EntityRef>,
        } => legal(SERVER, Sim, Write),
        DespawnMob {
            mob_id: u64,
        } => legal(SERVER, Sim, Write),
        SpawnItem {
            item: String,
            count: u8,
            pos: [f64; 3],
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        MobEmitterSet {
            mob_id: u64,
            key: String,
            active: bool,
        } => legal(SERVER, Sim, Write),
        MobMount {
            mob_id: u64,
            player_id: PlayerId,
            seat: u8,
        } => legal(SERVER, Sim, Write),
        MobDismount {
            player_id: PlayerId,
        } => legal(SERVER, Sim, Write),
        MobRiders {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        MobDrive {
            mob_id: u64,
            horizontal: Option<[f32; 2]>,
            vertical: Option<f32>,
            yaw: Option<f32>,
            while_walking: bool,
            gait: bool,
        } => legal(SERVER, Sim, Write),
        MobAnimSet {
            mob_id: u64,
            anim: String,
            active: bool,
        } => legal(SERVER, Sim, Write),
        MobAnimRate {
            mob_id: u64,
            anim: String,
            rate: f32,
        } => legal(SERVER, Sim, Write),
        MobAnimSeek {
            mob_id: u64,
            anim: String,
            phase: f32,
            rate: f32,
        } => legal(SERVER, Sim, Write),
        MobAnimState {
            mob_id: u64,
            anim: String,
        } => legal(SERVER, Sim, Read),
        MobInfo {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        MobCanReach {
            mob_id: u64,
            cell: [i32; 3],
        } => legal(SERVER, Sim, Read),
        PlayerPoseSet {
            player_id: PlayerId,
            anchor: [f64; 3],
            yaw: f32,
            pose: u8,
        } => legal(SERVER, Sim, Write),
        BlockModelGroup {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        SiteOpen {
            key: String,
            cell: [i32; 3],
        } => legal(SERVER, Sim, Read),
        LaunchItem {
            item: String,
            pos: [f64; 3],
            vel: [f32; 3],
            owner: Option<EntityRef>,
            data: Vec<(String, Vec<u8>)>,
        } => legal(SERVER, Sim, Write),
        ItemEntity {
            entity: u64,
        } => legal(SERVER, Sim, Read),
        /// Puts the live mob `mob_id` exactly where you say for this tick. With [`MobDrive`] you
        /// give a velocity and physics lands the body; here you give the landed pose, for things on
        /// a path the engine can't know about, like a cart on a rail.
        ///
        /// `pos` is the feet and `yaw` 0 faces `-Z`. Positive `pitch` is nose up and positive
        /// `roll` is right side up, both in radians. The engine doesn't move the body itself that
        /// tick, but it still blocks players, pushes soft mobs and carries riders.
        ///
        /// You have to send it every tick. When a mod stops, the body flies off with the velocity
        /// your placements implied and falls like anything else, and its tilt levels out over a few
        /// ticks. Fall bookkeeping restarts on each placement, so a dropped cart only pays for the
        /// fall it actually takes.
        ///
        /// Non-finite values, moves past the 16-block sweep bound, pitch outside `±π/2` and roll
        /// outside `±π` are refused. `false` means the mob is unknown or dead.
        ///
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        ///
        /// [`MobDrive`]: Self::MobDrive
        MobKinematic {
            mob_id: u64,
            pos: [f64; 3],
            yaw: f32,
            pitch: f32,
            roll: f32,
        } => legal(SERVER, Sim, Write),
        PathProbe {
            key: String,
            from: [i32; 3],
            to: [i32; 3],
            blocked: Vec<[i32; 3]>,
            max_nodes: u32,
        } => legal(SERVER, Sim, Read),
        Footholds {
            key: String,
            cells: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        MobHeldDisplay {
            mob_id: u64,
            main: Option<String>,
            off: Option<String>,
        } => legal(SERVER, Sim, Write),
        WalkRegion {
            key: String,
            from: [i32; 3],
            min: [i32; 3],
            max: [i32; 3],
            blocked: Vec<[i32; 3]>,
            toward: bool,
            max_nodes: u32,
        } => legal(SERVER, Sim, Read),
        SetMobDraw {
            mob_id: u64,
            frame: crate::DrawFrame,
            prims: Vec<crate::DrawPrim>,
        } => legal(SERVER, Sim, Write),
        MobDriveMany {
            drives: Vec<crate::MobDriveData>,
        } => legal(SERVER, Sim, Write),
        MobKinematicMany {
            poses: Vec<crate::MobKinematicData>,
        } => legal(SERVER, Sim, Write),
        MobAnimMany {
            ops: Vec<crate::MobAnimOp>,
        } => legal(SERVER, Sim, Write),
        MobRidersMany {
            mob_ids: Vec<u64>,
        } => legal(SERVER, Sim, Read),
        /// [`EntityCall::MobsInRadius`] for the species `kinds` only: the snapshots of the live
        /// mobs of those kinds within `radius` of `pos`.
        MobsInRadiusOf {
            pos: [f64; 3],
            radius: f32,
            kinds: Vec<MobId>,
        } => legal(SERVER, Sim, Read),
    }
}
