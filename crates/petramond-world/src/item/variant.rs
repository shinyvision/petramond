//! Per-stack item instance data, interned to a compact session id.
//!
//! A stack may carry a small namespaced key→bytes map (the item-side sibling
//! of per-cell block KV): `petramond:tint` on dyed wool, whatever a mod mints.
//! The map itself never rides in `ItemStack` — stacks stay `Copy` by holding a
//! [`VariantId`], an id interned process-wide from the map's CANONICAL BYTES.
//! Two stacks stack together iff item AND variant id match, which by
//! construction means byte-identical data.
//!
//! The id is a PROCESS-LOCAL compact: it never persists and never crosses the
//! save format, the wire, or the mod ABI — all three carry the canonical blob
//! (only for stacks that have data) and the receiving side re-interns. That
//! keeps saves registry-order-proof and spares the network layer a variant
//! remap table.
//!
//! Caps are deliberately tight (this multiplies across every inventory slot):
//! at most [`MAX_KEYS`] keys and [`MAX_VALUE_BYTES`]-byte values per map, and
//! a bounded intern table — an over-cap or table-full intern yields
//! `None`/`NONE` (the stack degrades to plain), never unbounded growth.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

/// Interned id for a stack's instance-data map. `NONE` (0) = no data.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VariantId(pub u16);

impl VariantId {
    pub const NONE: VariantId = VariantId(0);

    #[inline]
    pub fn is_none(self) -> bool {
        self == VariantId::NONE
    }
}

/// A stack's instance data: namespaced key → small opaque value.
pub type VariantMap = BTreeMap<String, Vec<u8>>;

/// Most keys one map may hold.
pub const MAX_KEYS: usize = 4;
/// Longest key, in bytes (namespaced `ns:name`).
pub const MAX_KEY_BYTES: usize = 64;
/// Longest value, in bytes.
pub const MAX_VALUE_BYTES: usize = 64;
/// Most distinct variants one process will intern (excluding `NONE`) — the
/// `u16` id ceiling. The table never evicts (outstanding ids must stay
/// valid), so this bounds a process-lifetime cache at ~10 MB worst case.
pub const MAX_VARIANTS: usize = u16::MAX as usize;

/// `true` if `data` fits every per-map cap and every key is namespaced.
pub fn valid(data: &VariantMap) -> bool {
    !data.is_empty()
        && data.len() <= MAX_KEYS
        && data.iter().all(|(k, v)| {
            k.len() <= MAX_KEY_BYTES
                && v.len() <= MAX_VALUE_BYTES
                && crate::registry::namespace(k).is_some()
        })
}

/// Canonical blob for `data` — the byte identity interning, saves, the wire,
/// and the ABI all share. BTreeMap iteration gives sorted keys, so equal maps
/// always canonicalize to equal bytes.
///
/// Layout: `u8` entry count, then per entry `u8` key len + key bytes +
/// `u8` value len + value bytes.
pub fn encode(data: &VariantMap) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(data.len() as u8);
    for (k, v) in data {
        out.push(k.len() as u8);
        out.extend_from_slice(k.as_bytes());
        out.push(v.len() as u8);
        out.extend_from_slice(v);
    }
    out
}

/// Parse a canonical blob back into a map. `None` on any structural fault,
/// over-cap content, or non-canonical key order — a foreign blob (save from a
/// modded build, wire noise) must never intern under a different byte identity
/// than [`encode`] would produce for the same map.
pub fn decode(bytes: &[u8]) -> Option<VariantMap> {
    let (&count, mut rest) = bytes.split_first()?;
    let mut map = VariantMap::new();
    for _ in 0..count {
        let (&klen, r) = rest.split_first()?;
        if r.len() < klen as usize {
            return None;
        }
        let (kb, r) = r.split_at(klen as usize);
        let key = std::str::from_utf8(kb).ok()?.to_owned();
        let (&vlen, r) = r.split_first()?;
        if r.len() < vlen as usize {
            return None;
        }
        let (vb, r) = r.split_at(vlen as usize);
        if let Some((last, _)) = map.last_key_value() {
            if *last >= key {
                return None;
            }
        }
        map.insert(key, vb.to_vec());
        rest = r;
    }
    if !rest.is_empty() || !valid(&map) {
        return None;
    }
    Some(map)
}

struct Interner {
    /// id-1 → (map, canonical blob). Index order is mint order.
    rows: Vec<(Arc<VariantMap>, Arc<Vec<u8>>)>,
    by_blob: HashMap<Vec<u8>, u16>,
}

static TABLE: OnceLock<RwLock<Interner>> = OnceLock::new();

fn table() -> &'static RwLock<Interner> {
    TABLE.get_or_init(|| {
        RwLock::new(Interner {
            rows: Vec::new(),
            by_blob: HashMap::new(),
        })
    })
}

/// Intern `data`, returning its process-wide id. `None` for an invalid map or
/// a full table — callers degrade the stack to plain (variant `NONE`) and the
/// mint site decides whether that is an error (ABI) or a logged warning
/// (save/wire ingest).
pub fn intern(data: &VariantMap) -> Option<VariantId> {
    if !valid(data) {
        return None;
    }
    let blob = encode(data);
    // Fast path under the read lock: on a running world almost every intern
    // is a repeat of an already-minted variant.
    if let Some(&id) = table()
        .read()
        .expect("variant table lock")
        .by_blob
        .get(&blob)
    {
        return Some(VariantId(id));
    }
    let mut t = table().write().expect("variant table lock");
    if let Some(&id) = t.by_blob.get(&blob) {
        return Some(VariantId(id));
    }
    if t.rows.len() >= MAX_VARIANTS {
        return None;
    }
    let id = (t.rows.len() + 1) as u16;
    t.rows
        .push((Arc::new(data.clone()), Arc::new(blob.clone())));
    t.by_blob.insert(blob, id);
    Some(VariantId(id))
}

/// Decode + intern a canonical blob (the save/wire/ABI ingest path).
pub fn intern_blob(bytes: &[u8]) -> Option<VariantId> {
    intern(&decode(bytes)?)
}

/// The interned map for `id` (`None` for `NONE` or an unknown id).
pub fn get(id: VariantId) -> Option<Arc<VariantMap>> {
    if id.is_none() {
        return None;
    }
    let t = table().read().expect("variant table lock");
    t.rows.get(id.0 as usize - 1).map(|(m, _)| m.clone())
}

/// The canonical blob for `id` (`None` for `NONE` or an unknown id) — what
/// saves, the wire, and the ABI ship instead of the id.
pub fn blob(id: VariantId) -> Option<Arc<Vec<u8>>> {
    if id.is_none() {
        return None;
    }
    let t = table().read().expect("variant table lock");
    t.rows.get(id.0 as usize - 1).map(|(_, b)| b.clone())
}

/// One value out of `id`'s map.
pub fn value(id: VariantId, key: &str) -> Option<Vec<u8>> {
    get(id).and_then(|m| m.get(key).cloned())
}

/// The `petramond:tint` multiply color `id` carries, if any (3 raw unorm8
/// bytes → `[0.0, 1.0]` floats; the multiply runs in the same gamma-encoded
/// domain the atlas texels and every other tint lane use). The
/// renderer-facing read behind every item-side tint draw.
pub fn tint(id: VariantId) -> Option<[f32; 3]> {
    let v = value(id, crate::block::TINT_KV_KEY)?;
    let [r, g, b]: [u8; 3] = v.as_slice().try_into().ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

/// The `petramond:overlay` instance-data key: the stack draws one or more
/// OTHER items' sprites composited over its own, at every site the base
/// sprite reaches (GUI icon, both hands, dropped entity). The value is a
/// comma-separated list of item registry NAMES — always by reference, never
/// pixels: pixels cannot fit the value cap, would mint a variant per pixmap,
/// and could not be rendered by a client that does not run the writing mod.
/// The overlay art is authored IN POSITION on its own transparent 16×16 (the
/// forge's diamond tip sits at the tip of its tool sprite), so compositing is
/// a plain stacked draw with no coordinates anywhere.
pub const OVERLAY_DATA_KEY: &str = "petramond:overlay";

/// The overlay items `id` names, resolved and in declared order. Lenient like
/// every instance-data read: non-UTF-8 bytes or an unknown item name simply
/// contribute nothing.
pub fn overlay_items(id: VariantId) -> Vec<super::ItemType> {
    let Some(v) = value(id, OVERLAY_DATA_KEY) else {
        return Vec::new();
    };
    let Ok(s) = std::str::from_utf8(&v) else {
        return Vec::new();
    };
    s.split(',')
        .filter_map(|name| super::ItemType::by_name(name.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &[u8])]) -> VariantMap {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_vec()))
            .collect()
    }

    /// The overlay key resolves BY REFERENCE and leniently: names resolve in
    /// declared order, unknown names contribute nothing (a pack uninstalled
    /// under a saved stack must degrade to the bare item, never error).
    #[test]
    fn overlay_items_resolve_names_in_order_and_skip_unknowns() {
        use super::super::ItemType;
        let id = |val: &[u8]| {
            let mut m = VariantMap::new();
            m.insert(OVERLAY_DATA_KEY.to_string(), val.to_vec());
            intern(&m).unwrap()
        };
        assert_eq!(
            overlay_items(id(b"petramond:stick, petramond:coal")),
            vec![ItemType::Stick, ItemType::Coal]
        );
        assert_eq!(
            overlay_items(id(b"gone:mod_item,petramond:stick")),
            vec![ItemType::Stick],
            "an unknown name is skipped, not an error"
        );
        assert_eq!(overlay_items(id(&[0xFF, 0xFE])), vec![], "non-UTF-8 = none");
        assert_eq!(overlay_items(VariantId::NONE), vec![]);
    }

    #[test]
    fn equal_maps_intern_to_the_same_id_and_unequal_ones_do_not() {
        let a = map(&[("m:tint", &[10, 20, 30])]);
        let b = map(&[("m:tint", &[10, 20, 30])]);
        let c = map(&[("m:tint", &[10, 20, 31])]);
        let ia = intern(&a).unwrap();
        assert_eq!(intern(&b).unwrap(), ia, "byte-equal maps share an id");
        assert_ne!(intern(&c).unwrap(), ia, "one byte off = distinct variant");
        assert_eq!(*get(ia).unwrap(), a);
    }

    #[test]
    fn blobs_round_trip_and_reintern_to_the_same_id() {
        let m = map(&[("m:a", &[1]), ("m:b", &[2, 3])]);
        let id = intern(&m).unwrap();
        let blob = blob(id).unwrap();
        assert_eq!(decode(&blob).unwrap(), m);
        assert_eq!(intern_blob(&blob).unwrap(), id);
    }

    #[test]
    fn invalid_maps_and_malformed_blobs_are_refused() {
        assert_eq!(intern(&VariantMap::new()), None, "empty = no variant");
        assert_eq!(intern(&map(&[("bare_key", &[1])])), None, "bare key");
        assert_eq!(intern(&map(&[("m:big", &[0u8; 65])])), None, "value cap");
        let over: VariantMap = (0..5).map(|i| (format!("m:k{i}"), vec![0u8])).collect();
        assert_eq!(intern(&over), None, "key-count cap");
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[1, 3]), None, "truncated key");
        // Non-canonical order must not decode (byte identity would fork):
        // hand-build reversed order — count 2, "m:b" then "m:a".
        let mut swapped = vec![2, 3];
        swapped.extend(b"m:b");
        swapped.extend([1, 2, 3]);
        swapped.extend(b"m:a");
        swapped.extend([1, 1]);
        assert_eq!(decode(&swapped), None, "unsorted keys refused");
    }
}
