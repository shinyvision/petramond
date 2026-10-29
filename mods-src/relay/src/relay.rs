//! The relay's rules, apart from the `Mod` glue so tests can drive them against a host.

use mod_sdk::*;

/// The largest stack any item holds; a row that asks for more still works (the engine clamps a
/// take to what the slot holds) but is almost certainly a mistake.
const STACK_LIMIT: u8 = 64;
/// How many `interval`s a relay that moved nothing waits when its row has no `idle_interval`.
const IDLE_FACTOR: u64 = 8;

#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    /// Ticks between two moves.
    pub interval: u64,
    /// Most items one move carries.
    pub stack: u8,
    /// Ticks to wait after a step that moved nothing (no container above, empty, target full
    /// or picky). Defaults to `interval` times [`IDLE_FACTOR`], so a relay with nothing to do
    /// costs one host call per long wait.
    #[serde(default)]
    pub idle_interval: Option<u64>,
}

impl Transfer {
    /// Why the row cannot run, or a warning when it runs but looks wrong.
    fn check(&self) -> Result<Option<String>, String> {
        if self.interval == 0 || self.stack == 0 || self.idle_interval == Some(0) {
            return Err("interval, stack and idle_interval must be above 0".into());
        }
        Ok((self.stack > STACK_LIMIT).then(|| {
            format!(
                "stack {} is above the largest item stack ({STACK_LIMIT}); a move is capped by the item",
                self.stack
            )
        }))
    }

    fn idle(&self) -> u64 {
        self.idle_interval
            .unwrap_or_else(|| self.interval.saturating_mul(IDLE_FACTOR))
    }
}

/// The slot of the first non-empty stack, with how many items of it one move takes.
pub fn next_move(slots: &[Option<ItemStackData>], stack: u8) -> Option<(u32, u8)> {
    slots.iter().enumerate().find_map(|(slot, s)| {
        let s = s.as_ref().filter(|s| s.count > 0)?;
        Some((slot as u32, s.count.min(stack)))
    })
}

#[derive(Default)]
pub struct Relay {
    rows: Vec<(BlockId, Transfer)>,
}

impl Relay {
    /// Keeps the usable rows; each rejected row is logged by block name.
    pub fn with_rows(rows: Vec<(BlockId, Transfer)>) -> Self {
        let names = block_names(rows.iter().map(|(block, _)| *block).collect());
        let mut kept = Vec::with_capacity(rows.len());
        for (i, (block, transfer)) in rows.into_iter().enumerate() {
            let name = names
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| format!("{block:?}"));
            match transfer.check() {
                Err(why) => log(&format!("relay: {name}: {why}; row skipped")),
                Ok(warning) => {
                    if let Some(warning) = warning {
                        log(&format!("relay: {name}: {warning}"));
                    }
                    kept.push((block, transfer));
                }
            }
        }
        Self { rows: kept }
    }

    pub fn is_idle(&self) -> bool {
        self.rows.is_empty()
    }

    fn row(&self, block: BlockId) -> Option<Transfer> {
        self.rows.iter().find(|(b, _)| *b == block).map(|(_, t)| *t)
    }

    /// A relay block was placed: start its timer.
    pub fn placed(&self, pos: [i32; 3], block: BlockId) {
        if let Some(transfer) = self.row(block) {
            schedule_tick(pos, transfer.interval);
        }
    }

    pub fn hooks(&self, hooks: &[(BlockHookKind, [i32; 3])]) {
        let cells = get_blocks(hooks.iter().map(|(_, pos)| *pos).collect());
        for ((kind, pos), block) in hooks.iter().zip(cells) {
            let Some(transfer) = block.and_then(|b| self.row(b)) else {
                continue;
            };
            match kind {
                BlockHookKind::ScheduledTick => step(*pos, transfer),
                BlockHookKind::RandomTick => schedule_tick(*pos, transfer.interval),
                _ => {}
            }
        }
    }
}

/// Reads only the source and lets `container_transfer` decide about the target: the engine
/// creates a never-opened container there and refuses a cell that cannot hold one. Always
/// schedules the next step, soon after a move and after the idle wait otherwise.
fn step(pos: [i32; 3], transfer: Transfer) {
    let [x, y, z] = pos;
    let (above, below) = ([x, y + 1, z], [x, y - 1, z]);
    let moved = container_get(above.into())
        .and_then(|source| next_move(&source, transfer.stack))
        .and_then(|(slot, count)| container_transfer(above.into(), slot, below.into(), count));
    let wait = if moved.is_some() {
        transfer.interval
    } else {
        transfer.idle()
    };
    schedule_tick(pos, wait);
}
