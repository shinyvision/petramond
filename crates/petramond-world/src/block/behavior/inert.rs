//! The do-nothing behaviour shared by every ordinary block (all trait defaults).

use super::BlockBehavior;

/// A block with no reactive behaviour. Zero-sized and stateless, so one shared
/// instance ([`INERT`]) serves every ordinary block.
pub struct Inert;

impl BlockBehavior for Inert {
    fn key(&self) -> &'static str {
        "inert"
    }
}

/// The shared inert singleton a row points at (`behavior: &behavior::INERT`) when
/// the block does nothing on its own — the overwhelming majority of blocks.
pub static INERT: Inert = Inert;
