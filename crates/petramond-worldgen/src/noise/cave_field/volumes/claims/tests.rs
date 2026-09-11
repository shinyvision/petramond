use super::*;
use crate::data::{excavations, underground};

#[test]
fn a_cut_claims_only_its_column_and_the_declared_height_band() {
    let habitats = underground::test_table(&[r#"{"underground_biomes":[{
        "underground_biome":"test:patch","climate":{"depth":[-2,2]},"y":[-24,-16]
    }]}"#]);
    let rows = excavations::test_table(
        &[r#"{"excavations":[{
        "excavation":"test:cut", "placement":{"spacing":32,"y":[-20,-20],"underground_biome":"test:patch"},
        "field":{"radius":[8,8],"height":[8,8],"bound_radius":16,"y":[-32,-16],"grid_step":16,"separation":16,
            "cut":["and",["and",["lt",["abs",["sub","x",-1]],3],["lt",["abs",["sub","z",-1]],3]],["and",["le",-22,"y"],["le","y",-18]]],
            "material":0,"palette":["petramond:air"],"biome_fill":{"y":[-64,31]}}
    }]}"#],
        habitats,
    );
    let field = CaveField::with_tables(31, habitats, rows);
    let id = habitats.id("test:patch").unwrap();
    let columns = Columns::gather(&field, [-16, -16], [15, 15]);
    for pos in [[-16, -64, -16], [-1, 0, -1], [-7, 31, -5], [0, 0, 0]] {
        assert_eq!(columns.at(pos), Some(id));
        assert_eq!(
            Columns::gather(&field, [pos[0], pos[2]], [pos[0], pos[2]]).at(pos),
            Some(id)
        );
    }
    for pos in [[-1, 32, -1], [-1, -65, -1]] {
        assert_eq!(columns.at(pos), None);
    }
    let mut ids = IdSet::default();
    include(&field, [-16, -64, -16], [-1, 31, -1], &mut ids);
    assert!(ids.contains(id));
}
