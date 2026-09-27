pub(crate) const TREE_FEATURE: u64 = 0x0000_7A3E_0AC0_FFEE;
pub(crate) const TREE_PRIORITY: u64 = 0x0000_7A3E_51AC_1EAF;
pub(crate) const TREE_BRANCH: u64 = 0x0000_7A3E_B4A0_C401;
pub(crate) const GROVE_DETAIL_XOR: u64 = 0xD37A_11ED_670E_0001;
pub(crate) const GROVE_CHOICE_XOR: u64 = 0x0000_7A3E_670E_C401;
pub(crate) const VEGETATION: u64 = 0x0000_5EED_1EAF_0001;
pub(crate) const FLOWER_PATCH_TYPE: u64 = 0x0000_F10E_7376_0001;
pub(crate) const FLOWER_PATCH_PRESENCE: u64 = 0x0000_B10C_7376_0001;
pub(crate) const HEMP_ANCHOR: u64 = 0x0000_4E4D_7038_0001;
pub(crate) const SEA_ICE_EDGE: u64 = 0x0000_5EA1_CE00_0001;
pub(crate) const CAVE_WALK_BRANCHING: u64 = 0xB40A;
pub(crate) const CAVE_WALK_STEEP: u64 = 0xB40B;
pub(crate) const EXCAVATION_PASSAGE_XOR: u64 = 0x5041_5353_4147_4500;
#[cfg(any(test, feature = "tools"))]
pub(crate) const FEATURE_PREVIEW: u64 = 0x0000_FE47_0000_0001;

#[cfg(test)]
const ENGINE: &[(&str, u64)] = &[
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

pub(crate) const fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    let mut i = 0;
    while i < bytes.len() {
        h = (h ^ bytes[i] as u64).wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}

pub(crate) fn named(kind: &str, name: &str) -> u64 {
    fnv64(format!("{kind}:{name}").as_bytes())
}

pub(crate) fn excavation(name: &str) -> u64 {
    fnv64(name.as_bytes())
}

pub(crate) fn grove_field(field: &str) -> u64 {
    fnv64(field.as_bytes())
}

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
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(named("fluid_pool", "x"), fnv64(b"fluid_pool:x"));
        assert_eq!(excavation("petramond:a"), fnv64(b"petramond:a"));
        assert_ne!(lining("petramond:a"), excavation("petramond:a"));
    }
}
