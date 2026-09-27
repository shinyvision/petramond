use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    BodyCall {
        PlayerState => legal(SERVER_CLIENT, Sim, Read),
        SetPlayerHeldPose {
            player: PlayerId,
            main: Option<crate::HeldPose>,
            off: Option<crate::HeldPose>,
        } => legal(SERVER_CLIENT, Sim, Write),
        SetPlayerBonePose {
            player: PlayerId,
            bones: Vec<crate::BonePoseData>,
        } => legal(SERVER_CLIENT, Sim, Write),
        HoldUse {
            player: PlayerId,
        } => legal(SERVER_CLIENT, Sim, Write),
        SetPlayerHeldDisplay {
            player: PlayerId,
            main: Option<String>,
            off: Option<String>,
        } => legal(SERVER_CLIENT, Sim, Write),
        PlayerInventory {
            player: PlayerId,
        } => legal(SERVER_CLIENT, Sim, Read),
        SetPlayerAnimatorParams {
            player: PlayerId,
            params: Vec<crate::AnimatorParam>,
        } => legal(SERVER_CLIENT, Sim, Write),
        SetPlayerAnimatorPlays {
            player: PlayerId,
            plays: Vec<crate::AnimatorPlay>,
        } => legal(SERVER_CLIENT, Sim, Write),
        FirePlayerAnimatorEvent {
            player: PlayerId,
            rig: String,
            event: String,
        } => legal(SERVER_CLIENT, Sim, Write),
        AnimationClip {
            rig: String,
            clip: String,
        } => legal(Sides::SERVER_CLIENT.union(Sides::SHELL), Any, Read),
        ActingPlayer => legal(SERVER_CLIENT, Sim, Read),
    }
}
