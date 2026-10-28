use super::bake_mobs;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::content::{self, Content};

#[test]
fn parallel_mob_bakes_use_the_callers_content_registry() {
    let process_content = Content::current();
    let scratch = TestScratchDir::new("mob-bake-content");
    let mods = scratch.join("mods");
    let pack = mods.join("bakefixture");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{"id":"bakefixture","name":"Mob bake fixture"}"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("mobs.json"),
        r#"{"mobs":[{
        "mob":"bakefixture:mob","key":"bakefixture:mob","model":"unused.bbmodel",
        "scale":1,"size":{"half_width":0.2,"height":0.4},"tags":{"petramond:health":1},
        "walk_speed":1,"jump_speed":0,"turn_rate":1,"walk_anim_rate":1,
        "cap":1,"spawn_group":{"min":1,"max":1},
        "wander":{"chance_per_tick":0,"radius":1},
        "habitat":{"avoid":[],"prefer":[]},"avoid_fluids":false,"brain":[]
    }]}"#,
    )
    .unwrap();
    let own = content::test_support::with_mods(&mods);
    let _pin = content::pin(own);
    assert!(!own.same(process_content));
    assert!(petramond::mob::by_key("bakefixture:mob").is_some());
    let expected: Vec<_> = petramond::mob::defs()
        .iter()
        .map(|def| petramond::mob::model(def.mob))
        .collect();

    let bakes = bake_mobs();
    assert_eq!(bakes.len(), expected.len());
    for (bake, model) in bakes.iter().zip(expected) {
        assert!(
            std::ptr::eq(bake.model, model),
            "each rig must bake its own registry's model"
        );
    }
    assert!(Content::current().same(own));
}
