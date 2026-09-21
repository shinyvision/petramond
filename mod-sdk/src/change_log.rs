//! Following the world's block change log from one tick to the next.

use crate::block_changes_since;

/// A mod's place in the world's block change log.
///
/// Call [`ChangeCursor::advance`] once per tick and hand the result to
/// whatever keeps derived facts about cells (a survey, a cache): it names
/// every cell that changed since the previous call. The first call, and any
/// call after the log slid past this cursor, comes back `lost` — treat every
/// cell as changed.
///
/// The log's numbering belongs to the session: keep a cursor in memory, never
/// in a save.
#[derive(Default)]
pub struct ChangeCursor {
    since: Option<u64>,
}

/// What changed since the cursor last advanced.
#[derive(Default, Debug)]
pub struct CellChanges {
    /// The cells changed, oldest first; one may appear more than once.
    pub cells: Vec<[i32; 3]>,
    /// Some changes are unknown: every cell may have changed.
    pub lost: bool,
}

impl ChangeCursor {
    pub fn advance(&mut self) -> CellChanges {
        let reply = block_changes_since(self.since);
        let lost = reply.lost || self.since.is_none();
        self.since = Some(reply.next);
        CellChanges {
            cells: reply.cells,
            lost,
        }
    }
}
