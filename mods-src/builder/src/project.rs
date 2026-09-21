//! Projects: one anchored design and the job building it, kept in world KV.
//!
//! A blueprint names its project by the world's nonce and the project id, so
//! a blueprint carried into another world reads as blank there. Live projects
//! (every phase before Complete/Cancelled) are listed in a sharded index the
//! tick walks; finished ones keep their record so their blueprint still says
//! what it built.
//!
//! - [`store`] — the session's view of the records and the live index.
//! - [`note`] — what a project's note says, where logic has to tell.

pub mod note;
mod store;

use mod_sdk::*;
use serde::{Deserialize, Serialize};

pub use note::Note;
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

    /// A golem is out in the world for this phase.
    pub fn active(self) -> bool {
        matches!(
            self,
            Phase::Emerging | Phase::Working | Phase::Returning | Phase::Burrowing
        )
    }
}

/// Why an active job stopped taking new work.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Hold {
    /// Someone pressed Pause.
    Player,
    /// Supplies no longer cover the remaining work.
    Supplies,
    /// The table or its blueprint is gone.
    Table,
    /// The golem died.
    Worker,
    /// Nowhere to put what the golem carries back.
    Storage,
    /// The golem does not carry the blueprint bound to this job: without its
    /// plans it does not know what to build.
    Blueprint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "Record", into = "Record")]
pub struct Project {
    pub id: ProjectId,
    /// The player name allowed to change it (operators always may).
    pub owner: String,
    pub table: [i32; 3],
    pub asset: Option<SchematicId>,
    pub title: String,
    /// The turned design's minimum corner, once anchored.
    pub origin: Option<[i32; 3]>,
    pub turns: u8,
    /// Where the job stands. Changed only by the transitions below.
    state: State,
    /// Where the golem emerged, and where it burrows back down.
    pub home: [i32; 3],
    /// Scaffold cells the golem placed and has not taken down yet.
    pub scaffolds: Vec<[i32; 3]>,
    /// The most recent specific reason work is waiting, in the owner's
    /// words. Read what it says through [`Note`].
    pub note: String,
    /// The ghost stays up through repositioning and the build. Off, it shows
    /// only a settled draft: it steps aside while being repositioned and
    /// goes once the golem starts.
    pub show_ghost: bool,
}

/// Where a job stands. Only a job with a golem summoned for it can be held,
/// be called off mid-way or lose its golem; a draft and an ended job carry
/// none of that.
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Draft,
    Active {
        stage: Stage,
        /// A golem tagged with this project exists (loaded or not).
        golem: bool,
        hold: Option<Hold>,
        /// Winding down to Cancelled rather than Complete.
        cancelling: bool,
    },
    Ended {
        cancelled: bool,
        /// Start was pressed once: the blueprint bound to this project builds
        /// this schematic here and nothing else, however the job ended.
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

/// The stored shape of a project, kept as it has always been written.
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
            scaffolds: r.scaffolds,
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
            scaffolds: p.scaffolds,
            note: p.note,
            show_ghost: p.show_ghost,
        }
    }
}

/// What a tick reads of a project, by value: the record stays in its store
/// while the tick edits it.
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
    /// A started job that has ended can be taken up again from a table
    /// holding its blueprint: the one thing a used blueprint is still for.
    pub fn resumable(&self) -> bool {
        self.started && self.phase.finished()
    }

    /// Its schematic and position can still be chosen.
    pub fn open_to_change(&self) -> bool {
        !self.started && self.phase == Phase::Draft
    }

    /// Its golem died and no new one has been started.
    pub fn lost_worker(&self) -> bool {
        self.phase.active() && self.hold == Some(Hold::Worker) && !self.worker
    }

    /// The same project as the table at `table` would run it.
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
            note: String::new(),
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
/// What the state says, in the words the rest of the mod reads.
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

    /// Returning or burrowing ends as Cancelled rather than Complete.
    pub fn cancelling(&self) -> bool {
        matches!(
            self.state,
            State::Active {
                cancelling: true,
                ..
            }
        )
    }

    /// A golem tagged with this project exists (loaded or not).
    pub fn worker(&self) -> bool {
        matches!(self.state, State::Active { golem: true, .. })
    }

    /// Start has been pressed once: from then on the blueprint bound to this
    /// project builds this schematic here and nothing else, whether the job
    /// was cancelled, finished or never got far.
    pub fn started(&self) -> bool {
        match self.state {
            State::Draft => false,
            State::Active { .. } => true,
            State::Ended { started, .. } => started,
        }
    }
}

/// Every change of a project's state, named.
impl Project {
    /// Start pressed and a golem set down at `home`: the job is on, and from
    /// now on its blueprint is this build's for good.
    pub fn summon(&mut self, home: [i32; 3]) {
        self.state = State::Active {
            stage: Stage::Emerging,
            golem: true,
            hold: None,
            cancelling: false,
        };
        self.home = home;
        self.note.clear();
    }

    /// The golem is up. A job called off while it rose goes straight home.
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

    /// Nothing is left to build: home, with `report` for the owner.
    pub fn wind_down(&mut self, report: String) {
        if let State::Active { stage, .. } = &mut self.state {
            *stage = Stage::Returning;
        }
        self.note = report;
    }

    /// At home with nothing left to hand in: down it goes.
    pub fn burrow(&mut self) {
        if let State::Active { stage, .. } = &mut self.state {
            *stage = Stage::Burrowing;
        }
    }

    /// The golem is gone into the ground: the job ends as it was heading.
    pub fn gone_home(&mut self) {
        self.state = State::Ended {
            cancelled: self.cancelling(),
            started: self.started(),
        };
    }

    /// The golem died. A job under way waits for its owner to start another;
    /// one caught rising or sinking is work again for whoever comes next.
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
            self.note = Note::GolemDied.into();
        }
    }

    /// Called off: a golem out winds the job down (its scaffolding first,
    /// then home); with none out the project simply ends.
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

    /// Work stops for `hold`, with what to tell the owner. Only a job under
    /// way can be held.
    pub fn hold_for(&mut self, hold: Hold, note: impl Into<String>) {
        if let State::Active { hold: held, .. } = &mut self.state {
            *held = Some(hold);
            self.note = note.into();
        }
    }

    /// Whatever held the work is over.
    pub fn release(&mut self) {
        if let State::Active { hold, .. } = &mut self.state {
            *hold = None;
        }
    }
}

impl KvRecord for Project {
    const VERSION: u8 = 3;

    fn encode(&self) -> Vec<u8> {
        mod_sdk::encode(self).expect("a project record encodes")
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        mod_sdk::decode(bytes).ok()
    }
}

pub fn tag_of(id: ProjectId) -> String {
    format!("{RECORD_PREFIX}{id}")
}

/// The project a schematic choice or positioning tag names.
pub fn id_of_tag(tag: &str) -> Option<ProjectId> {
    tag.strip_prefix(RECORD_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests;
