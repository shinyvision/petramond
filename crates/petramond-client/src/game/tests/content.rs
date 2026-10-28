use petramond::net::handle::ServerHandle;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::content::{self, Content};
use petramond_world::item::ItemType;

use crate::game::Game;

#[test]
fn world_mod_settings_select_content_before_session_construction() {
    petramond_world::test_child::run_ignored(
        "game::tests::content::world_mod_settings_select_content_before_session_construction_inner",
        [(
            "PETRAMOND_DATA_DIR",
            petramond_util::test_dirs::test_process_data_dir(),
        )],
    )
    .assert_passed();
}

#[test]
#[ignore]
fn world_mod_settings_select_content_before_session_construction_inner() {
    let fixture = TestScratchDir::new("session-content");
    let mods = fixture.join("mods");
    let pack = mods.join("scopefx");
    std::fs::create_dir_all(&pack).unwrap();
    for (file, text) in [
        ("pack.json", r#"{"id":"scopefx","name":"Session fixture"}"#),
        (
            "blocks.json",
            r#"{"blocks":[{
            "block":"scopefx:block","shape":"cube","flags":["solid"],"tags":[],
            "collision":[],"emission":0,"tiles":["stone","stone","stone"],
            "material":"stone","hardness":1,"drops":[]
        }]}"#,
        ),
        (
            "items.json",
            r#"{"items":[{
            "item":"scopefx:item","key":"scopefx:item","name":"Fixture item",
            "max_stack_size":64,"block":"scopefx:block"
        }]}"#,
        ),
        (
            "models.json",
            r#"{"models":[{
            "key":"scopefx:model","model_file":"unused.bbmodel","cells":[1,1,1]
        }]}"#,
        ),
        (
            "mobs.json",
            r#"{"mobs":[{
            "mob":"scopefx:mob","key":"scopefx:mob","model":"unused.bbmodel",
            "scale":1,"size":{"half_width":0.2,"height":0.4},"tags":{"petramond:health":1},
            "walk_speed":1,"jump_speed":0,"turn_rate":1,"walk_anim_rate":1,
            "cap":1,"spawn_group":{"min":1,"max":1},
            "wander":{"chance_per_tick":0,"radius":1},
            "habitat":{"avoid":[],"prefer":[]},"avoid_fluids":false,"brain":[]
        }]}"#,
        ),
    ] {
        std::fs::write(pack.join(file), text).unwrap();
    }
    let base = content::test_support::with_mods(&mods);
    let _base = content::pin(base);
    let world = "session-content";
    let dir = petramond::save::world_dir(world);
    let mut settings = petramond::save::settings::WorldSettings::default();
    settings.disabled_mods.insert("scopefx".into());
    petramond::save::settings::store(&dir, &settings).unwrap();

    for enabled in [false, true] {
        if enabled {
            settings.disabled_mods.clear();
            petramond::save::settings::store(&dir, &settings).unwrap();
        }
        let (mut server, bootstrap) = super::bootstrap::build_session_inline(world, 1, 1);
        let game = Game::assemble(
            Camera::new(WorldPos::new(0.0, 80.0, 0.0), 1.0),
            ServerHandle::serverless(),
            bootstrap,
        );
        assert!(Content::current().same(server.content()));
        assert_eq!(ItemType::by_name("scopefx:item").is_some(), enabled);
        assert_eq!(petramond::mob::by_key("scopefx:mob").is_some(), enabled);
        assert_eq!(
            content::current()
                .names()
                .blocks
                .id("scopefx:block")
                .is_some(),
            enabled,
        );
        assert_eq!(
            petramond_world::block_model::all()
                .iter()
                .any(|kind| petramond_world::block_model::def(*kind).key == "scopefx:model"),
            enabled,
        );
        server.close_sessions_and_save();
        drop(server);
        game.shutdown();
        assert!(
            Content::current().same(base),
            "leaving restores shell content"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
