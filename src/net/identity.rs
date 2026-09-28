use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::secure::Binding;

pub const MAX_NAME_CHARS: usize = 24;

const JOIN_PROOF_DOMAIN: &[u8] = b"petramond/join-proof/v2\0";

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerKey(pub [u8; 32]);

impl fmt::Display for PlayerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BadPlayerKey;

impl fmt::Display for BadPlayerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a player key is 64 hex digits")
    }
}

impl std::error::Error for BadPlayerKey {}

impl FromStr for PlayerKey {
    type Err = BadPlayerKey;

    fn from_str(s: &str) -> Result<PlayerKey, BadPlayerKey> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return Err(BadPlayerKey);
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| BadPlayerKey)?;
        }
        Ok(PlayerKey(out))
    }
}

pub type JoinChallenge = [u8; 32];

pub fn new_challenge() -> Option<JoinChallenge> {
    let mut challenge = [0u8; 32];
    getrandom::getrandom(&mut challenge).ok()?;
    Some(challenge)
}

/// The proof signs the connection's key-exchange binding (`net::secure`), which covers the
/// server's challenge, so it is good for this one connection to this one server only.
pub fn join_proof_message(binding: &Binding, key: &PlayerKey) -> Vec<u8> {
    let mut msg = Vec::with_capacity(JOIN_PROOF_DOMAIN.len() + 64);
    msg.extend_from_slice(JOIN_PROOF_DOMAIN);
    msg.extend_from_slice(binding);
    msg.extend_from_slice(&key.0);
    msg
}

pub fn verify_join(binding: &Binding, key: &PlayerKey, signature: &[u8]) -> bool {
    let Ok(verifying) = VerifyingKey::from_bytes(&key.0) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    verifying
        .verify_strict(&join_proof_message(binding, key), &signature)
        .is_ok()
}

pub fn offline_key(name: &str) -> PlayerKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"petramond/offline-local-identity/v1\0");
    hasher.update(canonical_name(name).as_bytes());
    PlayerKey(*hasher.finalize().as_bytes())
}

pub struct PlayerIdentity {
    signing: SigningKey,
}

impl fmt::Debug for PlayerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlayerIdentity")
            .field("key", &self.key())
            .finish()
    }
}

impl PlayerIdentity {
    pub fn generate() -> io::Result<PlayerIdentity> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(io::Error::other)?;
        Ok(PlayerIdentity {
            signing: SigningKey::from_bytes(&seed),
        })
    }

    pub fn key(&self) -> PlayerKey {
        PlayerKey(self.signing.verifying_key().to_bytes())
    }

    pub fn sign_join(&self, binding: &Binding) -> Vec<u8> {
        self.signing
            .sign(&join_proof_message(binding, &self.key()))
            .to_bytes()
            .to_vec()
    }

    pub fn load_or_create(path: &Path) -> io::Result<PlayerIdentity> {
        match std::fs::read_to_string(path) {
            Ok(text) => return Self::parse(&text),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            Err(_) => {}
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let fresh = Self::generate()?;
        let text = format!("{}\n", hex(&fresh.signing.to_bytes()));
        match petramond_persist::atomic_file::publish_new(path, |file| {
            restrict_to_owner(file)?;
            io::Write::write_all(file, text.as_bytes())
        }) {
            Ok(()) => Ok(fresh),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                Self::parse(&std::fs::read_to_string(path)?)
            }
            Err(e) => Err(e),
        }
    }

    pub fn load_or_create_default() -> io::Result<PlayerIdentity> {
        Self::load_or_create(&default_path())
    }

    fn parse(text: &str) -> io::Result<PlayerIdentity> {
        let PlayerKey(seed) = text.trim().parse().map_err(|e: BadPlayerKey| {
            io::Error::new(io::ErrorKind::InvalidData, format!("identity file: {e}"))
        })?;
        Ok(PlayerIdentity {
            signing: SigningKey::from_bytes(&seed),
        })
    }
}

fn default_path() -> PathBuf {
    std::env::var_os("PETRAMOND_IDENTITY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| petramond_util::paths::base_data_dir().join("identity.key"))
}

fn hex(bytes: &[u8; 32]) -> String {
    PlayerKey(*bytes).to_string()
}

fn restrict_to_owner(file: &std::fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    BadChar(char),
    Reserved,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Empty => write!(f, "Player name cannot be empty"),
            NameError::TooLong => {
                write!(f, "Player names are at most {MAX_NAME_CHARS} characters")
            }
            NameError::BadChar(c) => write!(
                f,
                "Player names may only use letters, digits, spaces, '_', '-' and '.' (not {c:?})"
            ),
            NameError::Reserved => write!(f, "That player name is reserved"),
        }
    }
}

pub const RESERVED_NAMES: &[&str] = &["server", "console", "system", "admin", "petramond"];

fn name_char_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.')
}

pub fn validate_player_name(raw: &str) -> Result<String, NameError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if let Some(c) = name.chars().find(|&c| !name_char_allowed(c)) {
        return Err(NameError::BadChar(c));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(NameError::TooLong);
    }
    if RESERVED_NAMES.contains(&canonical_name(name).as_str()) {
        return Err(NameError::Reserved);
    }
    Ok(name.to_string())
}

pub fn coerce_player_name(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|&c| name_char_allowed(c))
        .take(MAX_NAME_CHARS)
        .collect();
    validate_player_name(&kept).unwrap_or_else(|_| "Player".to_string())
}

pub fn canonical_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_challenge_verifies_only_for_its_challenge_and_key() {
        let id = PlayerIdentity::generate().expect("os randomness");
        let challenge = [7u8; 32];
        let proof = id.sign_join(&challenge);
        assert!(verify_join(&challenge, &id.key(), &proof));

        assert!(
            !verify_join(&[8u8; 32], &id.key(), &proof),
            "a proof never replays against another connection's challenge"
        );
        let other = PlayerIdentity::generate().expect("os randomness");
        assert!(
            !verify_join(&challenge, &other.key(), &proof),
            "a proof cannot be claimed for somebody else's key"
        );
        assert!(!verify_join(&challenge, &id.key(), &proof[..63]));
        assert!(!verify_join(&challenge, &id.key(), &[]));
        let mut forged = proof.clone();
        forged[0] ^= 1;
        assert!(!verify_join(&challenge, &id.key(), &forged));
    }

    #[test]
    fn offline_keys_are_stable_per_name() {
        assert_eq!(offline_key("Rachel"), offline_key(" rachel "));
        assert_ne!(offline_key("Rachel"), offline_key("Bob"));
    }

    #[test]
    fn challenges_are_fresh() {
        let a = new_challenge().expect("os randomness");
        let b = new_challenge().expect("os randomness");
        assert_ne!(a, b);
    }

    #[test]
    fn player_keys_roundtrip_through_hex() {
        let key = PlayerIdentity::generate().expect("os randomness").key();
        let text = key.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<PlayerKey>(), Ok(key));
        assert_eq!("zz".parse::<PlayerKey>(), Err(BadPlayerKey));
        assert_eq!("g".repeat(64).parse::<PlayerKey>(), Err(BadPlayerKey));
        assert_eq!("é".repeat(32).parse::<PlayerKey>(), Err(BadPlayerKey));
    }

    #[test]
    fn the_identity_file_is_created_once_and_then_reused() {
        let dir = std::env::temp_dir().join(format!(
            "petramond-identity-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("identity.key");
        let first = PlayerIdentity::load_or_create(&path).expect("created");
        let again = PlayerIdentity::load_or_create(&path).expect("reloaded");
        assert_eq!(first.key(), again.key(), "the identity is stable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the secret is owner-only");
        }

        std::fs::write(&path, "not a key").expect("corrupt");
        let err = PlayerIdentity::load_or_create(&path).expect_err("never silently replaced");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_validate_at_the_edge() {
        assert_eq!(validate_player_name("  Rachel S. "), Ok("Rachel S.".into()));
        assert_eq!(validate_player_name("   "), Err(NameError::Empty));
        assert_eq!(validate_player_name("Ann!"), Err(NameError::BadChar('!')));
        assert_eq!(
            validate_player_name(&"a".repeat(MAX_NAME_CHARS + 1)),
            Err(NameError::TooLong)
        );
        assert_eq!(coerce_player_name("rä©hel!"), "rhel");
        assert_eq!(coerce_player_name("!!!"), "Player");
        assert_eq!(
            coerce_player_name(&"b".repeat(40)).len(),
            MAX_NAME_CHARS,
            "coercion truncates"
        );
        assert_eq!(canonical_name(" RaChel "), "rachel");
    }

    #[test]
    fn reserved_names_are_refused() {
        assert_eq!(validate_player_name(" SERVER "), Err(NameError::Reserved));
        assert_eq!(validate_player_name("Console"), Err(NameError::Reserved));
        assert_eq!(validate_player_name("Servers"), Ok("Servers".into()));
        assert_eq!(coerce_player_name("server"), "Player");
        assert_eq!(
            validate_player_name("$[fg=red]x"),
            Err(NameError::BadChar('$')),
            "markup never survives validation"
        );
    }
}
