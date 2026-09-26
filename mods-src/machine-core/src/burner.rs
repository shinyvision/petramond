//! The fuel fire every fuelled machine keeps: burn down a tick at a time,
//! relight from a fuel slot only when there is work for the heat, and show
//! how much of the current fuel item is left.

use mod_sdk::*;

use crate::{consume_one, Caches};

/// A fuelled machine's fire — the engine furnace's burn rule, owned once so
/// an oven, a forge and any later kiln cannot drift apart.
///
/// Persisted as two consecutive LE `u32`s (`remaining`, then `max`) inside
/// the machine's own state record, through [`decode`](Self::decode) /
/// [`encode`](Self::encode), so the layout is the one both machines already
/// saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Burner {
    /// Ticks of fire left from the current fuel item.
    pub remaining: u32,
    /// The current fuel item's whole burn, for the gauge.
    pub max: u32,
}

impl Burner {
    /// Whether the fire is burning.
    pub fn lit(self) -> bool {
        self.remaining > 0
    }

    /// One tick of burning.
    pub fn tick(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }

    /// Relight from `fuel` when the fire is out AND the machine wants heat —
    /// idle fuel is never consumed (the furnace contract). A fuel item's burn
    /// ticks come from its row through `caches`; an item with none stays in
    /// the slot. Returns whether the fire caught.
    pub fn relight(
        &mut self,
        wants_heat: bool,
        fuel: &mut Option<ItemStackData>,
        caches: &mut Caches,
    ) -> bool {
        if !wants_heat || self.lit() {
            return false;
        }
        let Some(item) = fuel.as_ref().map(|s| s.item.clone()) else {
            return false;
        };
        let burn = caches.fuel_ticks_for(&item);
        if burn == 0 {
            return false;
        }
        *self = Burner {
            remaining: burn,
            max: burn,
        };
        consume_one(fuel);
        true
    }

    /// The fuel gauge in `[0, 1]`: how much of the current item is left.
    pub fn gauge01(self) -> f32 {
        if self.max == 0 {
            0.0
        } else {
            self.remaining as f32 / self.max as f32
        }
    }

    /// Read the two persisted fields; a missing field reads as out.
    pub fn decode(r: &mut ByteReader) -> Burner {
        Burner {
            remaining: r.u32().unwrap_or(0),
            max: r.u32().unwrap_or(0),
        }
    }

    /// Write the two persisted fields.
    pub fn encode(self, w: &mut ByteWriter) {
        w.u32(self.remaining);
        w.u32(self.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coal(count: u8) -> Option<ItemStackData> {
        Some(ItemStackData {
            item: "test:coal".into(),
            count,
            data: Vec::new(),
        })
    }

    fn caches() -> Caches {
        let mut caches = Caches::default();
        caches.fuel_ticks.insert("test:coal".into(), 80);
        caches.fuel_ticks.insert("test:sand".into(), 0);
        caches
    }

    #[test]
    fn relights_only_when_out_and_wanted() {
        let mut caches = caches();
        let mut fuel = coal(2);
        let mut fire = Burner::default();
        assert!(!fire.relight(false, &mut fuel, &mut caches), "idle fuel burned");
        assert_eq!(fuel, coal(2));

        assert!(fire.relight(true, &mut fuel, &mut caches));
        assert_eq!(fire, Burner { remaining: 80, max: 80 });
        assert_eq!(fuel, coal(1), "one item per relight");

        fire.tick();
        assert!(!fire.relight(true, &mut fuel, &mut caches), "relit while burning");
        assert_eq!(fuel, coal(1));
        assert_eq!(fire.gauge01(), 79.0 / 80.0);
    }

    #[test]
    fn a_non_fuel_stays_in_the_slot_and_the_gauge_reads_empty() {
        let mut caches = caches();
        let mut sand = Some(ItemStackData {
            item: "test:sand".into(),
            count: 3,
            data: Vec::new(),
        });
        let mut fire = Burner::default();
        assert!(!fire.relight(true, &mut sand, &mut caches));
        assert_eq!(sand.map(|s| s.count), Some(3));
        assert_eq!(fire.gauge01(), 0.0);
        fire.tick();
        assert_eq!(fire.remaining, 0, "an out fire does not wrap");
    }

    #[test]
    fn the_persisted_layout_is_remaining_then_max() {
        let fire = Burner {
            remaining: 733,
            max: 1600,
        };
        let mut w = ByteWriter::new();
        fire.encode(&mut w);
        let bytes = w.finish();
        assert_eq!(bytes[..4], 733u32.to_le_bytes());
        assert_eq!(Burner::decode(&mut ByteReader::new(&bytes)), fire);
        assert_eq!(Burner::decode(&mut ByteReader::new(&[])), Burner::default());
    }
}
