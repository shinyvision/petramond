use mod_api::{ClientEntitiesNear, HostRet};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::modding::client::state::ClientStoreData;

pub(super) fn answer(
    client: &ClientStoreData,
    ids: Vec<mod_api::EntityRef>,
    near: Option<ClientEntitiesNear>,
) -> HostRet {
    if let Some(near) = &near {
        if !near.center.iter().all(|v| v.is_finite()) || near.radius.is_nan() || near.radius < 0.0 {
            return HostRet::invalid(
                "ClientEntities: near needs a finite center and a radius that is not negative"
                    .into(),
            );
        }
    }
    let presented = client.presented.lock();
    let by_id: FxHashMap<_, _> = presented.entities.iter().map(|row| (row.id, row)).collect();
    let mut rows: Vec<_> = ids
        .iter()
        .filter_map(|id| by_id.get(id).map(|row| (*row).clone()))
        .collect();
    if let Some(near) = near {
        let asked: FxHashSet<_> = ids.iter().copied().collect();
        let dist2 = |feet: &[f64; 3]| {
            (0..3)
                .map(|i| (feet[i] - near.center[i]).powi(2))
                .sum::<f64>()
        };
        let r2 = f64::from(near.radius).powi(2);
        let mut close: Vec<_> = presented
            .entities
            .iter()
            .map(|row| (dist2(&row.feet), row))
            .filter(|(d, row)| *d <= r2 && !asked.contains(&row.id))
            .collect();
        close.sort_by(|a, b| a.0.total_cmp(&b.0));
        let max = usize::try_from(near.max).unwrap_or(usize::MAX);
        rows.extend(close.into_iter().take(max).map(|(_, row)| row.clone()));
    }
    HostRet::ClientEntities(rows)
}
