use std::fmt;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{CasError, Result};

/// Content identifier: a Blake3 hash of stored data.
///
/// Stored as 32 bytes (256 bits). Represented as lowercase hex in string form.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cid([u8; 32]);

impl Cid {
    /// Compute a CID by hashing the given bytes with Blake3.
    #[must_use] pub fn from_bytes(data: &[u8]) -> Self {
        let hash = blake3::hash(data);
        Self(*hash.as_bytes())
    }

    /// Compute a CID by streaming the contents of a file.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let mut hasher = blake3::Hasher::new();
        let reader = std::io::BufReader::new(std::fs::File::open(path)?);
        hasher.update_reader(reader)?;
        Ok(Self(*hasher.finalize().as_bytes()))
    }

    /// Return the raw 32-byte hash.
    #[must_use] pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return the lowercase hex representation (64 characters).
    #[must_use] pub fn as_hex(&self) -> String {
        hex_encode(&self.0)
    }

    /// Parse a CID from its hex representation.
    pub fn from_hex(hex: &str) -> Result<Self> {
        if hex.len() != 64 {
            return Err(CasError::InvalidCid(format!(
                "expected 64 hex characters, got {}",
                hex.len()
            )));
        }
        let bytes = hex_decode(hex)?;
        Ok(Self(bytes))
    }

    /// Relative path within the store: `ab/abcdef...`.
    pub(crate) fn relative_path(&self) -> std::path::PathBuf {
        let hex = self.as_hex();
        std::path::PathBuf::from(&hex[..2]).join(&hex)
    }
}

fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for &b in bytes {
        let _ = std::fmt::write(&mut s, format_args!("{b:02x}"));
    }
    s
}

fn hex_decode(hex: &str) -> std::result::Result<[u8; 32], CasError> {
    if hex.len() != 64 {
        return Err(CasError::InvalidCid(format!(
            "expected 64 hex chars, got {}",
            hex.len()
        )));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| CasError::InvalidCid(format!("hex decode at position {i}: {e}")))?;
        out[i] = byte;
    }
    Ok(out)
}

impl fmt::Display for Cid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_hex())
    }
}

impl std::str::FromStr for Cid {
    type Err = CasError;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_hex(s)
    }
}

impl Serialize for Cid {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for Cid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let hex = String::deserialize(deserializer)?;
        Self::from_hex(&hex).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_deterministic() {
        let a = Cid::from_bytes(b"hello world");
        let b = Cid::from_bytes(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn from_bytes_different_inputs() {
        let a = Cid::from_bytes(b"hello");
        let b = Cid::from_bytes(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn as_hex_length() {
        let cid = Cid::from_bytes(b"test");
        assert_eq!(cid.as_hex().len(), 64);
    }

    #[test]
    fn roundtrip_hex() {
        let cid = Cid::from_bytes(b"roundtrip test");
        let hex = cid.as_hex();
        let parsed = Cid::from_hex(&hex).unwrap();
        assert_eq!(cid, parsed);
    }

    #[test]
    fn from_hex_invalid_length() {
        assert!(Cid::from_hex("abc").is_err());
    }

    #[test]
    fn from_hex_invalid_chars() {
        assert!(Cid::from_hex(&"g".repeat(64)).is_err());
    }

    #[test]
    fn display_impl() {
        let cid = Cid::from_bytes(b"display");
        assert_eq!(format!("{cid}"), cid.as_hex());
    }

    #[test]
    fn from_str_impl() {
        let cid = Cid::from_bytes(b"from_str");
        let parsed: Cid = cid.as_hex().parse().unwrap();
        assert_eq!(cid, parsed);
    }

    #[test]
    fn serde_roundtrip() {
        let cid = Cid::from_bytes(b"serde");
        let json = serde_json::to_string(&cid).unwrap();
        let back: Cid = serde_json::from_str(&json).unwrap();
        assert_eq!(cid, back);
    }

    #[test]
    fn serde_in_struct() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            cid: Cid,
        }
        let w = Wrapper {
            cid: Cid::from_bytes(b"struct"),
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: Wrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn relative_path_format() {
        let cid = Cid::from_bytes(b"path");
        let hex = cid.as_hex();
        let expected = std::path::PathBuf::from(&hex[..2]).join(&hex);
        assert_eq!(cid.relative_path(), expected);
    }
}
