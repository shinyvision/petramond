use super::*;
use crate::net::protocol::{SectionBlocks, SectionPayload};
use crate::net::secure::channels_for_test;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::assets::{PackRoots, PackSet};
use petramond_world::chunk::{SectionPos, SECTION_VOLUME};
use petramond_world::content::{self, ContentRegistry};
use petramond_world::section::Section;
use std::net::TcpListener;

#[test]
fn connection_remaps_section_metrics_using_its_own_content() {
    petramond_world::test_child::run_ignored(
        "net::connection::tests::content::connection_remaps_section_metrics_using_its_own_content_inner",
        std::iter::empty::<(&str, &str)>(),
    )
    .assert_passed();
}

#[test]
#[ignore]
fn connection_remaps_section_metrics_using_its_own_content_inner() {
    let shell = ContentRegistry::load(PackSet::discover(&PackRoots::with_mods([]))).unwrap();
    content::install(shell);
    let scratch = TestScratchDir::new("connection-content");
    let mods = scratch.join("mods");
    let pack = mods.join("netfx");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{"id":"netfx","name":"Connection fixture"}"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("blocks.json"),
        r#"{"blocks":[{
            "block":"netfx:block","shape":"cube","flags":["solid","opaque"],"tags":[],
            "collision":[],"emission":0,"tiles":["stone","stone","stone"],
            "material":"stone","hardness":1,"drops":[]
        }]}"#,
    )
    .unwrap();
    let own = content::test_support::with_mods(&mods);
    let _pin = content::pin(own);
    let id = own.names().blocks.id("netfx:block").unwrap();
    let blocks = vec![id; SECTION_VOLUME];
    let expected = Section::metrics_from_blocks(&blocks);
    assert_ne!(
        shell.scope(|| Section::metrics_from_blocks(&blocks)),
        expected
    );
    let mut tables = crate::net::remap::local_name_tables();
    tables.blocks.push("missing:block".into());
    let remap = IdRemap::build(&tables);
    assert!(!remap.is_identity());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let (client_keys, mut server_keys) = channels_for_test();
    let mut conn = TcpClientConn::spawn(client, remap, client_keys).unwrap();
    let incoming = conn.take_receiver();
    write_sealed(
        &mut server,
        &ServerToClient::SectionData(Box::new(SectionPayload {
            pos: SectionPos::new(0, 0, 0),
            blocks: SectionBlocks(blocks.into()),
            metrics: Default::default(),
            fluid: None,
            skylight: None,
            blocklight: None,
            states: Default::default(),
        })),
        &mut server_keys.sealer,
    )
    .unwrap();
    let ServerToClient::SectionData(section) = incoming
        .recv_timeout(petramond_util::test_time::TEST_ENDEAVOR_DEADLINE)
        .unwrap()
    else {
        panic!("expected section data")
    };
    assert_eq!(section.metrics, expected);
}
