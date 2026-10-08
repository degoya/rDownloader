use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use utoipa::ToSchema;

use crate::MAX_PERSISTED_BYTES;

/// Byte count serialized as a decimal string for JavaScript safety.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, ToSchema)]
#[schema(value_type = String, example = "4294967296")]
pub struct ByteCount(u64);

impl ByteCount {
    /// Creates a byte count accepted by SQLite persistence.
    pub fn new(value: u64) -> Result<Self, &'static str> {
        if value > MAX_PERSISTED_BYTES {
            return Err("byte count exceeds SQLite INTEGER range");
        }
        Ok(Self(value))
    }

    /// Returns the raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for ByteCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let parsed = value.parse::<u64>().map_err(de::Error::custom)?;
        Self::new(parsed).map_err(de::Error::custom)
    }
}

/// Supported checksum algorithms.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Crc32,
    /// Dropbox's `content_hash` (RD-106-06): SHA-256 over each 4 MiB block of the file, then
    /// SHA-256 over the concatenated block digests. Not a plain digest of the bytes, so it
    /// needs a name of its own — stated as `sha256` it would fail every file it was meant to
    /// verify.
    DropboxContentHash,
}

#[cfg(test)]
mod tests {
    use super::ByteCount;

    #[test]
    fn bytes_are_json_strings() {
        let serialized = serde_json::to_string(&ByteCount::new(4_294_967_296).expect("valid"));
        assert!(matches!(serialized.as_deref(), Ok("\"4294967296\"")));
    }
}
