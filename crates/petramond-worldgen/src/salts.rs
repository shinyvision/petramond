//! The positional-RNG salt registry.
//!
//! [`FeatureRng::positional`](crate::rng::FeatureRng::positional) streams are
//! independent only while their salts differ: two consumers sharing a salt
//! (and coordinates) draw the SAME numbers, so ore placement could mirror
//! vegetation, or one tree stream shadow another. Every engine stream's salt
//! therefore lives here, in [`ENGINE`], checked unique by a test — together
//! with every salt the shipped catalogs resolve, and the xor modifiers the
//! engine folds into data salts.
//!
//! Data rows never need a hand-picked integer: a row's salt derives from its
//! namespaced name through the functions below, one derivation per row kind.
//! The derivations are frozen (worldgen output depends on their bits), so a
//! new row kind adds a new function rather than changing one.
//!
//! Salts are frozen literals: changing one changes world bytes.

/// The tree-feature stream: density, spacing and geometry draws.
pub(crate) const TREE_FEATURE: u64 = 0x0000_7A3E_0AC0_FFEE;
/// Ties between nearby tree candidates.
pub(crate) const TREE_PRIORITY: u64 = 0x0000_7A3E_51AC_1EAF;
/// The fallen-branch scatter, so branches do not move when a tree's geometry
/// draws change (and vice versa).
pub(crate) const TREE_BRANCH: u64 = 0x0000_7A3E_B4A0_C401;
/// Xor into a grove lattice's salt for its detail lattice, so it never
/// mirrors the broad one.
pub(crate) const GROVE_DETAIL_XOR: u64 = 0xD37A_11ED_670E_0001;
/// Xor into a grove lattice's salt for the per-site claim draw: territories
/// must not consume the density, spacing or geometry streams.
pub(crate) const GROVE_CHOICE_XOR: u64 = 0x0000_7A3E_670E_C401;
/// Per-column ground vegetation.
pub(crate) const VEGETATION: u64 = 0x0000_5EED_1EAF_0001;
/// The flower-patch SPECIES field (which one flower a patch is made of).
pub(crate) const FLOWER_PATCH_TYPE: u64 = 0x0000_F10E_7376_0001;
/// The flower-patch PRESENCE field (where flower patches occur at all).
pub(crate) const FLOWER_PATCH_PRESENCE: u64 = 0x0000_B10C_7376_0001;
/// Hemp stand anchors and their walks.
pub(crate) const HEMP_ANCHOR: u64 = 0x0000_4E4D_7038_0001;
/// The sea-ice edge cluster field.
pub(crate) const SEA_ICE_EDGE: u64 = 0x0000_5EA1_CE00_0001;
/// The branching cave-walk profile.
pub(crate) const CAVE_WALK_BRANCHING: u64 = 0xB40A;
/// The steep, unbranched cave-walk profile.
pub(crate) const CAVE_WALK_STEEP: u64 = 0xB40B;
/// Xor into an excavation row's salt for the passages between its rooms.
pub(crate) const EXCAVATION_PASSAGE_XOR: u64 = 0x5041_5353_4147_4500;
/// The feature preview tool's placement roll.
pub(crate) const FEATURE_PREVIEW: u64 = 0x0000_FE47_0000_0001;

/// Every engine salt, named, for the uniqueness check.
pub(crate) const ENGINE: &[(&str, u64)] = &[
    ("tree feature", TREE_FEATURE),
    ("tree priority", TREE_PRIORITY),
    ("tree branch", TREE_BRANCH),
    ("grove detail xor", GROVE_DETAIL_XOR),
    ("grove choice xor", GROVE_CHOICE_XOR),
    ("vegetation", VEGETATION),
    ("flower patch type", FLOWER_PATCH_TYPE),
    ("flower patch presence", FLOWER_PATCH_PRESENCE),
    ("hemp anchor", HEMP_ANCHOR),
    ("sea ice edge", SEA_ICE_EDGE),
    ("cave walk branching", CAVE_WALK_BRANCHING),
    ("cave walk steep", CAVE_WALK_STEEP),
    ("excavation passage xor", EXCAVATION_PASSAGE_XOR),
    ("feature preview", FEATURE_PREVIEW),
];

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 over `bytes`: the stable, platform-independent hash every
/// name-derived salt is built from.
pub(crate) const fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    let mut i = 0;
    while i < bytes.len() {
        h = (h ^ bytes[i] as u64).wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}

/// A row salt from its kind and namespaced name: `fnv64("kind:name")`. The
/// derivation for fluid pools and falls, and for every row kind added since
/// (ore veins without a pinned salt).
pub(crate) fn named(kind: &str, name: &str) -> u64 {
    fnv64(format!("{kind}:{name}").as_bytes())
}

/// An excavation row's salt: the hash of its unique name.
pub(crate) fn excavation(name: &str) -> u64 {
    fnv64(name.as_bytes())
}

/// A grove field's salt: the hash of the field name, so two tree rules that
/// name the same field share a pattern.
pub(crate) fn grove_field(field: &str) -> u64 {
    fnv64(field.as_bytes())
}

/// An underground biome's lining dither salt: its name under a `lining:`
/// prefix of its own.
pub(crate) fn lining(name: &str) -> u64 {
    fnv64(b"lining:").wrapping_mul(FNV_PRIME) ^ fnv64(name.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use petramond_world::biome::Biome;

    use super::*;
    use crate::biome::trees::Territory;
    use crate::surface::rule::{SurfaceCond, SurfaceRule};

    /// Every `ClusterNoiseBelow` salt in a surface rule tree.
    fn cluster_salts(rule: &SurfaceRule, out: &mut impl FnMut(u64)) {
        match rule {
            SurfaceRule::Block(_) => {}
            SurfaceRule::Sequence(rules) => {
                for rule in rules.iter() {
                    cluster_salts(rule, out);
                }
            }
            SurfaceRule::Condition { when, then } => {
                if let SurfaceCond::ClusterNoiseBelow { salt, .. } = when {
                    out(*salt);
                }
                cluster_salts(then, out);
            }
        }
    }

    /// Every salt the engine and the loaded catalogs draw positional streams
    /// from, by owner. A grove field or cluster is listed once however many
    /// rows carry it: rows naming the same one share its pattern by contract.
    fn loaded_salts() -> Vec<(String, u64)> {
        let mut all: Vec<(String, u64)> = ENGINE
            .iter()
            .map(|&(name, salt)| (format!("engine {name}"), salt))
            .collect();
        for vein in crate::data::ores::table().veins {
            all.push((format!("ore vein {:?}", vein.block), vein.salt));
        }
        for row in &crate::data::excavations::table().rows {
            all.push((format!("excavation {:#x}", row.salt), row.salt));
        }
        let underground = crate::data::underground::table();
        for id in 0..=u8::MAX {
            if let Some(faces) = underground.faces(id) {
                all.push((format!("underground biome {id} lining"), faces.salt));
            }
        }
        for pool in underground.pools.iter() {
            all.push((format!("fluid pool {:#x}", pool.salt), pool.salt));
        }
        for fall in underground.falls.iter() {
            // A fall draws from its salt and the next one up.
            all.push((format!("fluid fall {:#x}", fall.salt), fall.salt));
            all.push((format!("fluid fall {:#x} + 1", fall.salt), fall.salt + 1));
        }
        let mut shared: BTreeSet<(&str, u64)> = BTreeSet::new();
        for biome in Biome::all() {
            for rule in crate::biome::trees::profile(biome).rules.iter() {
                if let Territory::Grove(lattice) = &rule.territory {
                    shared.insert(("grove field", lattice.salt));
                }
            }
            let spec = crate::biome::spec(biome);
            cluster_salts(spec.surface, &mut |salt| {
                shared.insert(("surface cluster", salt));
            });
            if let Some(cluster) = spec.vegetation.cover_cluster {
                shared.insert(("cover cluster", cluster.salt));
            }
        }
        all.extend(
            shared
                .into_iter()
                .map(|(kind, salt)| (format!("{kind} {salt:#x}"), salt)),
        );
        all
    }

    #[test]
    fn no_two_streams_share_a_salt() {
        let mut owners: HashMap<u64, String> = HashMap::new();
        for (owner, salt) in loaded_salts() {
            if let Some(first) = owners.insert(salt, owner.clone()) {
                panic!("{owner} and {first} share salt {salt:#x}");
            }
        }
    }

    #[test]
    fn derivations_are_frozen() {
        // FNV-1a 64 reference vectors.
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(named("fluid_pool", "x"), fnv64(b"fluid_pool:x"));
        assert_eq!(excavation("petramond:a"), fnv64(b"petramond:a"));
        assert_ne!(lining("petramond:a"), excavation("petramond:a"));
    }
}
