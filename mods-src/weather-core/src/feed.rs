//! The weather field's interop channel (the `sdk` feature).
//!
//! The weather mod PUBLISHES its [`FieldRow`] once per tick as the
//! session-scoped [`FIELD_EVENT`] mod event; every other server mod that
//! cares about rain SUBSCRIBES and keeps the latest row in a [`FieldFeed`].
//! Nothing touches the persistent world KV, so:
//!
//! - a world whose weather mod is not installed hears no event and the feed
//!   reads "clear sky" — there is no leftover row to detect as stale;
//! - a weather mod that stops publishing mid-session (disabled after a
//!   trap) lapses the feed after one silent tick;
//! - a consumer need not run after the weather mod: [`FieldFeed::params_at`]
//!   advances the row to the clock the consumer reads.

use mod_sdk::{emit_event, register_event_handler, EventKind, EventPayload};

use crate::{FieldParams, FieldRow};

/// The mod event the weather mod publishes its row on. The key carries the
/// producer's namespace, which the host enforces: only the weather mod can
/// emit it.
pub const FIELD_EVENT: &str = "weather:field";

/// Ticks a feed may go without a publish before it reads as absent. The
/// producer publishes every tick and mod events drain within the tick they
/// are emitted, so every consumer tick normally sees a row one tick old;
/// anything older means the producer stopped.
pub const FEED_LAPSE_TICKS: u32 = 1;

/// Publish this tick's row (the weather mod's server tick).
pub fn publish(row: &FieldRow) {
    emit_event(FIELD_EVENT, &row.encode());
}

/// Subscribe a consumer to the channel: mod events reach `handler_id`, which
/// hands each payload to [`FieldFeed::observe`]. Call from `Mod::init`.
pub fn subscribe(handler_id: u32) {
    register_event_handler(EventKind::ModEvent, 0, handler_id);
}

/// A consumer's view of the channel: the latest row, until it lapses.
#[derive(Default)]
pub struct FieldFeed {
    row: Option<FieldRow>,
    /// Consumer ticks since the last publish arrived.
    silent: u32,
}

impl FieldFeed {
    /// Take a mod event from the subscribed handler. `true` when it was the
    /// field event (consumed, whether or not its payload decoded), `false`
    /// for any other event, which the caller routes on.
    pub fn observe(&mut self, payload: &EventPayload) -> bool {
        let EventPayload::ModEvent { key, data } = payload else {
            return false;
        };
        if key != FIELD_EVENT {
            return false;
        }
        self.row = FieldRow::decode(data);
        self.silent = 0;
        true
    }

    /// Age the feed by one tick; call once per consumer tick, before reading.
    /// A producer that missed more than [`FEED_LAPSE_TICKS`] publishes reads
    /// as absent from here on.
    pub fn tick(&mut self) {
        self.silent = self.silent.saturating_add(1);
        if self.silent > FEED_LAPSE_TICKS {
            self.row = None;
        }
    }

    /// The field exactly as last published; `None` = clear sky.
    pub fn params(&self) -> Option<FieldParams> {
        self.row.map(|row| row.params)
    }

    /// The field at `clock` (see [`FieldRow::params_at`]); `None` = clear sky.
    pub fn params_at(&self, clock: u64) -> Option<FieldParams> {
        self.row.map(|row| row.params_at(clock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field_params;

    fn row() -> FieldRow {
        FieldRow {
            params: field_params([100.0, 200.0], 5_000, 7),
            wind: [1.0, 0.5],
            clock: 5_000,
        }
    }

    fn event(key: &str, data: Vec<u8>) -> EventPayload {
        EventPayload::ModEvent {
            key: key.to_owned(),
            data,
        }
    }

    #[test]
    fn a_published_row_reads_until_the_producer_falls_silent() {
        let mut feed = FieldFeed::default();
        assert_eq!(feed.params(), None, "nothing heard: clear sky");
        assert!(feed.observe(&event(FIELD_EVENT, row().encode().to_vec())));
        feed.tick();
        assert_eq!(feed.params(), Some(row().params), "one tick old is live");
        feed.tick();
        assert_eq!(feed.params(), None, "a missed publish lapses the feed");
        assert!(feed.observe(&event(FIELD_EVENT, row().encode().to_vec())));
        assert_eq!(
            feed.params_at(5_000),
            Some(row().params),
            "a resumed producer is heard again"
        );
    }

    #[test]
    fn foreign_and_corrupt_events_never_read_as_weather() {
        let mut feed = FieldFeed::default();
        assert!(!feed.observe(&event("combat:shield_impact", Vec::new())));
        assert_eq!(feed.params(), None);
        assert!(feed.observe(&event(FIELD_EVENT, vec![1, 2, 3])));
        assert_eq!(feed.params(), None, "an undecodable row is no weather");
    }
}
