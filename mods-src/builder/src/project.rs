pub mod note;
mod scaffolds;
mod store;

use crate::host::prelude::*;
use serde::{Deserialize, Serialize};

pub use note::Note;
#[cfg(test)]
pub use note::Report;
pub use store::Projects;

pub type ProjectId = u64;

const RECORD_PREFIX: &str = "builder:project/";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Phase {
    Draft,
    Emerging,
    Working,
    Returning,
    Burrowing,
    Complete,
    Cancelled,
}

impl Phase {
    pub fn finished(self) -> bool {
        matches!(self, Phase::Complete | Phase::Cancelled)
    }

    pub fn active(self) -> bool {
        matches!(
            self,
            Phase::Emerging | Phase::Working | Phase::Returning | Phase::Burrowing
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Hold {
    Player,
    Supplies,
    Table,
    Worker,
    Storage,
    Blueprint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "Record", into = "Record")]
pub struct Project {
    pub id: ProjectId,
    pub owner: String,
    pub table: [i32; 3],
    pub asset: Option<SchematicId>,
    pub title: String,
    pub origin: Option<[i32; 3]>,
    pub turns: u8,
    state: State,
    pub home: [i32; 3],
    pub scaffolds: Vec<[i32; 3]>,
    pub note: Note,
    pub show_ghost: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Draft,
    Active {
        stage: Stage,
        golem: bool,
        hold: Option<Hold>,
        cancelling: bool,
    },
    Ended {
        cancelled: bool,
        started: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Emerging,
    Working,
    Returning,
    Burrowing,
}

#[derive(Serialize, Deserialize)]
struct Record {
    id: ProjectId,
    owner: String,
    table: [i32; 3],
    asset: Option<SchematicId>,
    title: String,
    origin: Option<[i32; 3]>,
    turns: u8,
    phase: Phase,
    hold: Option<Hold>,
    cancelling: bool,
    home: [i32; 3],
    worker: bool,
    legacy_scaffolds: Vec<[i32; 3]>,
    note: Note,
    started: bool,
    show_ghost: bool,
}

#[derive(Serialize, Deserialize)]
struct RecordV3 {
    id: ProjectId,
    owner: String,
    table: [i32; 3],
    asset: Option<SchematicId>,
    title: String,
    origin: Option<[i32; 3]>,
    turns: u8,
    phase: Phase,
    hold: Option<Hold>,
    cancelling: bool,
    home: [i32; 3],
    worker: bool,
    scaffolds: Vec<[i32; 3]>,
    note: String,
    started: bool,
    show_ghost: bool,
}

impl From<Record> for Project {
    fn from(r: Record) -> Self {
        let stage = match r.phase {
            Phase::Emerging => Some(Stage::Emerging),
            Phase::Working => Some(Stage::Working),
            Phase::Returning => Some(Stage::Returning),
            Phase::Burrowing => Some(Stage::Burrowing),
            Phase::Draft | Phase::Complete | Phase::Cancelled => None,
        };
        let state = match (stage, r.phase) {
            (Some(stage), _) => State::Active {
                stage,
                golem: r.worker,
                hold: r.hold,
                cancelling: r.cancelling,
            },
            (None, Phase::Draft) => State::Draft,
            (None, phase) => State::Ended {
                cancelled: phase == Phase::Cancelled,
                started: r.started,
            },
        };
        Self {
            id: r.id,
            owner: r.owner,
            table: r.table,
            asset: r.asset,
            title: r.title,
            origin: r.origin,
            turns: r.turns,
            state,
            home: r.home,
            scaffolds: r.legacy_scaffolds,
            note: r.note,
            show_ghost: r.show_ghost,
        }
    }
}

impl From<Project> for Record {
    fn from(p: Project) -> Self {
        Self {
            phase: p.phase(),
            hold: p.hold(),
            cancelling: p.cancelling(),
            worker: p.worker(),
            started: p.started(),
            id: p.id,
            owner: p.owner,
            table: p.table,
            asset: p.asset,
            title: p.title,
            origin: p.origin,
            turns: p.turns,
            home: p.home,
            legacy_scaffolds: Vec::new(),
            note: p.note,
            show_ghost: p.show_ghost,
        }
    }
}

impl From<RecordV3> for Record {
    fn from(r: RecordV3) -> Self {
        Self {
            id: r.id,
            owner: r.owner,
            table: r.table,
            asset: r.asset,
            title: r.title,
            origin: r.origin,
            turns: r.turns,
            phase: r.phase,
            hold: r.hold,
            cancelling: r.cancelling,
            home: r.home,
            worker: r.worker,
            legacy_scaffolds: r.scaffolds,
            note: Note::from_legacy(&r.note),
            started: r.started,
            show_ghost: r.show_ghost,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brief {
    pub id: ProjectId,
    pub table: [i32; 3],
    pub home: [i32; 3],
    pub phase: Phase,
    pub hold: Option<Hold>,
    pub cancelling: bool,
    pub worker: bool,
    pub started: bool,
    pub show_ghost: bool,
    pub asset: Option<SchematicId>,
    pub anchored: Option<(SchematicId, [i32; 3], u8)>,
}

impl Brief {
    pub fn resumable(&self) -> bool {
        self.started && self.phase.finished()
    }

    pub fn open_to_change(&self) -> bool {
        !self.started && self.phase == Phase::Draft
    }

    pub fn lost_worker(&self) -> bool {
        self.phase.active() && self.hold == Some(Hold::Worker) && !self.worker
    }

    pub fn at(mut self, table: [i32; 3]) -> Self {
        self.table = table;
        self
    }
}

impl Project {
    fn new(id: ProjectId, owner: String, table: [i32; 3]) -> Self {
        Self {
            id,
            owner,
            table,
            asset: None,
            title: String::new(),
            origin: None,
            turns: 0,
            state: State::Draft,
            home: table,
            scaffolds: Vec::new(),
            note: Note::None,
            show_ghost: true,
        }
    }

    pub fn brief(&self) -> Brief {
        Brief {
            id: self.id,
            table: self.table,
            home: self.home,
            phase: self.phase(),
            hold: self.hold(),
            cancelling: self.cancelling(),
            worker: self.worker(),
            started: self.started(),
            show_ghost: self.show_ghost,
            asset: self.asset,
            anchored: self.anchored(),
        }
    }

    pub fn tag(&self) -> String {
        tag_of(self.id)
    }

    pub fn resumable(&self) -> bool {
        self.brief().resumable()
    }

    pub fn anchored(&self) -> Option<(SchematicId, [i32; 3], u8)> {
        Some((self.asset?, self.origin?, self.turns))
    }
}
impl Project {
    pub fn phase(&self) -> Phase {
        match self.state {
            State::Draft => Phase::Draft,
            State::Active { stage, .. } => match stage {
                Stage::Emerging => Phase::Emerging,
                Stage::Working => Phase::Working,
                Stage::Returning => Phase::Returning,
                Stage::Burrowing => Phase::Burrowing,
            },
            State::Ended {
                cancelled: true, ..
            } => Phase::Cancelled,
            State::Ended { .. } => Phase::Complete,
        }
    }

    pub fn hold(&self) -> Option<Hold> {
        match self.state {
            State::Active { hold, .. } => hold,
            _ => None,
        }
    }

    pub fn cancelling(&self) -> bool {
        matches!(
            self.state,
            State::Active {
                cancelling: true,
                ..
            }
        )
    }

    pub fn worker(&self) -> bool {
        matches!(self.state, State::Active { golem: true, .. })
    }

    pub fn started(&self) -> bool {
        match self.state {
            State::Draft => false,
            State::Active { .. } => true,
            State::Ended { started, .. } => started,
        }
    }
}

impl Project {
    pub fn summon(&mut self, home: [i32; 3]) {
        self.state = State::Active {
            stage: Stage::Emerging,
            golem: true,
            hold: None,
            cancelling: false,
        };
        self.home = home;
        self.note = Note::None;
    }

    pub fn emerged(&mut self) {
        if let State::Active {
            stage, cancelling, ..
        } = &mut self.state
        {
            *stage = if *cancelling {
                Stage::Returning
            } else {
                Stage::Working
            };
        }
    }

    pub fn wind_down(&mut self, report: Note) {
        if let State::Active { stage, .. } = &mut self.state {
            *stage = Stage::Returning;
        }
        self.note = report;
    }

    pub fn burrow(&mut self) {
        if let State::Active { stage, .. } = &mut self.state {
            *stage = Stage::Burrowing;
        }
    }

    pub fn gone_home(&mut self) {
        self.state = State::Ended {
            cancelled: self.cancelling(),
            started: self.started(),
        };
    }

    pub fn golem_died(&mut self) {
        if let State::Active {
            stage, golem, hold, ..
        } = &mut self.state
        {
            *golem = false;
            *hold = Some(Hold::Worker);
            if matches!(stage, Stage::Emerging | Stage::Burrowing) {
                *stage = Stage::Working;
            }
            self.note = Note::GolemDied;
        }
    }

    pub fn cancel(&mut self) {
        match &mut self.state {
            State::Active {
                stage,
                golem: true,
                hold,
                cancelling,
            } => {
                *hold = None;
                *cancelling = true;
                if *stage == Stage::Working {
                    *stage = Stage::Returning;
                }
            }
            _ => {
                self.state = State::Ended {
                    cancelled: true,
                    started: self.started(),
                }
            }
        }
    }

    pub fn hold_for(&mut self, hold: Hold, note: Note) {
        if let State::Active { hold: held, .. } = &mut self.state {
            *held = Some(hold);
            self.note = note;
        }
    }

    pub fn release(&mut self) {
        if let State::Active { hold, .. } = &mut self.state {
            *hold = None;
        }
    }
}

impl KvRecord for Project {
    const VERSION: u8 = 4;
    const OLDEST_VERSION: u8 = 3;

    fn encode(&self) -> Vec<u8> {
        mod_sdk::encode(self).expect("a project record encodes")
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        mod_sdk::decode(bytes).ok()
    }

    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        match from {
            3 => {
                let old: RecordV3 = mod_sdk::decode(bytes).ok()?;
                mod_sdk::encode(&Record::from(old)).ok()
            }
            _ => None,
        }
    }
}

pub fn tag_of(id: ProjectId) -> String {
    format!("{RECORD_PREFIX}{id}")
}

pub fn id_of_tag(tag: &str) -> Option<ProjectId> {
    tag.strip_prefix(RECORD_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests;
