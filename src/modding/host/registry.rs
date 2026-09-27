use mod_api::{HostRet, RegistryCall};

use super::guards::batch_guard;

pub(super) fn handle_registry_call(call: RegistryCall) -> HostRet {
    match call {
        RegistryCall::BlockRecordPlans { records } => {
            if let Some(err) = batch_guard("BlockRecordPlans record", records.len()) {
                return err;
            }
            HostRet::RecordPlans(records.iter().map(super::construction::plan_out).collect())
        }
        RegistryCall::MobDataGet { mob, key } => HostRet::Bytes(
            crate::mob::defs()
                .get(mob.0 as usize)
                .and_then(|def| def.data_value(&key))
                .map(|value| value.as_bytes().to_vec()),
        ),
        RegistryCall::MobsWithData { key } => HostRet::MobDataRows(
            crate::mob::defs()
                .iter()
                .filter_map(|def| {
                    def.data_value(&key)
                        .map(|value| (mod_api::MobId(def.mob.id()), value.to_owned()))
                })
                .collect(),
        ),
        RegistryCall::LootRoll { key, mut seed } => HostRet::Loot(
            petramond_world::loot::catalog()
                .roll(&key, || super::splitmix_next(&mut seed))
                .map(|stacks| {
                    stacks
                        .iter()
                        .map(crate::modding::convert::item_stack_out)
                        .collect()
                }),
        ),
        RegistryCall::StructureInfo { key } => HostRet::StructureInfo(
            petramond_world::structure::by_key(&key).map(|template| Box::new(template.info())),
        ),
        RegistryCall::ResolveBlock { name } => HostRet::Block(
            petramond_world::registry::names()
                .blocks
                .id(&name)
                .map(mod_api::BlockId),
        ),
        RegistryCall::ResolveItem { name } => HostRet::Item(
            petramond_world::registry::names()
                .items
                .id(&name)
                .map(mod_api::ItemId),
        ),
        RegistryCall::BlockNames { blocks } => match batch_guard("BlockNames id", blocks.len()) {
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
        RegistryCall::ItemNames { items } => match batch_guard("ItemNames id", items.len()) {
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
        RegistryCall::ResolveMob { key } => {
            HostRet::MobKind(crate::mob::by_key(&key).map(|m| mod_api::MobId(m.0)))
        }
        RegistryCall::ResolveCondition { key } => {
            HostRet::Condition(petramond_world::condition::by_name(&key).map(|id| {
                let def = id.def();
                mod_api::ConditionInfoData {
                    id: mod_api::ConditionId(id.0),
                    key: def.name.to_owned(),
                    stages: def.stages.iter().map(|s| s.name.to_owned()).collect(),
                }
            }))
        }
        RegistryCall::ConditionNames { conditions } => {
            match batch_guard("ConditionNames id", conditions.len()) {
                Some(err) => err,
                None => HostRet::Names(
                    conditions
                        .iter()
                        .map(|c| {
                            petramond_world::condition::defs()
                                .get(c.0 as usize)
                                .map(|d| d.name.to_owned())
                        })
                        .collect(),
                ),
            }
        }
        RegistryCall::MobNames { mobs } => match batch_guard("MobNames id", mobs.len()) {
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
        RegistryCall::BlocksByTag { tag } => {
            HostRet::BlockList(match petramond_world::block::BlockTag::lookup(&tag) {
                Some(t) => petramond_world::block::Block::all()
                    .iter()
                    .filter(|b| b.has_tag(t))
                    .map(|b| mod_api::BlockId(b.id()))
                    .collect(),
                None => Vec::new(),
            })
        }
        RegistryCall::ItemsByTag { tag } => {
            HostRet::ItemList(match petramond_world::item::ItemTag::lookup(&tag) {
                Some(t) => petramond_world::item::ItemType::all()
                    .iter()
                    .filter(|i| i.has_tag(t))
                    .map(|i| mod_api::ItemId(i.id()))
                    .collect(),
                None => Vec::new(),
            })
        }
        RegistryCall::ItemInfo { item, data } => {
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
        RegistryCall::ResolveShape { key } => {
            HostRet::MaybeU16(petramond_world::block::shape_kind_id_by_key(&key))
        }
        RegistryCall::ItemDataGet { item, key } => HostRet::Bytes(
            petramond_world::item::ItemType(item.0)
                .data_value(&key)
                .map(|v| v.as_bytes().to_vec()),
        ),
        RegistryCall::ItemsWithData { key } => HostRet::ItemDataRows(
            petramond_world::item::ItemType::all()
                .iter()
                .filter_map(|i| {
                    i.data_value(&key)
                        .map(|v| (mod_api::ItemId(i.id()), v.to_owned()))
                })
                .collect(),
        ),
        RegistryCall::BlockDataGet { block, key } => HostRet::Bytes(
            petramond_world::block::Block::from_id(block.0)
                .data_value(&key)
                .map(|v| v.as_bytes().to_vec()),
        ),
        RegistryCall::BlockInfo { block } => {
            HostRet::BlockInfo(block_info_data(block).map(Box::new))
        }
        RegistryCall::BlockInfos { blocks } => match batch_guard("BlockInfos id", blocks.len()) {
            Some(err) => err,
            None => HostRet::BlockInfos(blocks.into_iter().map(block_info_data).collect()),
        },
        RegistryCall::BlocksWithData { key } => HostRet::BlockDataRows(
            petramond_world::block::Block::all()
                .iter()
                .filter_map(|b| {
                    b.data_value(&key)
                        .map(|v| (mod_api::BlockId(b.id()), v.to_owned()))
                })
                .collect(),
        ),
    }
}

fn block_info_data(block: mod_api::BlockId) -> Option<mod_api::BlockInfoData> {
    petramond_world::registry::names().blocks.name(block.0)?;
    let b = petramond_world::block::Block::from_id(block.0);
    Some(mod_api::BlockInfoData {
        material: material_name(b.material()).to_owned(),
        hardness: b.hardness(),
        harvest_tier: b.harvest_tier(),
        preferred_tool: b.preferred_tool().map(|t| t.name().to_owned()),
        item: {
            let item = petramond_world::item::ItemType::from_block(b);
            (item != petramond_world::item::ItemType::Air).then_some(mod_api::ItemId(item.id()))
        },
        collision: b.collision_boxes().iter().map(|a| (a.min, a.max)).collect(),
        fluid: b.fluid_def().map(fluid_info),
        replaceable: b.is_replaceable(),
        interaction: match b.interaction() {
            petramond_world::block::BlockInteraction::None => None,
            petramond_world::block::BlockInteraction::OpenGui(_) => {
                Some(mod_api::BlockUse::OpenGui)
            }
            petramond_world::block::BlockInteraction::ToggleDoor => {
                Some(mod_api::BlockUse::ToggleDoor)
            }
            petramond_world::block::BlockInteraction::ToggleTrapdoor => {
                Some(mod_api::BlockUse::ToggleTrapdoor)
            }
            petramond_world::block::BlockInteraction::Sleep => Some(mod_api::BlockUse::Sleep),
        },
    })
}

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

fn material_name(m: petramond_world::block::BlockMaterial) -> &'static str {
    use petramond_world::block::BlockMaterial;
    match m {
        BlockMaterial::None => "none",
        BlockMaterial::Dirt => "dirt",
        BlockMaterial::Sand => "sand",
        BlockMaterial::Snow => "snow",
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

fn item_use_key(u: petramond_world::item::ItemUse) -> &'static str {
    use petramond_world::item::ItemUse;
    match u {
        ItemUse::BucketFill { .. } => "bucket_fill",
        ItemUse::BucketPour { .. } => "bucket_pour",
        ItemUse::Shear => "shear",
    }
}

fn fluid_info(f: &petramond_world::fluid::FluidDef) -> mod_api::FluidInfoData {
    let condition = |c: petramond_world::condition::ConditionId| mod_api::ConditionId(c.0);
    mod_api::FluidInfoData {
        delay: f.delay,
        drop_off: f.drop_off,
        renewable: f.renewable,
        quench: f.quench.map(|q| mod_api::QuenchData {
            by: mod_api::BlockId(q.by.id()),
            result: mod_api::BlockId(q.result.id()),
        }),
        contact_damage: f.contact.damage.map(|p| mod_api::PulseData {
            amount: p.amount,
            interval: p.interval,
        }),
        applies: f.contact.applies.map(|g| mod_api::ConditionGrantData {
            condition: condition(g.condition),
            stage: g.stage,
            ticks: g.ticks,
        }),
        clears: f.contact.clears.iter().copied().map(condition).collect(),
        destroys_items: f.contact.destroys_items,
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, HostCall, HostRet};

    use crate::modding::host::{handle_host_call, ModStoreData};

    #[test]
    fn resolvers_answer_without_a_sim_scope_and_invert() {
        let mut store = ModStoreData::new("somemod", 1);
        let got = handle_host_call(
            &mut store,
            HostCall::from(calls::ResolveItem {
                name: "petramond:stick".into(),
            }),
        );
        let HostRet::Item(Some(id)) = got else {
            panic!("expected a resolved id for petramond:stick, got {got:?}");
        };
        assert_eq!(id.0, petramond_world::item::ItemType::Stick.id());
        let names = handle_host_call(
            &mut store,
            HostCall::from(calls::ItemNames {
                items: vec![id, mod_api::ItemId(u16::MAX)],
            }),
        );
        assert_eq!(
            names,
            HostRet::Names(vec![Some("petramond:stick".into()), None])
        );
        let unknown = handle_host_call(
            &mut store,
            HostCall::from(calls::ResolveItem {
                name: "somemod:not_a_thing".into(),
            }),
        );
        assert_eq!(unknown, HostRet::Item(None));

        let got = handle_host_call(
            &mut store,
            HostCall::from(calls::ResolveBlock {
                name: "petramond:air".into(),
            }),
        );
        assert_eq!(got, HostRet::Block(Some(mod_api::BlockId(0))));
        let names = handle_host_call(
            &mut store,
            HostCall::from(calls::BlockNames {
                blocks: vec![mod_api::BlockId(0), mod_api::BlockId(u16::MAX)],
            }),
        );
        assert_eq!(
            names,
            HostRet::Names(vec![Some("petramond:air".into()), None])
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::ResolveBlock {
                    name: "no_such:block".into(),
                }),
            ),
            HostRet::Block(None)
        );

        let got = handle_host_call(
            &mut store,
            HostCall::from(calls::ResolveMob {
                key: "petramond:owl".into(),
            }),
        );
        let HostRet::MobKind(Some(kind)) = got else {
            panic!("expected a resolved species id for petramond:owl, got {got:?}");
        };
        assert_eq!(kind.0, crate::mob::Mob::Owl.0);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::MobNames {
                    mobs: vec![kind, mod_api::MobId(u8::MAX)],
                }),
            ),
            HostRet::Names(vec![Some("petramond:owl".into()), None])
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::ResolveMob {
                    key: "no_such:mob".into(),
                }),
            ),
            HostRet::MobKind(None)
        );
    }

    #[test]
    fn blocks_by_tag_enumerates_members_and_never_registers() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::BlockList(leaves) = handle_host_call(
            &mut data,
            HostCall::from(calls::BlocksByTag {
                tag: "petramond:leaves".into(),
            }),
        ) else {
            panic!("block list expected");
        };
        assert!(leaves.contains(&mod_api::BlockId(
            petramond_world::block::Block::OakLeaves.id()
        )));
        assert!(!leaves.contains(&mod_api::BlockId(petramond_world::block::Block::Stone.id())));
        for tag in ["no_such_tag", "mymod:no_such_tag"] {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::BlocksByTag { tag: tag.into() })
                ),
                HostRet::BlockList(Vec::new()),
                "unlisted tag '{tag}' must read as an empty set"
            );
        }
    }

    #[test]
    fn items_by_tag_enumerates_members_and_never_registers() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::ItemList(shovels) = handle_host_call(
            &mut data,
            HostCall::from(calls::ItemsByTag {
                tag: "petramond:shovels".into(),
            }),
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
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::ItemsByTag { tag: tag.into() })
                ),
                HostRet::ItemList(Vec::new()),
                "unlisted tag '{tag}' must read as an empty set"
            );
        }
    }

    #[test]
    fn item_info_reads_the_row_by_registry_name() {
        let mut data = ModStoreData::new("alpha", 1);
        let HostRet::ItemInfo(Some(info)) = handle_host_call(
            &mut data,
            HostCall::from(calls::ItemInfo {
                item: "petramond:iron_pickaxe".into(),
                data: vec![],
            }),
        ) else {
            panic!("item info expected");
        };
        let tool = info.tool.expect("a pickaxe row declares a tool");
        assert_eq!(tool.kind, "pickaxe");
        assert_eq!(info.max_stack, 1, "durable items never stack");
        assert!(!info.display_name.is_empty());

        let HostRet::ItemInfo(Some(stone)) = handle_host_call(
            &mut data,
            HostCall::from(calls::ItemInfo {
                item: "petramond:stone".into(),
                data: vec![],
            }),
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
                HostCall::from(calls::ItemInfo {
                    item: "alpha:not_a_thing".into(),
                    data: vec![],
                }),
            ),
            HostRet::ItemInfo(None)
        );
    }
}
