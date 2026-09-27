use mod_sdk::*;

pub const REST_TICKS: u64 = 72_000;

const KEY: &str = "farming:attract_rests";
const LEGACY_PREFIX: &str = "farming:attract_rest";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Area {
    lo: [i32; 2],
    hi: [i32; 2],
    expiry: u64,
}

impl Area {
    fn covers(&self, [cx, cz]: [i32; 2]) -> bool {
        (self.lo[0]..=self.hi[0]).contains(&cx) && (self.lo[1]..=self.hi[1]).contains(&cz)
    }
}

#[derive(Debug, Default, PartialEq)]
struct Areas(Vec<Area>);

impl KvRecord for Areas {
    const VERSION: u8 = 1;

    fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_capacity(self.0.len() * 24);
        for area in &self.0 {
            w.i32(area.lo[0]);
            w.i32(area.lo[1]);
            w.i32(area.hi[0]);
            w.i32(area.hi[1]);
            w.raw(&area.expiry.to_le_bytes());
        }
        w.finish()
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if !bytes.len().is_multiple_of(24) {
            return None;
        }
        let mut r = ByteReader::new(bytes);
        let mut areas = Vec::with_capacity(bytes.len() / 24);
        while let Some(x0) = r.i32() {
            let lo = [x0, r.i32()?];
            let hi = [r.i32()?, r.i32()?];
            let expiry = u64::from_le_bytes(r.take(8)?.try_into().ok()?);
            areas.push(Area { lo, hi, expiry });
        }
        Some(Self(areas))
    }
}

#[derive(Default)]
pub struct Rests {
    areas: Option<Areas>,
}

impl Rests {
    fn areas(&mut self) -> &mut Areas {
        self.areas
            .get_or_insert_with(|| match world_kv_load::<Areas>(KEY) {
                Ok(areas) => areas.unwrap_or_default(),
                Err(error) => {
                    log(&format!(
                        "farming: {KEY} is unreadable ({error}); rests start afresh"
                    ));
                    Areas::default()
                }
            })
    }

    pub fn resting(&mut self, pos: [i32; 3]) -> bool {
        let now = current_tick();
        let col = column(pos);
        let rested = self
            .areas()
            .0
            .iter()
            .any(|area| now < area.expiry && area.covers(col));
        rested || legacy_resting(col, now)
    }

    pub fn begin(&mut self, pos: [i32; 3], radius: i32) {
        let now = current_tick();
        let areas = self.areas();
        areas.0.retain(|area| now < area.expiry);
        areas.0.push(Area {
            lo: column([pos[0] - radius, 0, pos[2] - radius]),
            hi: column([pos[0] + radius, 0, pos[2] + radius]),
            expiry: now + REST_TICKS,
        });
        world_kv_store(KEY, areas);
    }
}

fn legacy_resting([cx, cz]: [i32; 2], now: u64) -> bool {
    let key = format!("{LEGACY_PREFIX}/{cx}/{cz}");
    let Some(bytes) = world_kv_get(&key) else {
        return false;
    };
    let expiry = <[u8; 8]>::try_from(bytes.as_slice()).map(u64::from_le_bytes);
    if expiry.is_ok_and(|expiry| now < expiry) {
        return true;
    }
    world_kv_delete(&key);
    false
}

fn column(pos: [i32; 3]) -> [i32; 2] {
    [pos[0] >> 4, pos[2] >> 4]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_floor_into_their_own_column() {
        assert_eq!(column([-1, 0, -16]), [-1, -1]);
        assert_eq!(column([-17, 0, 15]), [-2, 0]);
    }

    #[test]
    fn an_area_covers_every_column_the_radius_touches() {
        let area = Area {
            lo: column([1 - 16, 0, 1 - 16]),
            hi: column([1 + 16, 0, 1 + 16]),
            expiry: 0,
        };
        assert!(area.covers([-1, -1]));
        assert!(area.covers([1, 1]));
        assert!(!area.covers([2, 0]));
        let covered = (-3..=3)
            .flat_map(|x| (-3..=3).map(move |z| [x, z]))
            .filter(|col| area.covers(*col))
            .count();
        assert_eq!(covered, 9);
    }

    #[test]
    fn the_stored_row_round_trips() {
        let areas = Areas(vec![
            Area {
                lo: [-2, -1],
                hi: [0, 1],
                expiry: 72_000,
            },
            Area {
                lo: [i32::MIN, 5],
                hi: [i32::MAX, 6],
                expiry: u64::MAX,
            },
        ]);
        let bytes = encode_versioned(&areas);
        assert_eq!(bytes[0], Areas::VERSION);
        assert_eq!(decode_versioned::<Areas>(&bytes), Ok(areas));
        assert_eq!(
            decode_versioned::<Areas>(&[Areas::VERSION]),
            Ok(Areas::default())
        );
    }

    #[test]
    fn a_truncated_row_does_not_decode() {
        let bytes = encode_versioned(&Areas(vec![Area {
            lo: [0, 0],
            hi: [1, 1],
            expiry: 9,
        }]));
        for cut in 2..bytes.len() {
            assert_eq!(
                decode_versioned::<Areas>(&bytes[..cut]),
                Err(RecordError::Corrupt),
                "cut at {cut}"
            );
        }
    }
}
