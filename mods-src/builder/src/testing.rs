//! What the builder's tests share: a fake world with a floor to build on, a
//! session over it, and projects anchored, compiled and staffed in it.

use std::rc::Rc;

use crate::content::{Content, BLUEPRINT, PROJECT_DATA};
use crate::design::Progress;
use crate::geometry::cell_of;
use crate::host::fake::{rows, Fake, Installed};
use crate::host::prelude::*;
use crate::jobs::Builder;
use crate::project::{Note, ProjectId};
use crate::supplies::Shortfall;
use crate::survey::Survey;
use crate::worker::{Body, Job, PROJECT_TAG};

/// The player every test project belongs to.
pub const OWNER: &str = "ada";
/// The schematic every test project builds.
pub const ASSET: SchematicId = [7; 32];
/// The floor's top layer: a body on it stands in y = 0.
pub const FLOOR: i32 = -1;

/// The note of a job short of `count` stone and nothing else.
pub fn short_of_stone(count: u32) -> Note {
    Note::Missing(Shortfall {
        count,
        name: "Stone".into(),
        more: false,
    })
}

/// The row site ([`Session::row`]): three stones to lay along x from the
/// origin, the table three cells south of them with a chest beside it, and
/// the open ground a golem rises out of beside the table.
pub const ROW: [([i32; 3], &str); 3] = [
    ([0, 0, 0], "petramond:stone"),
    ([1, 0, 0], "petramond:stone"),
    ([2, 0, 0], "petramond:stone"),
];
pub const TABLE_AT: [i32; 3] = [0, 0, -3];
pub const CHEST_AT: [i32; 3] = [1, 0, -3];
pub const HOME: [i32; 3] = [-1, 0, -4];

/// A session of the mod over a fake world, installed on the test's thread.
pub struct Session {
    pub world: Rc<Fake>,
    pub builder: Builder,
    _installed: Installed,
}

impl Session {
    /// A stone floor reaching `half` cells from the origin each way.
    pub fn flat(half: i32) -> Self {
        let world = Rc::new(Fake::new());
        world.fill([-half, FLOOR, -half], [half, FLOOR, half], rows::STONE);
        Self::over(world)
    }

    /// The mod starting up in `world`.
    pub fn over(world: Rc<Fake>) -> Self {
        let installed = world.install();
        let content = Content::resolve().expect("the fake world holds the pack's rows");
        Self {
            builder: Builder::new(content),
            world,
            _installed: installed,
        }
    }

    /// The row site on a floor reaching ten cells out: the design stored a
    /// section a cell, its blueprint in the table, the chest empty.
    pub fn row() -> (Self, ProjectId) {
        Self::site("Row", [3, 1, 1], &ROW)
    }

    /// The row site's table, chest and floor with another design anchored at
    /// the origin: `cells` in a box of `size`.
    pub fn site(title: &str, size: [i32; 3], cells: &[([i32; 3], &str)]) -> (Self, ProjectId) {
        let mut session = Self::flat(10);
        session.world.schematic(ASSET, title, size, cells, 1);
        let id = session.draft(TABLE_AT, [0, 0, 0]);
        session.world.chest(CHEST_AT, 4);
        let blueprint = session.blueprint(id);
        session
            .world
            .put(ContainerAddress::Block(TABLE_AT), 0, Some(blueprint));
        (session, id)
    }

    pub fn now(&self) -> u64 {
        self.world.state().now
    }

    /// A draft at a table standing at `table`, anchored on [`ASSET`] at
    /// `origin`, its blueprint slot empty.
    pub fn draft(&mut self, table: [i32; 3], origin: [i32; 3]) -> ProjectId {
        self.world.set(table, rows::TABLE);
        self.world
            .state_mut()
            .containers
            .insert(ContainerAddress::Block(table), vec![None]);
        let id = self.builder.projects.create(OWNER.into(), table);
        self.builder.projects.update(id, |p| {
            p.asset = Some(ASSET);
            p.origin = Some(origin);
        });
        id
    }

    /// The blueprint bound to project `id` in this world.
    pub fn blueprint(&self, id: ProjectId) -> ItemStackData {
        ItemStackData {
            item: BLUEPRINT.into(),
            count: 1,
            data: vec![(PROJECT_DATA.into(), self.builder.projects.binding(id))],
        }
    }

    /// The job for project `id`: its design compiled and every unit measured
    /// against the world.
    pub fn job(&mut self, id: ProjectId) -> &mut Job {
        let now = self.now();
        let brief = self.builder.projects.get(id).expect("a project").brief();
        let job = self
            .builder
            .jobs
            .attend(&brief, now)
            .expect("an anchored project");
        assert!(
            matches!(job.design.compile(1 << 16), Progress::Ready),
            "the design compiles"
        );
        let mut survey = Survey::new(&job.design);
        survey.step(&job.design, usize::MAX, now, 0, &[], true);
        job.survey = Some(survey);
        job
    }

    /// A golem out working project `id` (its job attended) from `home`,
    /// standing in `at` with the blueprint in its first slot.
    pub fn golem(&mut self, id: ProjectId, home: [i32; 3], at: [i32; 3]) -> u64 {
        self.builder.projects.update(id, |p| {
            p.summon(home);
            p.emerged();
        });
        let golem = self.world.golem_at(at);
        self.world
            .state_mut()
            .mobs
            .get_mut(&golem)
            .expect("the golem just stood up")
            .tags
            .insert(PROJECT_TAG.into(), MobTagValue::I64(id as i64));
        let blueprint = self.blueprint(id);
        self.world
            .put(ContainerAddress::Mob(golem), 0, Some(blueprint));
        let job = self.builder.jobs.map.get_mut(&id).expect("the job is attended");
        job.crew.mob = Some(golem);
        job.crew.last_mob = Some(golem);
        golem
    }

    /// The golem's body as a tick reads it.
    pub fn body(&self, golem: u64) -> Body {
        let state = self.world.state();
        let mob = &state.mobs[&golem];
        Body {
            id: golem,
            pos: mob.pos,
            cell: cell_of(mob.pos),
            on_ground: mob.on_ground,
            yaw: mob.yaw,
            slots: state
                .containers
                .get(&ContainerAddress::Mob(golem))
                .cloned()
                .unwrap_or_default(),
        }
    }
}
