use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(test)]
mod tests;

pub type Digest = [u8; 32];

pub fn digest(bytes: &[u8]) -> Digest {
    *blake3::hash(bytes).as_bytes()
}

const PACKET_BYTES: usize = 64 * 1024;
const WINDOW: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BlobPacket {
    Begin { digest: Digest, len: u64 },
    Data { digest: Digest, bytes: Vec<u8> },
    Credit { digest: Digest, count: usize },
    Cancel { digest: Digest },
}

impl BlobPacket {
    pub fn digest(&self) -> Digest {
        match self {
            Self::Begin { digest, .. }
            | Self::Data { digest, .. }
            | Self::Credit { digest, .. }
            | Self::Cancel { digest } => *digest,
        }
    }
}

pub struct BlobSender {
    digest: Digest,
    bytes: Arc<[u8]>,
    offset: usize,
    credit: usize,
    begun: bool,
}

impl BlobSender {
    pub fn new(digest: Digest, bytes: Arc<[u8]>) -> Self {
        Self {
            digest,
            bytes,
            offset: 0,
            credit: WINDOW,
            begun: false,
        }
    }

    pub fn digest(&self) -> Digest {
        self.digest
    }

    pub fn finished(&self) -> bool {
        self.begun && self.offset >= self.bytes.len()
    }

    pub fn credit(&mut self, count: usize) -> bool {
        if count > WINDOW || self.credit + count > WINDOW {
            return false;
        }
        self.credit += count;
        true
    }

    pub fn packets(&mut self) -> Vec<BlobPacket> {
        let mut out = Vec::new();
        if !self.begun {
            self.begun = true;
            out.push(BlobPacket::Begin {
                digest: self.digest,
                len: self.bytes.len() as u64,
            });
        }
        while self.credit > 0 && self.offset < self.bytes.len() {
            let end = (self.offset + PACKET_BYTES).min(self.bytes.len());
            out.push(BlobPacket::Data {
                digest: self.digest,
                bytes: self.bytes[self.offset..end].to_vec(),
            });
            self.offset = end;
            self.credit -= 1;
        }
        out
    }
}

pub struct BlobReceiver {
    digest: Digest,
    len: usize,
    bytes: Vec<u8>,
    owed_credit: usize,
}

impl BlobReceiver {
    pub fn begin(packet: &BlobPacket, expected: Digest, max_len: u64) -> Result<Self, String> {
        match *packet {
            BlobPacket::Begin { digest, len } if digest == expected && len > 0 => {
                if len > max_len {
                    return Err("The stream is too large".into());
                }
                Ok(Self {
                    digest,
                    len: usize::try_from(len).map_err(|_| "The stream is too large")?,
                    bytes: Vec::new(),
                    owed_credit: 0,
                })
            }
            _ => Err("Unexpected stream".into()),
        }
    }

    pub fn digest(&self) -> Digest {
        self.digest
    }

    pub fn receive(&mut self, packet: BlobPacket) -> Result<(), String> {
        let BlobPacket::Data { digest, bytes } = packet else {
            return Err("Unexpected packet".into());
        };
        if digest != self.digest
            || bytes.is_empty()
            || bytes.len() > PACKET_BYTES
            || self.bytes.len() + bytes.len() > self.len
            || self.owed_credit >= WINDOW
        {
            return Err("Invalid packet".into());
        }
        self.bytes.extend_from_slice(&bytes);
        self.owed_credit += 1;
        Ok(())
    }

    pub fn take_credit(&mut self) -> Option<BlobPacket> {
        (self.owed_credit > 0).then(|| BlobPacket::Credit {
            digest: self.digest,
            count: std::mem::take(&mut self.owed_credit),
        })
    }

    pub fn finish(&mut self) -> Option<Result<Vec<u8>, String>> {
        if self.bytes.len() < self.len {
            return None;
        }
        let bytes = std::mem::take(&mut self.bytes);
        Some(if digest(&bytes) == self.digest {
            Ok(bytes)
        } else {
            Err("The stream arrived damaged".into())
        })
    }
}
