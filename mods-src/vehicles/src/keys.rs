use crate::rail::{Axis, Corner, Dir, Form};

mod_sdk::pack_keys! {
    pub BOAT_ITEM: Item = "vehicles:boat";
    pub BOAT_MOB: Mob = "vehicles:boat";
    pub MINECART_ITEM: Item = "vehicles:minecart";
    pub MINECART_MOB: Mob = "vehicles:minecart";
    pub CART_TAG: MobTag = "vehicles:cart";
    pub ROLL_SOUND: Sound = "vehicles:minecart_roll";
    pub WATER: Block = "petramond:water";

    RAIL_NS: Block = "vehicles:rail_ns";
    RAIL_EW: Block = "vehicles:rail_ew";
    RAIL_CURVE_NE: Block = "vehicles:rail_curve_ne";
    RAIL_CURVE_SE: Block = "vehicles:rail_curve_se";
    RAIL_CURVE_SW: Block = "vehicles:rail_curve_sw";
    RAIL_CURVE_NW: Block = "vehicles:rail_curve_nw";
    RAIL_SLOPE_N: Block = "vehicles:rail_slope_n";
    RAIL_SLOPE_E: Block = "vehicles:rail_slope_e";
    RAIL_SLOPE_S: Block = "vehicles:rail_slope_s";
    RAIL_SLOPE_W: Block = "vehicles:rail_slope_w";
    BOOSTER_NS: Block = "vehicles:booster_rail_ns";
    BOOSTER_EW: Block = "vehicles:booster_rail_ew";
    BOOSTER_SLOPE_N: Block = "vehicles:booster_rail_slope_n";
    BOOSTER_SLOPE_E: Block = "vehicles:booster_rail_slope_e";
    BOOSTER_SLOPE_S: Block = "vehicles:booster_rail_slope_s";
    BOOSTER_SLOPE_W: Block = "vehicles:booster_rail_slope_w";
}

pub const RAIL_ROWS: [(Form, bool, &str); 16] = [
    (Form::Straight(Axis::NS), false, RAIL_NS),
    (Form::Straight(Axis::EW), false, RAIL_EW),
    (Form::Curve(Corner::NE), false, RAIL_CURVE_NE),
    (Form::Curve(Corner::SE), false, RAIL_CURVE_SE),
    (Form::Curve(Corner::SW), false, RAIL_CURVE_SW),
    (Form::Curve(Corner::NW), false, RAIL_CURVE_NW),
    (Form::Slope(Dir::N), false, RAIL_SLOPE_N),
    (Form::Slope(Dir::E), false, RAIL_SLOPE_E),
    (Form::Slope(Dir::S), false, RAIL_SLOPE_S),
    (Form::Slope(Dir::W), false, RAIL_SLOPE_W),
    (Form::Straight(Axis::NS), true, BOOSTER_NS),
    (Form::Straight(Axis::EW), true, BOOSTER_EW),
    (Form::Slope(Dir::N), true, BOOSTER_SLOPE_N),
    (Form::Slope(Dir::E), true, BOOSTER_SLOPE_E),
    (Form::Slope(Dir::S), true, BOOSTER_SLOPE_S),
    (Form::Slope(Dir::W), true, BOOSTER_SLOPE_W),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[PACK_KEYS]);
    }

    #[test]
    fn the_rail_table_covers_every_form_and_each_non_curve_booster_once() {
        for form in Form::ALL {
            let plain = RAIL_ROWS.iter().filter(|r| r.0 == form && !r.1).count();
            let boosted = RAIL_ROWS.iter().filter(|r| r.0 == form && r.1).count();
            assert_eq!(plain, 1, "{form:?}");
            assert_eq!(boosted, usize::from(!form.is_curve()), "{form:?}");
        }
    }
}
