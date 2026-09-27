use crate::block_changes_since;

#[derive(Default)]
pub struct ChangeCursor {
    since: Option<u64>,
}

#[derive(Default, Debug)]
pub struct CellChanges {
    pub cells: Vec<[i32; 3]>,
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
