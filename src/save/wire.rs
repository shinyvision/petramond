//! Codecs defined once.
//!
//! A persisted structure states its layout ONE time and both directions are
//! derived from that statement, so an encoder and its decoder cannot drift
//! apart:
//! - [`Wire`] is the codec of one value. Primitives, strings, lists, maps,
//!   options, tuples and positions implement it, and so does every record
//!   built with the macros below.
//! - `wire_struct!` derives [`Wire`] for a fixed positional struct from its
//!   field list: for small, stable, high-volume shapes (an item slot, a
//!   furnace's counters) where a tag per field would cost more than the
//!   data.
//! - `tagged_record!` derives [`Wire`] for an EVOLVING record, stored as a
//!   list of `[tag: u16][len: u32][value]` fields. A field the record does
//!   not carry reads as its default, so a record GAINS a field by adding a
//!   tag — no version bump, no upgrade step. A field this build does not
//!   know is kept in the record's `unknown` set and written back unchanged
//!   by [`Wire::put`]; whoever turns the record into a live object decides
//!   whether it can carry those fields or must refuse the record (see
//!   `save::format` for the policy).
//!
//! The bulk section arrays (blocks, light, fluid) stay hand-tuned raw
//! payloads in `save::codec`; everything around them is built from these.

use std::collections::BTreeMap;

use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_persist::bytecodec::{
    put_f32, put_f64, put_i64, put_u16, put_u32, put_u64, put_u8, Reader,
};

/// The codec of one persisted value.
pub trait Wire: Sized {
    fn put(&self, buf: &mut Vec<u8>);
    /// `None` on truncated or malformed input.
    fn get(r: &mut Reader) -> Option<Self>;
}

/// Encode `value` into a fresh buffer.
pub fn to_bytes<T: Wire>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    value.put(&mut buf);
    buf
}

/// Decode a whole buffer as one `T`: every byte must belong to it.
pub fn from_bytes<T: Wire>(bytes: &[u8]) -> Option<T> {
    let mut r = Reader::new(bytes);
    let value = T::get(&mut r)?;
    r.is_at_end().then_some(value)
}

macro_rules! wire_primitive {
    ($($t:ty => $put:ident, $get:ident;)*) => {$(
        impl Wire for $t {
            fn put(&self, buf: &mut Vec<u8>) {
                $put(buf, *self);
            }

            fn get(r: &mut Reader) -> Option<Self> {
                r.$get()
            }
        }
    )*};
}

wire_primitive! {
    u8 => put_u8, u8;
    u16 => put_u16, u16;
    u32 => put_u32, u32;
    u64 => put_u64, u64;
    i64 => put_i64, i64;
    f32 => put_f32, f32;
    f64 => put_f64, f64;
}

impl Wire for i32 {
    fn put(&self, buf: &mut Vec<u8>) {
        put_u32(buf, *self as u32);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        Some(r.u32()? as i32)
    }
}

impl Wire for bool {
    fn put(&self, buf: &mut Vec<u8>) {
        put_u8(buf, u8::from(*self));
    }

    fn get(r: &mut Reader) -> Option<Self> {
        match r.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

/// `[len: u32][utf-8 bytes]`.
impl Wire for String {
    fn put(&self, buf: &mut Vec<u8>) {
        put_u32(buf, self.len() as u32);
        buf.extend_from_slice(self.as_bytes());
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let len = r.u32()? as usize;
        Some(std::str::from_utf8(r.bytes(len)?).ok()?.to_owned())
    }
}

/// `[count: u32]` then each item.
impl<T: Wire> Wire for Vec<T> {
    fn put(&self, buf: &mut Vec<u8>) {
        put_u32(buf, self.len() as u32);
        for item in self {
            item.put(buf);
        }
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let n = r.u32()? as usize;
        // The count is untrusted: reserve for a sane list only.
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(T::get(r)?);
        }
        Some(out)
    }
}

/// `0` for none, `1` then the value for some.
impl<T: Wire> Wire for Option<T> {
    fn put(&self, buf: &mut Vec<u8>) {
        match self {
            None => put_u8(buf, 0),
            Some(value) => {
                put_u8(buf, 1);
                value.put(buf);
            }
        }
    }

    fn get(r: &mut Reader) -> Option<Self> {
        match r.u8()? {
            0 => Some(None),
            1 => Some(Some(T::get(r)?)),
            _ => None,
        }
    }
}

/// `[count: u32]` then each `(key, value)` in key order; a repeated key is
/// malformed.
impl<V: Wire> Wire for BTreeMap<String, V> {
    fn put(&self, buf: &mut Vec<u8>) {
        put_u32(buf, self.len() as u32);
        for (key, value) in self {
            key.put(buf);
            value.put(buf);
        }
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let n = r.u32()?;
        let mut out = BTreeMap::new();
        for _ in 0..n {
            let key = String::get(r)?;
            let value = V::get(r)?;
            if out.insert(key, value).is_some() {
                return None;
            }
        }
        Some(out)
    }
}

impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, buf: &mut Vec<u8>) {
        self.0.put(buf);
        self.1.put(buf);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let a = A::get(r)?;
        Some((a, B::get(r)?))
    }
}

impl Wire for WorldPos {
    fn put(&self, buf: &mut Vec<u8>) {
        put_f64(buf, self.x);
        put_f64(buf, self.y);
        put_f64(buf, self.z);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let (x, y) = (r.f64()?, r.f64()?);
        Some(WorldPos::new(x, y, r.f64()?))
    }
}

impl Wire for Vec3 {
    fn put(&self, buf: &mut Vec<u8>) {
        put_f32(buf, self.x);
        put_f32(buf, self.y);
        put_f32(buf, self.z);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let (x, y) = (r.f32()?, r.f32()?);
        Some(Vec3::new(x, y, r.f32()?))
    }
}

impl Wire for IVec3 {
    fn put(&self, buf: &mut Vec<u8>) {
        self.x.put(buf);
        self.y.put(buf);
        self.z.put(buf);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let (x, y) = (i32::get(r)?, i32::get(r)?);
        Some(IVec3::new(x, y, i32::get(r)?))
    }
}

/// Opaque bytes behind a `u16` length — the item-slot instance-data blob.
/// A longer blob is cut at `u16::MAX` bytes rather than desyncing the
/// record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blob16(pub Vec<u8>);

impl Wire for Blob16 {
    fn put(&self, buf: &mut Vec<u8>) {
        let bytes = &self.0[..self.0.len().min(u16::MAX as usize)];
        put_u16(buf, bytes.len() as u16);
        buf.extend_from_slice(bytes);
    }

    fn get(r: &mut Reader) -> Option<Self> {
        let len = r.u16()? as usize;
        Some(Blob16(r.bytes(len)?.to_vec()))
    }
}

/// Derive [`Wire`] for a positional struct: its fields, in the listed
/// order, each through its own [`Wire`].
macro_rules! wire_struct {
    ($name:ty { $($field:ident),+ $(,)? }) => {
        impl $crate::save::wire::Wire for $name {
            fn put(&self, buf: &mut Vec<u8>) {
                $( $crate::save::wire::Wire::put(&self.$field, buf); )+
            }

            fn get(r: &mut petramond_persist::bytecodec::Reader) -> Option<Self> {
                Some(Self {
                    $( $field: $crate::save::wire::Wire::get(r)?, )+
                })
            }
        }
    };
}
pub(super) use wire_struct;

/// Fields of a tagged record this build does not know, by tag, kept to be
/// written back unchanged.
pub type UnknownFields = BTreeMap<u16, Vec<u8>>;

/// Derive [`Wire`] for a tagged record: each listed field under its tag,
/// then the record's `unknown` fields. The struct must have an
/// `unknown: UnknownFields` field and every listed field type must be
/// `Default` (what an absent field reads as).
macro_rules! tagged_record {
    ($name:ty { $($tag:literal => $field:ident),+ $(,)? }) => {
        impl $crate::save::wire::Wire for $name {
            fn put(&self, buf: &mut Vec<u8>) {
                let mut w = $crate::save::wire::TaggedWriter::new(buf);
                $( w.field($tag, &self.$field); )+
                w.unknown(&self.unknown);
                w.finish();
            }

            fn get(r: &mut petramond_persist::bytecodec::Reader) -> Option<Self> {
                let mut fields = $crate::save::wire::TaggedFields::read(r)?;
                Some(Self {
                    $( $field: fields.take($tag)?, )+
                    unknown: fields.into_unknown(),
                })
            }
        }
    };
}
pub(super) use tagged_record;

/// Writes one tagged record: `[count: u16]` then each field as
/// `[tag: u16][len: u32][value]`.
pub struct TaggedWriter<'a> {
    buf: &'a mut Vec<u8>,
    count_at: usize,
    count: u16,
}

impl<'a> TaggedWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>) -> Self {
        let count_at = buf.len();
        put_u16(buf, 0);
        Self {
            buf,
            count_at,
            count: 0,
        }
    }

    pub fn field<T: Wire>(&mut self, tag: u16, value: &T) {
        put_u16(self.buf, tag);
        let len_at = self.buf.len();
        put_u32(self.buf, 0);
        value.put(self.buf);
        let len = (self.buf.len() - len_at - 4) as u32;
        self.buf[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
        self.count += 1;
    }

    /// A field whose value is already encoded.
    pub fn raw(&mut self, tag: u16, bytes: &[u8]) {
        put_u16(self.buf, tag);
        put_u32(self.buf, bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
        self.count += 1;
    }

    /// Fields kept from a record this build could not fully read.
    pub fn unknown(&mut self, fields: &UnknownFields) {
        for (&tag, bytes) in fields {
            self.raw(tag, bytes);
        }
    }

    pub fn finish(self) {
        self.buf[self.count_at..self.count_at + 2].copy_from_slice(&self.count.to_le_bytes());
    }
}

/// A tagged record's fields as read, each still undecoded.
pub struct TaggedFields<'a> {
    fields: BTreeMap<u16, &'a [u8]>,
}

impl<'a> TaggedFields<'a> {
    /// `None` when truncated or when a tag repeats.
    pub fn read(r: &mut Reader<'a>) -> Option<Self> {
        let count = r.u16()?;
        let mut fields = BTreeMap::new();
        for _ in 0..count {
            let tag = r.u16()?;
            let len = r.u32()? as usize;
            if fields.insert(tag, r.bytes(len)?).is_some() {
                return None;
            }
        }
        Some(Self { fields })
    }

    /// Field `tag` decoded — its default when the record does not carry it,
    /// `None` when it does but the value is malformed or does not fill its
    /// bytes exactly.
    pub fn take<T: Wire + Default>(&mut self, tag: u16) -> Option<T> {
        match self.fields.remove(&tag) {
            None => Some(T::default()),
            Some(bytes) => from_bytes(bytes),
        }
    }

    /// Every field not taken.
    pub fn into_unknown(self) -> UnknownFields {
        self.fields
            .into_iter()
            .map(|(tag, bytes)| (tag, bytes.to_vec()))
            .collect()
    }
}

#[cfg(test)]
mod tests;
