use super::*;

const MOSS: BlockId = BlockId(200);
const STONE: BlockId = BlockId(3);

fn dark_site() -> HostileSpawnCandidate {
    HostileSpawnCandidate {
        pos: [8.5, 20.0, 8.5],
        cell: [8, 20, 8],
        combined_light: 0,
        sky_light: 0,
        block_light: 0,
        nearest_player_dist: 40.0,
    }
}

fn alone(_pos: [f64; 3], _radius: f32) -> Vec<MobSnapshot> {
    Vec::new()
}

fn no_claim() -> u64 {
    HUSHJAW_CLAIM_PER_100
}

fn species_over(proof: &SpawnProof, ground: Option<BlockId>) -> Option<&'static str> {
    site_species(
        &dark_site(),
        1.0,
        proof,
        Species::default(),
        &|| ground,
        &no_claim,
        &alone,
    )
}

#[test]
fn a_spawn_proof_floor_refuses_a_site_that_is_otherwise_perfect() {
    let proof = SpawnProof::new(vec![MOSS]);
    assert_eq!(species_over(&proof, Some(MOSS)), None, "moss refuses");
    assert_eq!(
        species_over(&proof, Some(STONE)),
        Some(keys::ZOMBIE),
        "the site was otherwise perfect, so the FLOOR is what refused it"
    );
    assert_eq!(
        species_over(&proof, None),
        None,
        "an unreadable floor refuses"
    );
    let lit = HostileSpawnCandidate {
        block_light: 30,
        ..dark_site()
    };
    let stone = || Some(STONE);
    assert_eq!(
        site_species(
            &lit,
            1.0,
            &proof,
            Species::default(),
            &stone,
            &no_claim,
            &alone
        ),
        None,
        "a lit site is still refused"
    );
}

#[test]
fn spawn_proof_membership_covers_wide_and_unmarked_block_ids() {
    let marked = BlockId(300);
    let proof = SpawnProof::new(vec![marked, marked]);
    assert_eq!(proof.count, 1);
    assert!(proof.refuses(&|| Some(marked)));
    assert!(!proof.refuses(&|| Some(BlockId(u16::MAX))));
    let highest = SpawnProof::new(vec![BlockId(u16::MAX)]);
    assert!(highest.refuses(&|| Some(BlockId(u16::MAX))));
}

#[test]
fn an_unmarked_world_neither_changes_behaviour_nor_reads_the_floor() {
    let reads = std::cell::Cell::new(0u32);
    let ground = || {
        reads.set(reads.get() + 1);
        Some(MOSS)
    };
    let none = SpawnProof::default();
    assert_eq!(
        site_species(
            &dark_site(),
            1.0,
            &none,
            Species::default(),
            &ground,
            &no_claim,
            &alone
        ),
        Some(keys::ZOMBIE),
        "with nothing tagged, every previously-good site is still good"
    );
    assert_eq!(
        reads.get(),
        0,
        "and the world is never read — no pack pays for a rule it does not use"
    );
}
