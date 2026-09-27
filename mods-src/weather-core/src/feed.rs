use mod_sdk::{emit_event, register_event_handler, EventKind, EventPayload};

use crate::{FieldParams, FieldRow};

pub const FIELD_EVENT: &str = "weather:field";

pub const FEED_LAPSE_TICKS: u32 = 1;

pub fn publish(row: &FieldRow) {
    emit_event(FIELD_EVENT, &row.encode());
}

pub fn subscribe(handler_id: u32) {
    register_event_handler(EventKind::ModEvent, 0, handler_id);
}

#[derive(Default)]
pub struct FieldFeed {
    row: Option<FieldRow>,
    silent: u32,
}

impl FieldFeed {
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

    pub fn tick(&mut self) {
        self.silent = self.silent.saturating_add(1);
        if self.silent > FEED_LAPSE_TICKS {
            self.row = None;
        }
    }

    pub fn params(&self) -> Option<FieldParams> {
        self.row.map(|row| row.params)
    }

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
