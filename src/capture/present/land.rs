//! The frame-side half of a presentation: landing a prepared apply,
//! releasing frames as the position comes to them, keeping the window
//! resident, reading ahead, and following changes to the files it reads.

use std::collections::BTreeSet;
use std::sync::Arc;

use mod_api::capture::ClientStateKey;
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::super::fold::{KeyChange, Need};
use super::super::source::FileChange;
use super::super::unpack::{Frame, FrameItem};
use super::super::window::{Away, AwayColumn};
use super::{DriveOut, Prepared, Presentation};
use crate::world::{
    decode_section_payload, Cached, DetachedFold, PieceRange, ReplicaWorld, Resident, TerrainEdit,
};

impl Presentation {
    /// Land a prepared apply in this frame: pointer swaps for what differs.
    pub(super) fn land(
        &mut self,
        replica: &mut ReplicaWorld,
        prepared: Prepared,
        out: &mut DriveOut,
    ) {
        let Prepared {
            id,
            at,
            columns,
            presence,
            moment,
            views,
            rest,
            released_through,
            index_only,
        } = prepared;
        let mut installed = Vec::new();
        // The fold applied the whole-world statement to these before their
        // events; the events' writes stand.
        let folded: std::collections::BTreeSet<ChunkPos> = columns.iter().map(|c| c.pos).collect();
        for outcome in columns {
            let pos = outcome.pos;
            if replica.data().columns.contains_key(&pos) {
                if matches!(outcome.column, Some(KeyChange::Removed)) {
                    replica.unload_presented_column(pos);
                    continue;
                }
                match outcome.column {
                    Some(KeyChange::Set(payload, origin)) => {
                        replica.install_prepared_column((*payload).clone(), origin)
                    }
                    Some(KeyChange::Origin(origin)) => {
                        replica.set_presented_origin(Resident::Column(pos), origin)
                    }
                    Some(KeyChange::Removed) | None => {}
                }
                for (cy, change) in outcome.sections {
                    let sp = SectionPos::new(pos.cx, cy, pos.cz);
                    match change {
                        KeyChange::Set(content, origin) => {
                            installed.push(replica.install_prepared_section(sp, content, origin));
                        }
                        KeyChange::Origin(origin) => {
                            replica.set_presented_origin(Resident::Section(sp), origin)
                        }
                        KeyChange::Removed => replica.unload_presented_section(sp),
                    }
                }
                continue;
            }
            let whole = outcome.whole;
            if whole.column.is_none() && whole.sections.is_empty() {
                self.stated.away.remove(&pos);
                continue;
            }
            if self.window.is_some_and(|w| w.contains(pos)) {
                // Folded whole, and in the window: resident at once.
                self.stated.away.remove(&pos);
                if let Some((payload, origin)) = whole.column {
                    replica.install_prepared_column((*payload).clone(), origin);
                }
                for (cy, (content, origin)) in whole.sections {
                    let sp = SectionPos::new(pos.cx, cy, pos.cz);
                    installed.push(replica.install_prepared_section(sp, content, origin));
                }
                continue;
            }
            for (range, cached) in whole
                .column
                .iter()
                .filter_map(|(p, o)| o.map(|r| (r, Cached::Column(Arc::clone(p)))))
                .chain(
                    whole
                        .sections
                        .values()
                        .filter_map(|(c, o)| o.map(|r| (r, Cached::Section(c.clone())))),
                )
                .collect::<Vec<_>>()
            {
                replica.remember_piece(range, || cached);
            }
            self.stated.away.insert(pos, whole.into_away());
            if self.window.is_some_and(|w| w.contains(pos)) {
                self.enter_later(pos);
            }
        }
        replica.finish_remote_install_batch(&installed);
        for piece in index_only {
            let (column, key) = match piece.key {
                ClientStateKey::Section([x, y, z]) => (ChunkPos::new(x, z), Some(y)),
                ClientStateKey::Column([x, z]) => (ChunkPos::new(x, z), None),
                _ => continue,
            };
            if replica.data().columns.contains_key(&column) {
                // It came into the window while this apply was prepared:
                // it goes away and comes back in through its origin.
                self.leave(replica, column);
            }
            let entry = self.stated.entry(column);
            match key {
                Some(cy) => {
                    entry.sections.insert(cy, Away::Range(piece.range));
                }
                None => entry.column = Some(Away::Range(piece.range)),
            }
            if self.window.is_some_and(|w| w.contains(column)) {
                self.enter_later(column);
            }
        }
        if let Some(presence) = presence {
            let columns: Vec<ChunkPos> = replica
                .data()
                .columns
                .keys()
                .copied()
                .filter(|c| !folded.contains(c))
                .collect();
            for c in columns {
                if !presence.lists_column(c) {
                    replica.unload_presented_column(c);
                    continue;
                }
                for sp in replica.column_sections(c) {
                    if !presence.lists_section(sp) {
                        replica.unload_presented_section(sp);
                    }
                }
            }
            self.stated.away.retain(|&c, away| {
                if folded.contains(&c) {
                    return true;
                }
                if !presence.lists_column(c) {
                    return false;
                }
                away.sections
                    .retain(|&cy, _| presence.lists_section(SectionPos::new(c.cx, cy, c.cz)));
                true
            });
        }
        self.moment = moment;
        self.queue = rest;
        self.position = at;
        self.released_through = released_through;
        self.views = views;
        self.applied = id;
        self.error = None;
        out.jumped = true;
        out.landed.push(id);
        out.messages = self.moment.restatement();
        out.cues.clear();
        out.views = self.views.clone();
    }

    /// Release every queued frame the position needs, in order, presented
    /// in full: its one-shots, sounds and cues play.
    pub(super) fn release(&mut self, replica: &mut ReplicaWorld, out: &mut DriveOut) {
        if !self.ops.is_empty() {
            return;
        }
        let limit = self.position.max(0.0).floor() as u64 + 1;
        let mut installed = Vec::new();
        while let Some(span) = self.queue.front() {
            let key = (span.file.incarnation, span.offset);
            let frame = match self.frames.get(&key) {
                None => break,
                Some(Err(why)) => {
                    if self.error.as_deref() != Some(why.as_str()) {
                        log::warn!("presentation: the events stop here: {why}");
                        self.error = Some(why.clone());
                    }
                    break;
                }
                Some(Ok(f)) if f.due > limit => break,
                Some(Ok(f)) => Arc::clone(f),
            };
            self.queue.advance(frame.record.len);
            self.release_frame(replica, &frame, out, &mut installed);
        }
        replica.finish_remote_install_batch(&installed);
    }

    fn release_frame(
        &mut self,
        replica: &mut ReplicaWorld,
        frame: &Frame,
        out: &mut DriveOut,
        installed: &mut Vec<SectionPos>,
    ) {
        self.moment.frame(frame);
        for item in &frame.items {
            match item {
                FrameItem::Terrain(edit) => self.route(replica, edit.clone(), installed),
                FrameItem::Message(msg) => out.messages.push(msg.clone()),
                FrameItem::Batch(t) => {
                    out.messages
                        .push(crate::net::protocol::ServerToClient::Tick(Box::new(
                            (**t).clone(),
                        )));
                }
                FrameItem::Cues(cues) => out.cues.push(cues.clone()),
                FrameItem::View(view) => {
                    self.views.push((**view).clone());
                    out.views.push((**view).clone());
                }
            }
        }
        self.released_through = self.released_through.max(frame.newest_batch());
        let keep_from = self
            .views
            .iter()
            .rposition(|v| v.at <= self.position)
            .unwrap_or(0);
        self.views.drain(..keep_from);
    }

    /// One released terrain write: onto the replica where its column is
    /// resident (or enters the window with it), else onto the away index.
    fn route(
        &mut self,
        replica: &mut ReplicaWorld,
        edit: TerrainEdit,
        installed: &mut Vec<SectionPos>,
    ) {
        let mut columns = BTreeSet::new();
        edit.columns(|c| {
            columns.insert(c);
        });
        let single = columns.len() == 1;
        for c in columns {
            let part = if single {
                Some(edit.clone())
            } else {
                edit.for_column(c)
            };
            let Some(part) = part else { continue };
            let resident = replica.data().columns.contains_key(&c)
                || (!self.stated.away.contains_key(&c)
                    && self.window.is_some_and(|w| w.contains(c)));
            if resident {
                installed.extend(replica.apply_terrain_edit(part));
            } else {
                self.route_away(replica, c, part);
            }
        }
    }

    fn route_away(&mut self, replica: &mut ReplicaWorld, c: ChunkPos, edit: TerrainEdit) {
        let entry = self.stated.away.entry(c).or_default();
        if entry.pending.is_empty() {
            match &edit {
                TerrainEdit::Section(payload, Some(r)) => {
                    if let Some(content) = decode_section_payload((**payload).clone()) {
                        replica.remember_piece(*r, || Cached::Section(content));
                        entry.sections.insert(payload.pos.cy, Away::Range(*r));
                        return;
                    }
                }
                TerrainEdit::Column(payload, Some(r)) => {
                    replica.remember_piece(*r, || Cached::Column(Arc::clone(payload)));
                    entry.column = Some(Away::Range(*r));
                    return;
                }
                TerrainEdit::SectionUnload(sp) => {
                    entry.sections.remove(&sp.cy);
                    return;
                }
                TerrainEdit::ColumnUnload(_) => {
                    self.stated.away.remove(&c);
                    return;
                }
                _ => {}
            }
        }
        entry.pending.push(edit);
    }

    /// Column `c`, away inside the window, loads as the window's own do.
    fn enter_later(&mut self, c: ChunkPos) {
        if !self.entering.contains(&c) {
            self.entering.insert(0, c);
        }
    }

    /// Move column `c` out of the replica into the away index.
    pub(super) fn leave(&mut self, replica: &mut ReplicaWorld, c: ChunkPos) {
        let mut away = AwayColumn::default();
        if let Some(payload) = replica.column_content(c) {
            let payload = Arc::new(payload);
            away.column = Some(match replica.origin_of(Resident::Column(c)) {
                Some(r) => {
                    replica.remember_piece(r, || Cached::Column(Arc::clone(&payload)));
                    Away::Range(r)
                }
                None => Away::Held(payload),
            });
        }
        for sp in replica.column_sections(c) {
            if let Some((content, origin)) = replica.section_content(sp) {
                away.sections.insert(
                    sp.cy,
                    match origin {
                        Some(r) => {
                            replica.remember_piece(r, || Cached::Section(content));
                            Away::Range(r)
                        }
                        None => Away::Held(content),
                    },
                );
            }
        }
        replica.unload_presented_column(c);
        self.stated.away.insert(c, away);
    }

    /// Keep resident exactly the stated columns inside the window.
    pub(super) fn keep_window(&mut self, replica: &mut ReplicaWorld) {
        if self.window_dirty {
            self.window_dirty = false;
            let window = self.window;
            let leaving: Vec<ChunkPos> = replica
                .data()
                .columns
                .keys()
                .copied()
                .filter(|&c| !window.is_some_and(|w| w.contains(c)))
                .collect();
            for c in leaving {
                self.leave(replica, c);
            }
            self.entering = match window {
                Some(w) => {
                    let mut e: Vec<ChunkPos> = self
                        .stated
                        .away
                        .keys()
                        .copied()
                        .filter(|&c| w.contains(c))
                        .collect();
                    e.sort_by_key(|&c| std::cmp::Reverse(w.distance(c)));
                    e
                }
                None => Vec::new(),
            };
        }
        if self.entering.is_empty() {
            return;
        }
        let mut needs = Vec::new();
        let mut waiting = Vec::new();
        let mut installed = Vec::new();
        // Nearest first: the list is kept farthest first, popped from the back.
        while let Some(c) = self.entering.pop() {
            if !self.window.is_some_and(|w| w.contains(c)) {
                continue;
            }
            let Some(away) = self.stated.away.get(&c) else {
                continue;
            };
            let missing: Vec<PieceRange> = away
                .ranges()
                .filter(|r| replica.cached_piece(r).is_none())
                .collect();
            if !missing.is_empty() {
                let failed = missing
                    .iter()
                    .find_map(|r| self.decoded.get(r).and_then(|d| d.as_ref().err()).cloned());
                if let Some(why) = failed {
                    log::warn!("presentation: column {c:?} cannot load: {why}");
                    self.error = Some(why);
                    continue;
                }
                needs.extend(missing.into_iter().map(Need::Piece));
                waiting.push(c);
                continue;
            }
            let away = self.stated.away.remove(&c).expect("checked above");
            installed.extend(self.enter(replica, c, away));
        }
        replica.finish_remote_install_batch(&installed);
        waiting.reverse();
        self.entering = waiting;
        if !needs.is_empty() {
            let key = self
                .window
                .and_then(|w| self.entering.last().map(|&c| w.distance(c)))
                .unwrap_or(0);
            self.request(needs, key);
        }
    }

    /// Install an away column whose content is all in memory.
    fn enter(
        &mut self,
        replica: &mut ReplicaWorld,
        c: ChunkPos,
        away: AwayColumn,
    ) -> Vec<SectionPos> {
        let content = |r: &PieceRange, replica: &ReplicaWorld| replica.cached_piece(r).cloned();
        let mut installed = Vec::new();
        if away.pending.is_empty() {
            match &away.column {
                Some(Away::Range(r)) => {
                    if let Some(Cached::Column(p)) = content(r, replica) {
                        replica.install_prepared_column((*p).clone(), Some(*r));
                    }
                }
                Some(Away::Held(p)) => replica.install_prepared_column((**p).clone(), None),
                None => {}
            }
            for (&cy, s) in &away.sections {
                let sp = SectionPos::new(c.cx, cy, c.cz);
                let (section, origin) = match s {
                    Away::Range(r) => match content(r, replica) {
                        Some(Cached::Section(s)) => (s, Some(*r)),
                        _ => continue,
                    },
                    Away::Held(s) => (s.clone(), None),
                };
                installed.push(replica.install_prepared_section(sp, section, origin));
            }
            return installed;
        }
        let mut fold = DetachedFold::new();
        match &away.column {
            Some(Away::Range(r)) => {
                if let Some(Cached::Column(p)) = content(r, replica) {
                    fold.seed_column((*p).clone(), Some(*r));
                }
            }
            Some(Away::Held(p)) => fold.seed_column((**p).clone(), None),
            None => {}
        }
        for (&cy, s) in &away.sections {
            let sp = SectionPos::new(c.cx, cy, c.cz);
            match s {
                Away::Range(r) => {
                    if let Some(Cached::Section(s)) = content(r, replica) {
                        fold.seed_section(sp, s, Some(*r));
                    }
                }
                Away::Held(s) => fold.seed_section(sp, s.clone(), None),
            }
        }
        for edit in away.pending {
            fold.apply(edit);
        }
        if let Some((payload, origin)) = fold.column(c) {
            replica.install_prepared_column(payload, origin);
        }
        for sp in fold.column_sections(c) {
            if let Some((content, origin)) = fold.section(sp) {
                installed.push(replica.install_prepared_section(sp, content, origin));
            }
        }
        installed
    }

    /// Keep read what the position will need before a new read could land:
    /// its measured rate × the measured read latency, and at least the next
    /// release.
    pub(super) fn read_ahead(&mut self, dt: f32, before: f64) {
        if dt > 0.0 {
            let rate = ((self.position - before) / f64::from(dt)).max(0.0);
            self.ticks_per_second = super::ema(self.ticks_per_second, rate);
        }
        if !self.ops.is_empty() {
            return;
        }
        let horizon = (self.ticks_per_second * self.read_seconds * 2.0).max(1.0);
        let target = self.position.max(0.0).floor() as u64 + 1 + horizon.ceil() as u64;
        let mut need = None;
        'spans: for span in self.queue.spans() {
            let mut offset = span.offset;
            while offset < span.end {
                match self.frames.get(&(span.file.incarnation, offset)) {
                    Some(Ok(f)) => {
                        if f.due > target {
                            break 'spans;
                        }
                        offset += f.record.len;
                    }
                    Some(Err(_)) => break 'spans,
                    None => {
                        need = Some(Need::Frames {
                            incarnation: span.file.incarnation,
                            offset,
                            end: span.end,
                        });
                        break 'spans;
                    }
                }
            }
        }
        if let Some(need) = need {
            self.request(vec![need], i64::MAX / 8);
        }
    }

    /// Follow changes to the bytes this presentation reads.
    pub(super) fn take_file_changes(&mut self, replica: &mut ReplicaWorld) {
        for change in self.changes.take() {
            let (incarnation, start, end, label) = match change {
                FileChange::Bytes {
                    incarnation,
                    start,
                    end,
                } => (incarnation, start, end, None),
                FileChange::Ended { incarnation, label } => (incarnation, 0, u64::MAX, Some(label)),
            };
            replica.invalidate_piece_bytes(incarnation, start, end);
            let hit = |i: u64, o: u64| i == incarnation && o < end;
            self.shapes.retain(|&(i, o), _| !hit(i, o));
            self.frames.retain(|&(i, o), f| {
                !(hit(i, o) && f.as_ref().map_or(true, |f| o + f.record.len > start))
            });
            self.decoded.retain(|r, _| {
                !(r.incarnation == incarnation && r.offset < end && start < r.offset + r.len)
            });
            let gone = self.stated.forget_bytes(incarnation, start, end);
            if let Some(label) = label {
                self.queue.forget(incarnation);
                self.files.remove(&incarnation);
                if gone > 0 {
                    self.error = Some(format!(
                        "{label} was deleted: {gone} stated keys whose content lived only there \
                         left the presented world"
                    ));
                }
            }
        }
    }
}
