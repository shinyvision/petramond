//! Element types of the batched entity calls: one crossing carries every
//! intent a tick system decided for its whole population, and each element
//! means exactly what the single call with the same fields means.

use serde::{Deserialize, Serialize};

use crate::data::{EntityRef, MobTagValue};
use crate::ids::ConditionId;

/// One [`EntityCall::MobDrive`](crate::EntityCall::MobDrive) intent, for
/// [`EntityCall::MobDriveMany`](crate::EntityCall::MobDriveMany).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MobDriveData {
    pub mob_id: u64,
    pub horizontal: Option<[f32; 2]>,
    pub vertical: Option<f32>,
    pub yaw: Option<f32>,
    pub while_walking: bool,
    pub gait: bool,
}

impl MobDriveData {
    /// A plain vehicle drive: a world-space `[x, z]` velocity (m/s) and an
    /// optional absolute facing — the SDK's `mob_drive`.
    pub fn horizontal(mob_id: u64, vel: [f32; 2], yaw: Option<f32>) -> Self {
        Self {
            mob_id,
            horizontal: Some(vel),
            vertical: None,
            yaw,
            while_walking: false,
            gait: false,
        }
    }

    /// A vertical launch composed with the brain's own walking — the SDK's
    /// `mob_drive_vertical`.
    pub fn vertical(mob_id: u64, vel: f32, while_walking: bool) -> Self {
        Self {
            mob_id,
            horizontal: None,
            vertical: Some(vel),
            yaw: None,
            while_walking,
            gait: false,
        }
    }
}

/// One [`EntityCall::MobKinematic`](crate::EntityCall::MobKinematic) pose,
/// for [`EntityCall::MobKinematicMany`](crate::EntityCall::MobKinematicMany).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MobKinematicData {
    pub mob_id: u64,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// One named-animation command, for
/// [`EntityCall::MobAnimMany`](crate::EntityCall::MobAnimMany): the batched
/// form of `MobAnimSet` / `MobAnimRate` / `MobAnimSeek`, applied in order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobAnimOp {
    Set { mob_id: u64, anim: String, active: bool },
    Rate { mob_id: u64, anim: String, rate: f32 },
    Seek { mob_id: u64, anim: String, phase: f32, rate: f32 },
}

/// One tag write, for [`TagCall::MobTagsWrite`](crate::TagCall::MobTagsWrite):
/// the batched form of `MobTagSet` / `MobTagDelete`, applied in order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobTagOp {
    Set {
        mob_id: u64,
        key: String,
        value: MobTagValue,
    },
    Delete {
        mob_id: u64,
        key: String,
    },
}

/// One body-condition command, for
/// [`ConditionCall::EntityConditionsMany`](crate::ConditionCall::EntityConditionsMany):
/// the batched form of `EntityConditionApply` / `EntityConditionCool`,
/// applied in order.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum ConditionOp {
    Apply {
        entity: EntityRef,
        condition: ConditionId,
        stage: u8,
        ticks: u32,
    },
    Cool {
        entity: EntityRef,
        condition: ConditionId,
        ticks: u32,
    },
}
