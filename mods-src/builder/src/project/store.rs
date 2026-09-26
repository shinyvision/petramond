//! The session's view of the project records: read once, written only when
//! an edit changed them, and the index of the live ones. Each record's
//! scaffold list is joined to it on load and stored beside it (see
//! [`super::scaffolds`]).

use crate::host::prelude::*;

use crate::content::PROJECT_DATA;
use crate::fx::HashMap;
use crate::project::{scaffolds, Project, ProjectId, RECORD_PREFIX};

const NONCE_KEY: &str = "builder:nonce";
const NEXT_KEY: &str = "builder:next_project";
const INDEX_PREFIX: &str = "builder:live";
/// Per table, the newest project that finished there: the table's report,
/// found again after the record has left memory.
const REPORT_PREFIX: &str = "builder:report";

fn report_key(table: [i32; 3]) -> String {
    format!("{REPORT_PREFIX}/{}/{}/{}", table[0], table[1], table[2])
}

fn read_id(bytes: Vec<u8>) -> Option<ProjectId> {
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

pub struct Projects {
    nonce: u64,
    records: RecordStore<Project>,
    live: IdShards,
    /// Per loaded project, its scaffold list as last stored: what an edit is
    /// compared against, and the mark that the list was joined to the record.
    stored_scaffolds: HashMap<ProjectId, Vec<[i32; 3]>>,
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
            stored_scaffolds: HashMap::default(),
        }
    }

    /// Every project before Complete/Cancelled, oldest first.
    pub fn live(&self) -> impl Iterator<Item = ProjectId> + '_ {
        self.live.iter()
    }

    pub fn get(&mut self, id: ProjectId) -> Option<&Project> {
        match self.records.get(id) {
            Ok(Some(_)) => {}
            // A listed project with no record is no project.
            Ok(None) => {
                self.live.set(id, false);
                return None;
            }
            // An unreadable record (say, from a newer build) stays listed:
            // the world keeps it for a build that can read it.
            Err(_) => return None,
        }
        self.join_scaffolds(id);
        self.records.peek(id)
    }

    /// Give a record this session just read its scaffold list, once. A
    /// project whose scaffolds were never stored beside it adopts the list
    /// its (version 3) record carried inline, and stores it.
    fn join_scaffolds(&mut self, id: ProjectId) {
        if self.stored_scaffolds.contains_key(&id) {
            return;
        }
        let Some(inline) = self.records.peek(id).map(|p| p.scaffolds.clone()) else {
            return;
        };
        let cells = scaffolds::load(id).unwrap_or_else(|| {
            scaffolds::save(id, &[], &inline, true);
            inline
        });
        self.records.update(id, |p| p.scaffolds = cells.clone());
        self.stored_scaffolds.insert(id, cells);
    }

    /// Store project `id`'s scaffold list if an edit changed it.
    fn store_scaffolds(&mut self, id: ProjectId) {
        let (Some(project), Some(stored)) =
            (self.records.peek(id), self.stored_scaffolds.get_mut(&id))
        else {
            return;
        };
        if project.scaffolds != *stored {
            scaffolds::save(id, stored, &project.scaffolds, false);
            stored.clone_from(&project.scaffolds);
        }
    }

    /// A project this session already holds, without reading the world.
    pub fn peek(&self, id: ProjectId) -> Option<&Project> {
        self.records.peek(id)
    }

    pub fn update<R>(&mut self, id: ProjectId, f: impl FnOnce(&mut Project) -> R) -> Option<R> {
        if !matches!(self.records.get(id), Ok(Some(_))) {
            return None;
        }
        self.join_scaffolds(id);
        let ((out, finished, table), changed) = self.records.update(id, |p| {
            let out = f(p);
            (out, p.phase().finished(), p.table)
        })?;
        if changed {
            self.live.set(id, !finished);
            if finished {
                file_report(table, id);
            }
        }
        self.store_scaffolds(id);
        Some(out)
    }

    pub fn create(&mut self, owner: String, table: [i32; 3]) -> ProjectId {
        let id = world_kv_get(NEXT_KEY)
            .and_then(|b| Some(u64::from_le_bytes(b.try_into().ok()?)))
            .unwrap_or(1);
        world_kv_set(NEXT_KEY, (id + 1).to_le_bytes().to_vec());
        self.records.insert(id, Project::new(id, owner, table));
        self.stored_scaffolds.insert(id, Vec::new());
        self.live.set(id, true);
        id
    }

    /// The newest project at `table` with a golem out. Found by position,
    /// not slot: a golem carries its blueprint away. Only live projects can
    /// be active, and the tick reads every live record, so the live index
    /// over the session's records is the whole answer.
    pub fn active_at(&self, table: [i32; 3]) -> Option<ProjectId> {
        self.live
            .iter()
            .filter(|id| {
                self.records
                    .peek(*id)
                    .is_some_and(|p| p.table == table && p.phase().active())
            })
            .max()
    }

    /// The newest finished project at `table` — its report. The filed
    /// pointer finds it after a sweep or a reload; a loaded record finished
    /// before reports were filed still counts.
    pub fn report_at(&self, table: [i32; 3]) -> Option<ProjectId> {
        let filed = world_kv_get(&report_key(table)).and_then(read_id);
        let loaded = self
            .records
            .loaded()
            .filter(|(_, p)| p.table == table && p.phase().finished())
            .map(|(id, _)| id)
            .max();
        filed.max(loaded)
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
        let records = &self.records;
        self.stored_scaffolds
            .retain(|id, _| records.peek(*id).is_some());
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

/// File `id` as `table`'s report unless a newer project already is.
fn file_report(table: [i32; 3], id: ProjectId) {
    let key = report_key(table);
    if world_kv_get(&key).and_then(read_id).is_some_and(|filed| filed >= id) {
        return;
    }
    world_kv_set(&key, id.to_le_bytes().to_vec());
}
