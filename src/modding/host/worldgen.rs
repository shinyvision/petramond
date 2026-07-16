//! Worldgen calls: registry-only block resolution (legal on any instance)
//! and the init-window-gated gen hook registrations.

use mod_api::{HostCall, HostRet};

use super::{ModStoreData, Registration};

/// Worldgen-hook calls (block-name resolution plus the gen registrations).
pub(super) fn handle_worldgen_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    match call {
        // ResolveBlock reads only the process-wide registry, so it is legal on
        // ANY instance — worldgen worker instances (which never get a SimCtx)
        // resolve their block ids during their own `mod_init`.
        HostCall::ResolveBlock { key } => HostRet::Block(
            crate::registry::names()
                .blocks
                .id(&key)
                .map(mod_api::BlockId),
        ),
        HostCall::RegisterWorldgenFeature { feature_id, stage } => {
            if stage == mod_api::WorldgenStage::Climate {
                return HostRet::Error(
                    "worldgen features cannot attach after the climate stage (it is \
                     column-level, before any blocks exist); use Terrain or later"
                        .into(),
                );
            }
            data.register(Registration::WorldgenFeature { stage, feature_id })
        }
        HostCall::RegisterStageReplacement { stage, callback_id } => {
            data.register(Registration::StageReplacement { stage, callback_id })
        }
        HostCall::RegisterGenerator { callback_id } => {
            data.register(Registration::Generator { callback_id })
        }
        other => HostRet::Error(format!(
            "non-worldgen call {other:?} mis-routed to handle_worldgen_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::modding::host::{handle_host_call, ModStoreData, Phase, Registration};

    /// Worldgen hook registration is `mod_init`-window-gated like every other
    /// registration, and `Climate` is not a feature attach point (features
    /// write blocks; climate is column-level). `ResolveBlock` needs no window
    /// and no simulation scope — it must work on worldgen instances.
    #[test]
    fn gen_registrations_gate_on_the_init_window() {
        let mut data = ModStoreData::new("alpha", 1);
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::RegisterWorldgenFeature {
                    feature_id: 1,
                    stage: mod_api::WorldgenStage::Trees,
                },
            ),
            HostRet::Unit
        );
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::RegisterWorldgenFeature {
                    feature_id: 2,
                    stage: mod_api::WorldgenStage::Climate,
                },
            ),
            HostRet::Error(_)
        ));
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::RegisterStageReplacement {
                    stage: mod_api::WorldgenStage::Terrain,
                    callback_id: 3,
                },
            ),
            HostRet::Unit
        );
        assert_eq!(
            handle_host_call(&mut data, HostCall::RegisterGenerator { callback_id: 4 }),
            HostRet::Unit
        );
        assert_eq!(data.stats.registered, 3);
        assert!(data.pending.iter().all(Registration::is_gen));

        // Outside the window every gen registration is rejected...
        data.phase = Phase::Run;
        for call in [
            HostCall::RegisterWorldgenFeature {
                feature_id: 1,
                stage: mod_api::WorldgenStage::Trees,
            },
            HostCall::RegisterStageReplacement {
                stage: mod_api::WorldgenStage::Terrain,
                callback_id: 3,
            },
            HostCall::RegisterGenerator { callback_id: 4 },
        ] {
            assert!(matches!(
                handle_host_call(&mut data, call),
                HostRet::Error(_)
            ));
        }
        assert_eq!(data.stats.rejected_registrations, 3);
        // ...but ResolveBlock works anywhere, with no SimCtx published.
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ResolveBlock {
                    key: "petramond:air".into()
                },
            ),
            HostRet::Block(Some(mod_api::BlockId(0)))
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ResolveBlock {
                    key: "no_such:block".into(),
                },
            ),
            HostRet::Block(None)
        );
    }
}
