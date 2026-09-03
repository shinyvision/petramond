//! Registry queries: name↔id resolution (blocks, items, mob species), tag
//! membership, and item row reads. Every call here touches ONLY the
//! process-wide registries — no simulation context, no init window — so the
//! whole domain is legal on ANY instance (server, worldgen workers, client),
//! any time.

use mod_api::{HostCall, HostRet};

use super::guards::batch_guard;

/// The registry-query family (block + item + mob-species resolvers, tag
/// membership, reverse name lookups, item row reads).
pub(super) fn handle_registry_call(call: HostCall) -> HostRet {
    match call {
        HostCall::ResolveBlock { name } => HostRet::Block(
            petramond_world::registry::names()
                .blocks
                .id(&name)
                .map(mod_api::BlockId),
        ),
        HostCall::ResolveItem { name } => HostRet::Item(
            petramond_world::registry::names()
                .items
                .id(&name)
                .map(mod_api::ItemId),
        ),
        // The reverse resolvers answer `None` for unregistered ids — a mod
        // holding a stale id degrades, it is not a protocol break. Their id
        // lists share the sim batch cap (a legitimate batch never exceeds
        // the 256-id space anyway).
        HostCall::BlockNames { blocks } => match batch_guard("BlockNames id", blocks.len()) {
            Some(err) => err,
            None => HostRet::Names(
                blocks
                    .iter()
                    .map(|b| {
                        petramond_world::registry::names()
                            .blocks
                            .name(b.0)
                            .map(str::to_owned)
                    })
                    .collect(),
            ),
        },
        HostCall::ItemNames { items } => match batch_guard("ItemNames id", items.len()) {
            Some(err) => err,
            None => HostRet::Names(
                items
                    .iter()
                    .map(|i| {
                        petramond_world::registry::names()
                            .items
                            .name(i.0)
                            .map(str::to_owned)
                    })
                    .collect(),
            ),
        },
        // Mob species speak their `mobs.json` KEY (the string the whole mob
        // surface already uses); the def table is id-ordered, so id → key is
        // an index and key → id the shared O(1) hash index.
        HostCall::ResolveMob { key } => {
            HostRet::MobKind(crate::mob::by_key(&key).map(|m| mod_api::MobId(m.0)))
        }
        HostCall::MobNames { mobs } => match batch_guard("MobNames id", mobs.len()) {
            Some(err) => err,
            None => HostRet::Names(
                mobs.iter()
                    .map(|m| {
                        crate::mob::defs()
                            .get(m.0 as usize)
                            .map(|d| d.key.to_owned())
                    })
                    .collect(),
            ),
        },
        // Tag membership never interns: a name nothing lists is an empty
        // set, and a query cannot grow the tag table.
        HostCall::BlocksByTag { tag } => {
            HostRet::BlockList(match petramond_world::block::BlockTag::lookup(&tag) {
                Some(t) => petramond_world::block::Block::all()
                    .iter()
                    .filter(|b| b.has_tag(t))
                    .map(|b| mod_api::BlockId(b.id()))
                    .collect(),
                None => Vec::new(),
            })
        }
        HostCall::ItemsByTag { tag } => {
            HostRet::ItemList(match petramond_world::item::ItemTag::lookup(&tag) {
                Some(t) => petramond_world::item::ItemType::all()
                    .iter()
                    .filter(|i| i.has_tag(t))
                    .map(|i| mod_api::ItemId(i.id()))
                    .collect(),
                None => Vec::new(),
            })
        }
        // The row AS A STACK CARRIES IT: the instance data interns to the
        // same variant the inventory would hold, so `ItemStack::tool` applies
        // an augment's override exactly as mining and melee do.
        HostCall::ItemInfo { item, data } => {
            let variant = match super::guards::intern_abi_data("ItemInfo", &data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            HostRet::ItemInfo(petramond_world::item::ItemType::by_name(&item).map(|t| {
                Box::new(item_info_data(
                    &petramond_world::item::ItemStack::with_variant(t, 1, variant),
                ))
            }))
        }
        // The shape-kind resolver: like the block/item/mob resolvers, a key→id
        // lookup over the process-wide registry, unknown key = `None`.
        HostCall::ResolveShape { key } => {
            HostRet::MaybeByte(petramond_world::block::shape_kind_id_by_key(&key))
        }
        // The row-data interop surface: opaque raw JSON a consuming system's
        // key names — same never-interns contract as the tag queries.
        HostCall::ItemDataGet { item, key } => HostRet::Bytes(
            petramond_world::item::ItemType(item.0)
                .data_value(&key)
                .map(|v| v.as_bytes().to_vec()),
        ),
        HostCall::ItemsWithData { key } => HostRet::ItemDataRows(
            petramond_world::item::ItemType::all()
                .iter()
                .filter_map(|i| {
                    i.data_value(&key)
                        .map(|v| (mod_api::ItemId(i.id()), v.to_owned()))
                })
                .collect(),
        ),
        HostCall::BlockDataGet { block, key } => HostRet::Bytes(
            petramond_world::block::Block::from_id(block.0)
                .data_value(&key)
                .map(|v| v.as_bytes().to_vec()),
        ),
        // The block twin of `ItemInfo`: the row's stable harvest facts, with
        // the engine's own material→tool derivation answered rather than
        // re-derived mod-side (the duplicated-constants trap).
        HostCall::BlockInfo { block } => {
            // Registration gates BEFORE any row accessor runs — an
            // unregistered id must answer `None`, never index a row table.
            HostRet::BlockInfo(
                petramond_world::registry::names()
                    .blocks
                    .name(block.0)
                    .map(|_| {
                        let b = petramond_world::block::Block::from_id(block.0);
                        Box::new(mod_api::BlockInfoData {
                            material: material_name(b.material()).to_owned(),
                            hardness: b.hardness(),
                            harvest_tier: b.harvest_tier(),
                            preferred_tool: b.preferred_tool().map(|t| t.name().to_owned()),
                            item: {
                                let item = petramond_world::item::ItemType::from_block(b);
                                (item != petramond_world::item::ItemType::Air)
                                    .then_some(mod_api::ItemId(item.id()))
                            },
                        })
                    }),
            )
        }
        HostCall::BlocksWithData { key } => HostRet::BlockDataRows(
            petramond_world::block::Block::all()
                .iter()
                .filter_map(|b| {
                    b.data_value(&key)
                        .map(|v| (mod_api::BlockId(b.id()), v.to_owned()))
                })
                .collect(),
        ),
        other => HostRet::Error(format!(
            "non-registry call {other:?} mis-routed to handle_registry_call (host bug)"
        )),
    }
}

/// One item row as its ABI crossing — the stable, mod-relevant fields of the
/// `items.json` row (presentation internals stay engine-side).
fn item_info_data(stack: &petramond_world::item::ItemStack) -> mod_api::ItemInfoData {
    let item = stack.item;
    mod_api::ItemInfoData {
        max_stack: item.max_stack_size(),
        fuel_burn_ticks: item.fuel_burn_ticks() as u32,
        tags: item.tags().iter().map(|t| t.name().to_owned()).collect(),
        display_name: item.name().to_owned(),
        block: item.as_block().map(|b| mod_api::BlockId(b.id())),
        tool: stack.tool().map(|t| mod_api::ToolInfoData {
            kind: t.kind.name().to_owned(),
            tier: t.tier,
            speed: t.speed,
            damage: [t.damage.0, t.damage.1],
            knockback: t.knockback,
        }),
        food: item.food().map(|f| mod_api::FoodInfoData {
            eat_ticks: f.eat_ticks,
            effects: f
                .effects
                .iter()
                .map(|&(fx, ticks)| mod_api::FoodEffectData {
                    effect: fx.def().name.to_owned(),
                    ticks,
                })
                .collect(),
        }),
        item_use: item.item_use().map(|u| item_use_key(u).to_owned()),
    }
}

/// The `blocks.json` `material` string a row's parsed class was declared as
/// (the serde snake_case names, answered back verbatim). Exhaustive on
/// purpose: a new material must pick its ABI name here.
fn material_name(m: petramond_world::block::BlockMaterial) -> &'static str {
    use petramond_world::block::BlockMaterial;
    match m {
        BlockMaterial::None => "none",
        BlockMaterial::Dirt => "dirt",
        BlockMaterial::Sand => "sand",
        BlockMaterial::Stone => "stone",
        BlockMaterial::Ore => "ore",
        BlockMaterial::Wood => "wood",
        BlockMaterial::Wool => "wool",
        BlockMaterial::Foliage => "foliage",
        BlockMaterial::Plant => "plant",
        BlockMaterial::Glass => "glass",
        BlockMaterial::Ice => "ice",
        BlockMaterial::Other => "other",
    }
}

/// The `items.json` `use` key a resolved handler was declared as. Wildcard
/// field patterns keep this stable across handler-param reshapes; a NEW
/// handler variant must pick its key here (exhaustive on purpose).
fn item_use_key(u: petramond_world::item::ItemUse) -> &'static str {
    use petramond_world::item::ItemUse;
    match u {
        ItemUse::BucketFill { .. } => "bucket_fill",
        ItemUse::BucketPour { .. } => "bucket_pour",
        ItemUse::Shear => "shear",
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::modding::host::{handle_host_call, ModStoreData};

    /// The registry domain answers OUTSIDE any published SimCtx (legal on
    /// any instance): forward resolution returns the session id, the reverse
    /// resolvers invert it, and unknown names/ids answer `None` — never an
    /// error.
    #[test]
    fn resolvers_answer_without_a_sim_scope_and_invert() {
        let mut store = ModStoreData::new("somemod", 1);
        let got = handle_host_call(
            &mut store,
            HostCall::ResolveItem {
                name: "petramond:stick".into(),
            },
        );
        let HostRet::Item(Some(id)) = got else {
            panic!("expected a resolved id for petramond:stick, got {got:?}");
        };
        assert_eq!(id.0, petramond_world::item::ItemType::Stick.id());
        // id → name inverts the resolution; an out-of-range id is None.
        let names = handle_host_call(
            &mut store,
            HostCall::ItemNames {
                items: vec![id, mod_api::ItemId(u16::MAX)],
            },
        );
        assert_eq!(
            names,
            HostRet::Names(vec![Some("petramond:stick".into()), None])
        );
        let unknown = handle_host_call(
            &mut store,
            HostCall::ResolveItem {
                name: "somemod:not_a_thing".into(),
            },
        );
        assert_eq!(unknown, HostRet::Item(None));

        // The block side mirrors it.
        let got = handle_host_call(
            &mut store,
            HostCall::ResolveBlock {
                name: "petramond:air".into(),
            },
        );
        assert_eq!(got, HostRet::Block(Some(mod_api::BlockId(0))));
        let names = handle_host_call(
            &mut store,
            HostCall::BlockNames {
                blocks: vec![mod_api::BlockId(0), mod_api::BlockId(u16::MAX)],
            },
        );
        assert_eq!(
            names,
            HostRet::Names(vec![Some("petramond:air".into()), None])
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ResolveBlock {
                    name: "no_such:block".into(),
                },
            ),
            HostRet::Block(None)
        );

        // The mob-species side mirrors it (key vocabulary).
        let got = handle_host_call(
            &mut store,
            HostCall::ResolveMob {
                key: "petramond:owl".into(),
            },
        );
        let HostRet::MobKind(Some(kind)) = got else {
            panic!("expected a resolved species id for petramond:owl, got {got:?}");
        };
        assert_eq!(kind.0, crate::mob::Mob::Owl.0);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::MobNames {
                    mobs: vec![kind, mod_api::MobId(u8::MAX)],
                },
            ),
            HostRet::Names(vec![Some("petramond:owl".into()), None])
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ResolveMob {
                    key: "no_such:mob".into(),
                },
            ),
            HostRet::MobKind(None)
        );
    }

    /// `BlocksByTag` is registry-only membership: a tagged block is in, an
    /// untagged one is not, and an unlisted name — bare or namespaced — is an
    /// empty set (the query must never intern a new tag).
    #[test]
    fn blocks_by_tag_enumerates_members_and_never_registers() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::BlockList(leaves) = handle_host_call(
            &mut data,
            HostCall::BlocksByTag {
                tag: "petramond:leaves".into(),
            },
        ) else {
            panic!("block list expected");
        };
        assert!(leaves.contains(&mod_api::BlockId(
            petramond_world::block::Block::OakLeaves.id()
        )));
        assert!(!leaves.contains(&mod_api::BlockId(petramond_world::block::Block::Stone.id())));
        for tag in ["no_such_tag", "mymod:no_such_tag"] {
            assert_eq!(
                handle_host_call(&mut data, HostCall::BlocksByTag { tag: tag.into() }),
                HostRet::BlockList(Vec::new()),
                "unlisted tag '{tag}' must read as an empty set"
            );
        }
    }

    /// `ItemsByTag` is registry-only membership like `BlocksByTag`: a tagged
    /// item is in, an untagged one is not, and an unlisted name — bare or
    /// namespaced — is an empty set (the query must never intern a new tag).
    #[test]
    fn items_by_tag_enumerates_members_and_never_registers() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::ItemList(shovels) = handle_host_call(
            &mut data,
            HostCall::ItemsByTag {
                tag: "petramond:shovels".into(),
            },
        ) else {
            panic!("item list expected");
        };
        let by_name = |name: &str| {
            mod_api::ItemId(
                petramond_world::registry::names()
                    .items
                    .id(name)
                    .expect("engine item registered"),
            )
        };
        assert!(shovels.contains(&by_name("petramond:iron_shovel")));
        assert!(!shovels.contains(&by_name("petramond:stick")));
        for tag in ["no_such_tag", "mymod:no_such_tag"] {
            assert_eq!(
                handle_host_call(&mut data, HostCall::ItemsByTag { tag: tag.into() }),
                HostRet::ItemList(Vec::new()),
                "unlisted tag '{tag}' must read as an empty set"
            );
        }
    }

    /// `ItemInfo` is addressed by registry NAME and exposes the row's
    /// mod-relevant fields (tool/food/block link/use key included), without a
    /// sim scope. Unknown names answer `None`. (No row VALUES are pinned —
    /// only presence/shape of fields the rows structurally guarantee.)
    #[test]
    fn item_info_reads_the_row_by_registry_name() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::ItemInfo(Some(info)) = handle_host_call(
            &mut data,
            HostCall::ItemInfo {
                item: "petramond:iron_pickaxe".into(),
                data: vec![],
            },
        ) else {
            panic!("item info expected");
        };
        let tool = info.tool.expect("a pickaxe row declares a tool");
        assert_eq!(tool.kind, "pickaxe");
        assert_eq!(info.max_stack, 1, "durable items never stack");
        assert!(!info.display_name.is_empty());

        let HostRet::ItemInfo(Some(stone)) = handle_host_call(
            &mut data,
            HostCall::ItemInfo {
                item: "petramond:stone".into(),
                data: vec![],
            },
        ) else {
            panic!("item info expected");
        };
        assert_eq!(
            stone.block,
            Some(mod_api::BlockId(petramond_world::block::Block::Stone.id())),
            "a placeable item exposes its block link"
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ItemInfo {
                    item: "alpha:not_a_thing".into(),
                    data: vec![],
                },
            ),
            HostRet::ItemInfo(None)
        );
    }
}
