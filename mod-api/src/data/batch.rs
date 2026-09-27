use serde::{Deserialize, Serialize};

use crate::data::{EntityRef, MobTagValue};
use crate::ids::ConditionId;

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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MobKinematicData {
    pub mob_id: u64,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobAnimOp {
    Set {
        mob_id: u64,
        anim: String,
        active: bool,
    },
    Rate {
        mob_id: u64,
        anim: String,
        rate: f32,
    },
    Seek {
        mob_id: u64,
        anim: String,
        phase: f32,
        rate: f32,
    },
}

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
