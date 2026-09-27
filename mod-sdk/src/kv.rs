use crate::__rt::host_fn;
use crate::__rt::try_host_fn;

try_host_fn! {
    pub fn try_world_kv_set(key: &str, value: Vec<u8>)
        => WorldKvSet { key: key.into(), value }
}

try_host_fn! {
    pub fn try_section_kv_set(pos: [i32; 3], key: &str, value: Vec<u8>) -> bool
        => SectionKvSet { pos, key: key.into(), value } => Bool
}

try_host_fn! {
    pub fn try_section_kv_set_many(key: &str, writes: Vec<([i32; 3], Option<Vec<u8>>)>) -> Vec<bool>
        => SectionKvSetMany { key: key.into(), writes } => Bools
}

host_fn! {
    pub fn section_kv_find(section: [i32; 3], key: &str) -> Option<Vec<[i32; 3]>>
        => SectionKvFind { section, key: key.into() } => FoundBlocks
}

host_fn! {
    pub fn world_kv_get(key: &str) -> Option<Vec<u8>> => WorldKvGet { key: key.into() } => Bytes
}

host_fn! {
    pub fn world_kv_set(key: &str, value: Vec<u8>) => WorldKvSet { key: key.into(), value }
}

host_fn! {
    pub fn world_kv_delete(key: &str) -> bool => WorldKvDelete { key: key.into() } => Bool
}

host_fn! {
    pub fn section_kv_get(pos: [i32; 3], key: &str) -> Option<Vec<u8>>
        => SectionKvGet { pos, key: key.into() } => Bytes
}

host_fn! {
    pub fn section_kv_set(pos: [i32; 3], key: &str, value: Vec<u8>) -> bool
        => SectionKvSet { pos, key: key.into(), value } => Bool
}

host_fn! {
    pub fn section_kv_delete(pos: [i32; 3], key: &str) -> bool
        => SectionKvDelete { pos, key: key.into() } => Bool
}

host_fn! {
    pub fn section_kv_get_many(key: &str, positions: Vec<[i32; 3]>) -> Vec<Option<Vec<u8>>>
        => SectionKvGetMany { key: key.into(), positions } => BytesMany
}

host_fn! {
    pub fn section_kv_set_many(key: &str, writes: Vec<([i32; 3], Option<Vec<u8>>)>) -> Vec<bool>
        => SectionKvSetMany { key: key.into(), writes } => Bools
}
