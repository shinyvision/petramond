use std::sync::OnceLock;

use mod_api::capture::{ClientPieceInfo, ClientPieceKind, ClientStateKey};
use serde::Serialize;

use super::body::{self, BatchRows};
use crate::net::protocol::{EntityLane, NameTables, RowSet, TickUpdate};

#[derive(Clone, Debug)]
pub struct Piece {
    pub bytes: Vec<u8>,
    pub info: ClientPieceInfo,
}

impl Piece {
    pub fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

pub fn vocabulary() -> &'static (NameTables, u64) {
    static TABLES: OnceLock<(NameTables, u64)> = OnceLock::new();
    TABLES.get_or_init(|| {
        let tables = crate::net::remap::local_name_tables();
        let hash = body::vocabulary_of(&tables);
        (tables, hash)
    })
}

pub type EncodeError = String;

pub fn piece<T: Serialize>(
    kind: ClientPieceKind,
    key: [u8; 12],
    provisional: bool,
    value: &T,
) -> Result<Piece, EncodeError> {
    let frame =
        body::encode_body(value, !kind.documented()).map_err(|e| format!("{kind:?}: {e}"))?;
    let bytes = body::piece(kind, key, provisional, vocabulary().1, &frame);
    let batch = kind
        .is_batch()
        .then(|| u64::from_le_bytes(key[0..8].try_into().expect("8 bytes")));
    Ok(Piece {
        info: ClientPieceInfo {
            kind,
            key: ClientStateKey::from_head(kind, key),
            batch,
            range: [0, bytes.len() as u64],
            crc: mod_api::capture::crc32(&frame),
            provisional,
        },
        bytes,
    })
}

pub fn state<T: Serialize>(
    key: ClientStateKey,
    provisional: bool,
    value: &T,
) -> Result<Piece, EncodeError> {
    piece(key.kind(), key.key_bytes(), provisional, value)
}

pub fn batch_key(tick: u64) -> [u8; 12] {
    let mut key = [0u8; 12];
    key[0..8].copy_from_slice(&tick.to_le_bytes());
    key
}

pub fn split_batch(t: &TickUpdate) -> Result<Vec<Piece>, EncodeError> {
    let key = batch_key(t.tick);
    let (world, rows, rest) = body::split_batch(t);
    let mut out = vec![piece(ClientPieceKind::BatchWorld, key, false, &world)?];
    split_rows(key, rows, &mut out)?;
    out.push(piece(ClientPieceKind::BatchRest, key, false, &rest)?);
    Ok(out)
}

fn split_rows(key: [u8; 12], rows: BatchRows, out: &mut Vec<Piece>) -> Result<(), EncodeError> {
    match piece(ClientPieceKind::BatchRows, key, false, &rows) {
        Ok(piece) => {
            out.push(piece);
            Ok(())
        }
        Err(why) if rows.entries() <= 1 => Err(why),
        Err(_) => {
            let (a, b) = halve(rows);
            split_rows(key, a, out)?;
            split_rows(key, b, out)
        }
    }
}

fn halve(rows: BatchRows) -> (BatchRows, BatchRows) {
    let mut take = rows.entries() / 2;
    let (mobs, mobs_rest) = split_lane(rows.mobs, &mut take);
    let (items, items_rest) = split_lane(rows.items, &mut take);
    let (players, players_rest) = split_lane(rows.players, &mut take);
    (
        BatchRows {
            mobs,
            items,
            players,
        },
        BatchRows {
            mobs: mobs_rest,
            items: items_rest,
            players: players_rest,
        },
    )
}

fn split_lane<R: Clone, K>(
    lane: EntityLane<R, K>,
    take: &mut usize,
) -> (EntityLane<R, K>, EntityLane<R, K>) {
    let EntityLane {
        mut despawned,
        spawned,
        updated,
    } = lane;
    let n = (*take).min(despawned.len());
    let despawned_rest = despawned.split_off(n);
    *take -= n;
    let (spawned, spawned_rest) = split_set(spawned, take);
    let (updated, updated_rest) = split_set(updated, take);
    (
        EntityLane {
            despawned,
            spawned,
            updated,
        },
        EntityLane {
            despawned: despawned_rest,
            spawned: spawned_rest,
            updated: updated_rest,
        },
    )
}

fn split_set<R: Clone>(set: RowSet<R>, take: &mut usize) -> (RowSet<R>, RowSet<R>) {
    let mut rows: Vec<R> = set.iter().cloned().collect();
    let n = (*take).min(rows.len());
    let rest = rows.split_off(n);
    *take -= n;
    (rows.into(), rest.into())
}
