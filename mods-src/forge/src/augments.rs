//! Data keys for tools/materials/socket gem, plus the `forge:augments` record on an augmented
//! stack.
//!
//! Format: `"<carved>|<id[@cond][^lvl]>,..."`, stored as instance data on a plain tool stack.
//!
//! - `<carved>`: how many lockable sockets have been carved open.
//! - one entry per socket cell, in order: augment id (a fit's canonical overlay item name),
//!   condition in quanta (missing = full, `@0` = broken), mount level (missing = Basic). A record
//!   before wear and a freshly stamped one are the same bytes. The grayed icon lands in the right
//!   cell because the record tracks cell position.
//!
//! If bytes don't re-encode faithfully, [`Record::parse`] returns `None` and we leave them alone.
//! It could be a stack from a richer pack set, and rewriting it risks corrupting it.
//!
//! Lives here because it's the contract between the anvil (writes: carve, install, repair, upgrade,
//! wear) and gold's nondestructive mining (reads it off a held stack for its gentle grant). Both
//! depend on this module, not on each other.

pub(crate) const AUGMENTS_KEY: &str = "forge:augments";

pub(crate) const LEVEL_MAX: u8 = 3;

pub(crate) fn quanta_max(lvl: u8) -> u16 {
    100 + 50 * lvl as u16
}

pub(crate) fn level_word(lvl: u8) -> (&'static str, &'static str) {
    match lvl {
        0 => ("Basic", "white"),
        1 => ("Great", "green"),
        2 => ("Epic", "purple"),
        _ => ("Legendary", "gold"),
    }
}

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

pub(crate) fn repairable(cond: u16, lvl: u8) -> bool {
    condition_word(cond, lvl).0 != "Pristine"
}

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

#[derive(Default, Clone, PartialEq, Debug)]
pub(crate) struct Record {
    pub carved: u8,
    pub entries: Vec<Entry>,
}

impl Record {
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

    pub(crate) fn set_id(&mut self, socket: usize, id: &str) {
        let e = self.entry_mut(socket);
        e.id = id.to_owned();
        e.cond = quanta_max(e.lvl);
    }
}

#[cfg(test)]
mod tests;
