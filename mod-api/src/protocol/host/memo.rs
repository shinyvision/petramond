use crate::legality::prelude::*;

host_domain! {
    MemoCall {
        MemoGet {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
        } => legal(BESIDE_WORLD, Any, Read),
        MemoGetMany {
            keys: Vec<Vec<u8>>,
        } => legal(BESIDE_WORLD, Any, Read),
        MemoPut {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
            #[serde(with = "serde_bytes")]
            value: Vec<u8>,
        } => legal(BESIDE_WORLD, Any, Write),
        /// Like [`Self::MemoGet`], but also settles who derives a missing entry.
        /// First caller to miss gets the lease, must [`Self::MemoPut`] the value.
        /// Others missing during the lease wait briefly, then hear derivation is
        /// pending, so they can defer instead of idling or redoing the work.
        /// → [`HostRet::MemoClaim`](crate::HostRet::MemoClaim).
        MemoClaim {
            #[serde(with = "serde_bytes")]
            key: Vec<u8>,
        } => legal(BESIDE_WORLD, Any, Write),
    }
}
