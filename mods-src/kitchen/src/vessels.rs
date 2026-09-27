use mod_sdk::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VesselSpec {
    returns: String,
}

const VESSEL_KEY: &str = "kitchen:vessel";

#[derive(Default)]
pub struct Vessels {
    rows: Vec<(ItemId, String)>,
}

impl Vessels {
    pub fn init(&mut self) {
        for (dish, spec) in items_with_data_as::<VesselSpec>(VESSEL_KEY) {
            if resolve_item(&spec.returns).is_some() {
                self.rows.push((dish, spec.returns));
            } else {
                log(&format!("kitchen: unknown vessel '{}'", spec.returns));
            }
        }
    }

    pub fn on_item_used(&self, player: PlayerId, item: ItemId, kind: ItemUseEvent) {
        if kind != ItemUseEvent::Eaten {
            return;
        }
        for (dish, returns) in &self.rows {
            if *dish == item {
                give_item_to(player, returns, 1, &[]);
            }
        }
    }
}

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn shipped_stew_declares_its_returned_bowl() {
        let rows = pack_rows_with_data(include_str!("../pack/items.json"), "items", VESSEL_KEY);
        assert_eq!(rows.len(), 1);
        let (dish, raw) = &rows[0];
        assert_eq!(dish, "kitchen:rabbit_stew");
        let spec: VesselSpec = parse_row_data(raw).unwrap();
        assert_eq!(spec.returns, "kitchen:wooden_bowl");
    }
}
