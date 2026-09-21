//! The session's view of the project records: read once, written only when
//! an edit changed them, and the index of the live ones.

use mod_sdk::*;

use crate::content::PROJECT_DATA;
use crate::fx::HashMap;
use crate::project::{Phase, Project, ProjectId, RECORD_PREFIX};

const NONCE_KEY: &str = "builder:nonce";
const NEXT_KEY: &str = "builder:next_project";
const INDEX_PREFIX: &str = "builder:live";

pub struct Projects {
    nonce: u64,
    records: RecordStore<Project>,
    live: IdShards,
}

impl Projects {
    pub fn load() -> Self {
        let nonce = world_kv_get(NONCE_KEY)
            .and_then(|b| Some(u64::from_le_bytes(b.try_into().ok()?)))
            .unwrap_or_else(|| {
                let nonce = rng_u64("builder:nonce") ^ current_tick().rotate_left(29);
                world_kv_set(NONCE_KEY, nonce.to_le_bytes().to_vec());
                nonce
            });
        Self {
            nonce,
            records: RecordStore::new(RECORD_PREFIX),
            live: IdShards::load(INDEX_PREFIX),
        }
    }

    /// Every project before Complete/Cancelled, oldest first.
    pub fn live(&self) -> impl Iterator<Item = ProjectId> + '_ {
        self.live.iter()
    }

    pub fn get(&mut self, id: ProjectId) -> Option<&Project> {
        if self.records.get(id).is_none() {
            // A listed project whose record cannot be read is no project.
            self.live.set(id, false);
            return None;
        }
        self.records.peek(id)
    }

    /// A project this session already holds, without reading the world.
    pub fn peek(&self, id: ProjectId) -> Option<&Project> {
        self.records.peek(id)
    }

    pub fn update<R>(&mut self, id: ProjectId, f: impl FnOnce(&mut Project) -> R) -> Option<R> {
        let ((out, finished), changed) = self.records.update(id, |p| {
            let out = f(p);
            (out, p.phase().finished())
        })?;
        if changed {
            self.live.set(id, !finished);
        }
        Some(out)
    }

    pub fn create(&mut self, owner: String, table: [i32; 3]) -> ProjectId {
        let id = world_kv_get(NEXT_KEY)
            .and_then(|b| Some(u64::from_le_bytes(b.try_into().ok()?)))
            .unwrap_or(1);
        world_kv_set(NEXT_KEY, (id + 1).to_le_bytes().to_vec());
        self.records.insert(id, Project::new(id, owner, table));
        self.live.set(id, true);
        id
    }

    /// The newest started, loaded project at `table` that `wanted` accepts.
    /// Found by position, not slot: a golem carries its blueprint away.
    pub fn at_table(
        &self,
        table: [i32; 3],
        wanted: impl Fn(&Project) -> bool,
    ) -> Option<ProjectId> {
        self.records
            .loaded()
            .filter(|(_, p)| p.table == table && p.phase() != Phase::Draft && wanted(p))
            .map(|(id, _)| id)
            .max()
    }

    /// Let go of finished projects nobody has asked about since the last
    /// sweep. The newest at each table stays: it is that table's report.
    pub fn sweep(&mut self) {
        let mut reports: HashMap<[i32; 3], ProjectId> = HashMap::default();
        for (id, project) in self.records.loaded() {
            if project.phase().finished() {
                let newest = reports.entry(project.table).or_insert(id);
                *newest = (*newest).max(id);
            }
        }
        self.records
            .sweep(|id, p| !p.phase().finished() || reports.get(&p.table) == Some(&id));
    }

    /// The project a blueprint stack is bound to in this world.
    pub fn bound(&self, stack: &ItemStackData) -> Option<ProjectId> {
        let (_, bytes) = stack.data.iter().find(|(k, _)| k == PROJECT_DATA)?;
        let bytes: &[u8; 16] = bytes.as_slice().try_into().ok()?;
        let nonce = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        (nonce == self.nonce).then(|| u64::from_le_bytes(bytes[8..].try_into().unwrap()))
    }

    pub fn binding(&self, id: ProjectId) -> Vec<u8> {
        let mut bytes = self.nonce.to_le_bytes().to_vec();
        bytes.extend(id.to_le_bytes());
        bytes
    }
}
