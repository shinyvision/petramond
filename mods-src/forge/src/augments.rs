//! The augment VOCABULARY: the data keys tools, materials and the socket gem
//! carry, and the `forge:augments` RECORD that rides an augmented stack.
//!
//! THE RECORD IS `"<carved>|<id[@cond][^lvl]>,…"`, one instance-data value on
//! an ordinary tool stack:
//!
//! - `<carved>` — how many LOCKABLE sockets this tool has had carved open.
//! - then one entry per socket cell, POSITIONALLY: the installed augment's
//!   IDENTITY (a fit's canonical overlay item name), its CONDITION in quanta
//!   (omitted = full for the level, `@0` = broken) and the socket's mount
//!   upgrade LEVEL (omitted = Basic). A pre-wear record and a fresh stamp are
//!   byte-identical, and the panel's grayed icon sits in the exact cell the
//!   material went into because the record says which cell that was.
//!
//! Bytes this build cannot re-encode faithfully REFUSE ([`Record::parse`]
//! answers `None`): a decoration or format we would rewrite wrong is a stack
//! from a richer pack set, and taking no further changes is the only answer
//! that cannot corrupt it.
//!
//! It all lives here because it is the CONTRACT between two peer policies
//! that must not know about each other: the anvil WRITES the record (carve,
//! install, repair, upgrade, wear) while gold's nondestructive mining READS
//! it off a held stack to find its gentle grant. Both depend on this module;
//! neither depends on the other.

/// This pack's own record of the carved sockets and installed augments (see
/// the module docs and [`Record`]).
pub(crate) const AUGMENTS_KEY: &str = "forge:augments";

/// The row-data key an augment MATERIAL carries: its list of fits (tool kind,
/// edge tier, multipliers, cost, overlay art, optional behaviour grant).
pub(crate) const AUGMENT_KEY: &str = "forge:augment";

/// The data key that marks an item as a SOCKET material (the petramond gem):
/// consumed at the anvil to carve a locked socket open or raise a mount
/// level. Membership is the whole vocabulary — the value is not read.
pub(crate) const SOCKET_ITEM_KEY: &str = "forge:socket_key";

/// Row-data key marking an item as an innately gentle miner. Gold's policy
/// declares the behaviour; the anvil refuses fitting a `gentle` augment to a
/// tool that already has it (no gold-on-gold).
pub(crate) const NONDESTRUCTIVE_KEY: &str = "forge:nondestructive";

/// A socket's mount can be upgraded this many times past Basic.
pub(crate) const LEVEL_MAX: u8 = 3;

/// Condition is stored in QUANTA: one quantum is 1% of the fit's BASE
/// maximum, so the stored value space stays tiny (0..=250) no matter how
/// large a fit's advertised maximum is — every distinct stored value mints
/// an interned variant, and the intern table never evicts, so per-point
/// storage of a 3000-point pool would exhaust it on a long-lived server.
/// Full condition for a socket at level `lvl`; upgrades add 50% of base.
pub(crate) fn quanta_max(lvl: u8) -> u16 {
    100 + 50 * lvl as u16
}

/// The mount level's display word and its tooltip palette color.
pub(crate) fn level_word(lvl: u8) -> (&'static str, &'static str) {
    match lvl {
        0 => ("Basic", "white"),
        1 => ("Great", "green"),
        2 => ("Epic", "purple"),
        _ => ("Legendary", "gold"),
    }
}

/// The condition's display word and its tooltip palette color: Broken at
/// exactly zero (the augment stops contributing), then even quintiles of
/// the CURRENT (level-scaled) maximum.
pub(crate) fn condition_word(cond: u16, lvl: u8) -> (&'static str, &'static str) {
    if cond == 0 {
        return ("Broken", "red");
    }
    let pct = cond as u32 * 100 / quanta_max(lvl) as u32;
    match pct {
        81.. => ("Pristine", "green"),
        61..=80 => ("Excellent", "green"),
        41..=60 => ("Good", "yellow"),
        21..=40 => ("Worn", "yellow"),
        _ => ("Damaged", "red"),
    }
}

/// Repair is REFUSED while the condition still reads Pristine — topping up
/// the top band would waste most of a material's quanta; the gesture opens
/// at Excellent or lower (Rachel, 2026-08-10). Derived from the same
/// banding as the word the player sees.
pub(crate) fn repairable(cond: u16, lvl: u8) -> bool {
    condition_word(cond, lvl).0 != "Pristine"
}

/// One socket's slice of the record: the installed augment identity (empty
/// = open and empty), its CONDITION in quanta, and the socket's mount
/// upgrade level.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Entry {
    pub id: String,
    pub cond: u16,
    pub lvl: u8,
}

impl Entry {
    fn empty() -> Entry {
        Entry {
            id: String::new(),
            cond: 0,
            lvl: 0,
        }
    }

    /// `id[@cond][^lvl]` — omitted cond = full for the level, omitted lvl =
    /// Basic. A plain identity (every pre-wear record) parses as pristine.
    fn parse(s: &str) -> Option<Entry> {
        let (rest, lvl) = match s.split_once('^') {
            Some((r, l)) => (r, l.trim().parse::<u8>().ok()?),
            None => (s, 0),
        };
        let (id, cond) = match rest.split_once('@') {
            Some((i, c)) => (i, Some(c.trim().parse::<u16>().ok()?)),
            None => (rest, None),
        };
        if lvl > LEVEL_MAX {
            return None;
        }
        let id = id.trim();
        if id.is_empty() && cond.is_some() {
            return None;
        }
        let cond = match cond {
            Some(c) if c > quanta_max(lvl) => return None,
            Some(c) => c,
            None if id.is_empty() => 0,
            None => quanta_max(lvl),
        };
        Some(Entry {
            id: id.to_owned(),
            cond,
            lvl,
        })
    }

    fn encode(&self) -> String {
        if self.id.is_empty() {
            return match self.lvl {
                0 => String::new(),
                l => format!("^{l}"),
            };
        }
        let mut s = self.id.clone();
        if self.cond != quanta_max(self.lvl) {
            s.push('@');
            s.push_str(&self.cond.to_string());
        }
        if self.lvl != 0 {
            s.push('^');
            s.push_str(&self.lvl.to_string());
        }
        s
    }
}

/// The parsed `forge:augments` record: how many lockable sockets have been
/// carved, plus one [`Entry`] per socket cell (the list may be shorter than
/// the machine's cell count).
#[derive(Default, Clone, PartialEq, Debug)]
pub(crate) struct Record {
    pub carved: u8,
    pub entries: Vec<Entry>,
}

impl Record {
    /// Parse the record value. `None` for bytes this build cannot reason
    /// about (not UTF-8, a foreign/older format without the `|`, or entry
    /// decorations we cannot re-encode faithfully): such a record takes no
    /// further changes.
    pub fn parse(bytes: &[u8]) -> Option<Record> {
        let s = std::str::from_utf8(bytes).ok()?;
        let (carved, ids) = s.split_once('|')?;
        let carved = carved.trim().parse().ok()?;
        let entries = if ids.is_empty() {
            Vec::new()
        } else {
            ids.split(',').map(Entry::parse).collect::<Option<_>>()?
        };
        Some(Record { carved, entries })
    }

    /// The record on a stack's instance data — absent key = a fresh tool.
    pub fn of_stack(data: &[(String, Vec<u8>)]) -> Option<Record> {
        match data.iter().find(|(k, _)| k == AUGMENTS_KEY) {
            None => Some(Record::default()),
            Some((_, v)) => Record::parse(v),
        }
    }

    pub fn encode(&self) -> String {
        let encoded: Vec<String> = self.entries.iter().map(Entry::encode).collect();
        let last = encoded.iter().rposition(|e| !e.is_empty());
        let ids = match last {
            None => String::new(),
            Some(l) => encoded[..=l].join(","),
        };
        format!("{}|{}", self.carved, ids)
    }

    /// The installed identities, in socket order — broken augments INCLUDED:
    /// a broken fang still occupies its socket (repair it, never re-stage
    /// a second one) and still draws its art.
    pub fn installed(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|e| !e.id.is_empty())
            .map(|e| e.id.as_str())
    }

    pub(crate) fn entry_at(&self, socket: usize) -> Option<&Entry> {
        self.entries.get(socket).filter(|e| !e.id.is_empty())
    }

    pub(crate) fn id_at(&self, socket: usize) -> Option<&str> {
        self.entry_at(socket).map(|e| e.id.as_str())
    }

    pub(crate) fn entry_mut(&mut self, socket: usize) -> &mut Entry {
        if self.entries.len() <= socket {
            self.entries.resize(socket + 1, Entry::empty());
        }
        &mut self.entries[socket]
    }

    /// Install `id` at `socket`, pristine for the socket's mount level.
    pub(crate) fn set_id(&mut self, socket: usize, id: &str) {
        let e = self.entry_mut(socket);
        e.id = id.to_owned();
        e.cond = quanta_max(e.lvl);
    }
}

#[cfg(test)]
mod tests;
