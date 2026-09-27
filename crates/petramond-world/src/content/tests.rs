use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::assets::{PackRoots, PackSet};
use crate::block::Block;

fn fluid_row(name: &str, tiles: &str) -> String {
    format!(
        r#"{{
        "block": "{name}",
        "fluid": {{
            "delay": 5, "drop_off": 1, "renewable": false,
            "motion": {{
                "speed_scale": 1.0, "accel": 8.0, "friction": 0.4, "rise": 1.5, "sink": 0.5,
                "vertical_accel": 6.0, "entry_friction": 0.5, "probe_fraction": 0.5,
                "probe_offset": 0.0, "climb": "jump"
            }},
            "medium": {{
                "fog_color": [0.2, 0.2, 0.2], "fog_start": 0.5, "fog_end": 8.0,
                "volume_tint": [1.0, 1.0, 1.0], "surface_tint": [1.0, 1.0, 1.0],
                "surface_alpha": 1.0
            }}
        }},
        "shape": "cube", "flags": ["transparent", "fluid"], "tags": ["replaceable"],
        "behavior": "fluid", "interaction": "none", "collision": [], "emission": 0,
        "tiles": ["{tiles}_still", "{tiles}_still", "{tiles}_still"],
        "flow_tile": "{tiles}_flow",
        "material": "none", "hardness": -1, "drops": []
    }}"#
    )
}

struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("petramond-content-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("mods")).unwrap();
        Fixture(root)
    }

    fn mods(&self) -> PathBuf {
        self.0.join("mods")
    }

    fn pack(&self, id: &str, extra: &str, files: &[(&str, String)]) -> &Fixture {
        let dir = self.mods().join(id);
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(
            dir.join("pack.json"),
            format!(r#"{{"id": "{id}", "name": "{id}"{extra}}}"#),
        )
        .unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        self
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn atlas(tiles: &str) -> String {
    format!(
        r#"{{"tiles": [
            {{"name": "{tiles}_still", "file": "stone.png"}},
            {{"name": "{tiles}_flow", "file": "dirt.png"}}
        ]}}"#
    )
}

fn packs(mods: &Path) -> PackSet {
    PackSet::discover(&PackRoots::with_mods([mods.to_path_buf()]))
}

fn block_id(name: &str) -> Option<u16> {
    crate::registry::names().blocks.id(name)
}

#[test]
fn the_loader_reports_every_bad_row_and_skips_dependent_stages() {
    let fx = Fixture::new("bad-rows");
    fx.pack(
        "fx",
        "",
        &[(
            "blocks.json",
            format!(
                r#"{{"blocks": [{}, {}]}}"#,
                fluid_row("fx:first", "fx_missing_a"),
                fluid_row("fx:second", "fx_missing_b")
            ),
        )],
    );
    let report = ContentRegistry::load(packs(&fx.mods())).expect_err("unknown tiles fail");
    let blocks: Vec<&ContentError> = report
        .errors
        .iter()
        .filter(|e| e.stage == stage::BLOCKS)
        .collect();
    assert_eq!(blocks.len(), 2, "{report}");
    assert!(blocks[0].message.contains("fx:first"), "{report}");
    assert!(blocks[1].message.contains("fx:second"), "{report}");
    for skipped in [stage::BLOCK_VIEWS, stage::ITEMS, stage::CONSTRUCTION] {
        assert!(
            report.skipped.iter().any(|(s, _)| *s == skipped),
            "{skipped} must be skipped: {report}"
        );
    }
}

#[test]
fn a_pinned_fixture_registry_is_current_on_its_thread_only() {
    let fx = Fixture::new("pinned");
    fx.pack(
        "fx",
        "",
        &[
            ("textures/atlas.json", atlas("fx_tar")),
            (
                "blocks.json",
                format!(r#"{{"blocks": [{}]}}"#, fluid_row("fx:tar", "fx_tar")),
            ),
        ],
    );
    let content = test_support::with_mods(&fx.mods());
    let tar = content
        .names()
        .blocks
        .id("fx:tar")
        .expect("the pack registers its block");
    assert_eq!(tar as usize, content.names().blocks.len() - 1);
    {
        let _pin = pin(content);
        assert!(std::ptr::eq(current(), content.registry()));
        assert_eq!(block_id("fx:tar"), Some(tar));
        assert!(
            Block(tar).is_fluid(),
            "hot accessors read the pinned registry"
        );
        let elsewhere = std::thread::spawn(|| block_id("fx:tar")).join().unwrap();
        assert_eq!(elsewhere, None, "another thread keeps the process registry");
    }
    assert_eq!(block_id("fx:tar"), None, "the pin ends with its guard");
}

#[test]
fn for_world_drops_a_disabled_packs_ids_and_reuses_registries() {
    let fx = Fixture::new("for-world");
    fx.pack(
        "fx",
        "",
        &[
            ("textures/atlas.json", atlas("fx_tar")),
            (
                "blocks.json",
                format!(r#"{{"blocks": [{}]}}"#, fluid_row("fx:tar", "fx_tar")),
            ),
        ],
    );
    let base = test_support::with_mods(&fx.mods());
    let _pin = pin(base);
    assert!(for_world(&BTreeSet::new(), &[]).unwrap().same(base));
    let off: BTreeSet<String> = ["fx".to_owned()].into();
    let scoped = for_world(&off, &[]).unwrap();
    assert!(!scoped.same(base));
    assert_eq!(scoped.names().blocks.id("fx:tar"), None);
    assert_eq!(
        scoped.names().blocks.len() + 1,
        base.names().blocks.len(),
        "exactly the disabled pack's id is gone"
    );
    assert!(for_world(&off, &[]).unwrap().same(scoped), "built once");
    activate(scoped);
    assert_eq!(block_id("fx:tar"), None, "activate re-pins a pinned thread");
}

#[test]
fn enabled_views_cascade_to_dependents_and_keep_asset_layers() {
    let fx = Fixture::new("cascade");
    fx.pack("base_mod", "", &[])
        .pack("dependent", r#", "dependencies": ["base_mod"]"#, &[])
        .pack("loner", "", &[]);
    let all = packs(&fx.mods());
    assert_eq!(all.packs().len(), 3);
    let scoped = all.enabled(&["base_mod".to_owned()].into());
    let ids: Vec<&str> = scoped
        .packs()
        .iter()
        .filter_map(|p| p.id.as_deref())
        .collect();
    assert_eq!(ids, ["loner"]);
    assert!(scoped.disabled().contains("dependent"));
    assert_eq!(scoped.installed().len(), 3);
    assert_eq!(
        scoped.layers().len(),
        3,
        "asset layers span every installed pack"
    );
    assert_eq!(scoped.catalog_layers().len(), 1);
}

#[test]
fn refused_packs_are_listed_on_the_pack_set() {
    let fx = Fixture::new("refused");
    fx.pack("needy", r#", "dependencies": ["absent"]"#, &[]);
    let set = packs(&fx.mods());
    assert!(set.packs().is_empty());
    assert_eq!(set.refused().len(), 1);
    assert_eq!(set.refused()[0].dir_name, "needy");
}
