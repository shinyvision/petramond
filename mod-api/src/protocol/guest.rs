use serde::{Deserialize, Serialize};

pub use super::host::HostCall;
use crate::client::{ClientCanvasEvent, ClientFrameData, ClientUiEvent};
use crate::data::{AiNodeCtx, AiNodeDecision, BlockHookKind, HostileSpawnCandidate};
use crate::events::{EventPayload, Outcome};
use crate::ids::BlockId;
use crate::sched::WorldgenStage;
use crate::shape::{
    BakedItemGeometry, BakedRenderCell, BakedSimCell, CellInput, PlaceInputsView,
    ShapePlacementResult,
};

pub type GenWrite = ([i32; 3], BlockId);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StructurePlacement {
    pub template: String,
    pub origin: [i32; 3],
    pub turn: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FeaturePlacement {
    pub feature: String,
    pub origins: Vec<[i32; 3]>,
    pub salt: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GenOutput {
    pub blocks: Vec<GenWrite>,
    pub structures: Vec<StructurePlacement>,
    pub features: Vec<FeaturePlacement>,
    pub deferred: bool,
}

impl GenOutput {
    pub fn deferred() -> Self {
        Self {
            deferred: true,
            ..Self::default()
        }
    }
}

impl From<Vec<GenWrite>> for GenOutput {
    fn from(blocks: Vec<GenWrite>) -> Self {
        Self {
            blocks,
            structures: Vec::new(),
            features: Vec::new(),
            deferred: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestCall {
    TickSystem {
        id: u32,
    },
    HandleEvent {
        id: u32,
        payload: EventPayload,
    },

    GenFeature {
        feature_id: u32,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u16>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },
    GenStage {
        callback_id: u32,
        stage: WorldgenStage,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u16>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },

    GuiClick {
        kind_key: String,
        widget_id: String,
        at: Option<crate::ContainerAddress>,
    },

    HostileSpawnCandidate {
        callback_id: u32,
        candidate: HostileSpawnCandidate,
    },

    BlockBehavior {
        callback_id: u32,
        kind: BlockHookKind,
        pos: [i32; 3],
    },

    /// One AI decision for one mob this tick, for the node registered via
    /// [`CoreCall::RegisterAiNode`](crate::CoreCall::RegisterAiNode).
    /// Decision-only, no sim scope. World edits, spawns and player state error here; `CurrentTick`,
    /// RNG and log work fine, and `ctx.tick` already has the tick.
    /// Return desires via [`GuestRet::AiDecision`], brain arbitration merges by priority.
    /// On ABI 2.1+ the engine batches via [`GuestCall::AiNodeBatch`] instead; this is just the
    /// fallback for older guests.
    AiNode {
        callback_id: u32,
        ctx: AiNodeCtx,
    },

    ClientFrame {
        frame: ClientFrameData,
    },
    ClientKey {
        action_id: u32,
        pressed: bool,
    },
    ClientUi {
        kind_key: String,
        event: ClientUiEvent,
    },
    ClientCanvas {
        canvas_key: String,
        event: ClientCanvasEvent,
    },
    ClientCanvasScroll {
        canvas_key: String,
        x: f32,
        y: f32,
        delta: f32,
    },

    BakeShapeSim {
        shape_kind: u16,
        cells: Vec<CellInput>,
    },
    BakeShapeRender {
        shape_kind: u16,
        cells: Vec<CellInput>,
    },
    BakeShapeItem {
        shape_kind: u16,
        block_id: BlockId,
    },
    ShapePlacementPlan {
        shape_kind: u16,
        block_id: BlockId,
        inputs: PlaceInputsView,
    },

    AiNodeBatch {
        callback_id: u32,
        ctxs: Vec<AiNodeCtx>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestRet {
    Unit,
    Event {
        outcome: Outcome,
        payload: Option<EventPayload>,
    },
    GenOutput(GenOutput),
    GenBlocks(Vec<u16>),
    GenBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    HostileSpawn(Option<String>),
    AiDecision(Option<AiNodeDecision>),

    BakedSim(Vec<BakedSimCell>),
    BakedRender(Vec<BakedRenderCell>),
    BakedItem(BakedItemGeometry),
    ShapePlacement(ShapePlacementResult),
    Unsupported,
    AiDecisions(Vec<Option<AiNodeDecision>>),
}
