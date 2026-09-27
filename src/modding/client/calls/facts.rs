use std::collections::BTreeSet;
use std::sync::OnceLock;

use mod_api::{ClientCall, ClientEngineFactsData, ClientPackInfo, ClientWallTime, HostRet};

use crate::modding::client::state::ClientStoreData;

fn vocabulary() -> u64 {
    static VOCABULARY: OnceLock<u64> = OnceLock::new();
    *VOCABULARY.get_or_init(|| {
        let tables = crate::net::remap::local_name_tables();
        mod_api::capture::fnv1a64(&postcard::to_allocvec(&tables).expect("name tables encode"))
    })
}

pub(super) fn handle(client: &ClientStoreData, guest_memory_max: u64, call: ClientCall) -> HostRet {
    match call {
        ClientCall::ClientEngineFacts => {
            let limits = client.presented.lock().frame_limits;
            HostRet::ClientEngineFacts(ClientEngineFactsData {
                capture_format: mod_api::capture::CAPTURE_FORMAT,
                protocol: crate::net::PROTOCOL_VERSION,
                vocabulary: vocabulary(),
                tick_dt: crate::events::tick::TICK_DT,
                max_frame_side: limits.map_or(0, |l| l.max_side),
                max_frame_bytes: limits.map_or(0, |l| l.max_bytes),
                guest_memory_max,
            })
        }
        ClientCall::ClientPacks => {
            let packs = petramond_world::assets::packs();
            HostRet::ClientPacks(
                crate::modding::modset::active(&BTreeSet::new())
                    .into_iter()
                    .map(|entry| ClientPackInfo {
                        client_wasm: packs.iter().any(|pack| {
                            pack.id.as_deref() == Some(entry.id.as_str())
                                && pack.client_wasm.is_some()
                        }),
                        id: entry.id,
                        version: entry.version,
                        affects_world: entry.affects_world,
                    })
                    .collect(),
            )
        }
        ClientCall::ClientWallClock => {
            let now = jiff::Zoned::now();
            HostRet::ClientWallClock(ClientWallTime {
                unix_ms: now.timestamp().as_millisecond(),
                utc_offset_min: now.offset().seconds().div_euclid(60) as i16,
            })
        }
        other => HostRet::invalid(format!(
            "non-facts call {other:?} mis-routed to the facts handler (host bug)"
        )),
    }
}
