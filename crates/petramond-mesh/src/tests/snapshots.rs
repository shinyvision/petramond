use super::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const SNAPSHOT_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/tests/snapshots/mesh_digests.txt"
);
const BLESS_VAR: &str = "PETRAMOND_BLESS_MESH";

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn digest_lines(case: &str, mesh: &ChunkMesh, out: &mut BTreeMap<String, String>) {
    let mut put = |stream: &str, len: usize, bytes: &[u8]| {
        out.insert(
            format!("{case} {stream}"),
            format!("{len} {:016x}", fnv1a(bytes)),
        );
    };
    let verts = |v: &[Vertex]| bytemuck::cast_slice::<Vertex, u8>(v).to_vec();
    put("opaque", mesh.opaque.len(), &verts(&mesh.opaque));
    put(
        "far_opaque_len",
        mesh.far_opaque_len as usize,
        &mesh.far_opaque_len.to_le_bytes(),
    );
    put(
        "transparent",
        mesh.transparent.len(),
        &verts(&mesh.transparent),
    );
    put(
        "transparent_two_sided",
        mesh.transparent_two_sided.len(),
        &verts(&mesh.transparent_two_sided),
    );
    put(
        "translucent",
        mesh.translucent.len(),
        &verts(&mesh.translucent),
    );
    put(
        "model",
        mesh.model.len(),
        bytemuck::cast_slice::<ModelVertex, u8>(&mesh.model),
    );
    put(
        "model_idx",
        mesh.model_idx.len(),
        bytemuck::cast_slice::<u32, u8>(&mesh.model_idx),
    );
    put(
        "model_blend_idx",
        mesh.model_blend_idx.len(),
        bytemuck::cast_slice::<u32, u8>(&mesh.model_blend_idx),
    );
    put(
        "contact",
        mesh.contact.len(),
        bytemuck::cast_slice::<crate::ContactShadowVertex, u8>(&mesh.contact),
    );
}

fn cases() -> Vec<(String, ChunkMesh)> {
    let mut out = Vec::new();
    let (section, scene) = fixtures::showcase();
    out.push((
        "showcase".to_owned(),
        scene.mesh(&section, SectionPos::new(0, 0, 0)),
    ));
    for (pos, section, world) in fixtures::generated_sections() {
        out.push((
            format!("generated_{}_{}_{}", pos.cx, pos.cy, pos.cz),
            world.mesh(&section, pos),
        ));
    }
    for (i, section) in fixtures::catalog_sections().into_iter().enumerate() {
        out.push((
            format!("catalog_{i}"),
            fixtures::standalone(&section).mesh(&section, SectionPos::new(0, 0, 0)),
        ));
    }
    out
}

fn render(digests: &BTreeMap<String, String>) -> String {
    let mut s = String::from(
        "# Mesh digests (see src/tests/snapshots.rs). Re-bless with PETRAMOND_BLESS_MESH=1.\n",
    );
    for (k, v) in digests {
        let _ = writeln!(s, "{k} {v}");
    }
    s
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut parts = l.rsplitn(3, ' ');
            let digest = parts.next()?;
            let len = parts.next()?;
            let key = parts.next()?;
            Some((key.to_owned(), format!("{len} {digest}")))
        })
        .collect()
}

#[test]
fn representative_section_meshes_match_their_golden_digests() {
    let packs = petramond_world::assets::PackSet::discover(
        &petramond_world::assets::PackRoots::with_mods(Vec::new()),
    );
    let content = petramond_world::content::ContentLoader::new(packs)
        .stages(&petramond_worldgen::data::content_stages())
        .load()
        .expect("base mesh snapshot content loads");
    let _pin = petramond_world::content::pin(content);
    let mut digests = BTreeMap::new();
    for (case, mesh) in cases() {
        digest_lines(&case, &mesh, &mut digests);
    }
    let bless = std::env::var_os(BLESS_VAR).is_some_and(|v| v == "1");
    let golden = std::fs::read_to_string(SNAPSHOT_FILE).ok();
    let Some(golden) = golden.filter(|_| !bless) else {
        std::fs::create_dir_all(
            std::path::Path::new(SNAPSHOT_FILE)
                .parent()
                .expect("snapshot dir"),
        )
        .expect("create snapshot dir");
        std::fs::write(SNAPSHOT_FILE, render(&digests)).expect("write mesh digests");
        eprintln!(
            "blessed {} mesh digests into {SNAPSHOT_FILE}",
            digests.len()
        );
        return;
    };
    let golden = parse(&golden);
    let mismatches: Vec<String> = digests
        .iter()
        .filter(|(k, v)| golden.get(*k) != Some(*v))
        .map(|(k, v)| {
            format!(
                "{k}: got {v}, golden {}",
                golden.get(k).map_or("<missing>", String::as_str)
            )
        })
        .chain(
            golden
                .keys()
                .filter(|k| !digests.contains_key(*k))
                .map(|k| format!("{k}: golden case no longer produced")),
        )
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} mesh digest(s) changed (re-bless with {BLESS_VAR}=1 if intended):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
