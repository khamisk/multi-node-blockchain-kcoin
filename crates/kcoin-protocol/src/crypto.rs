use std::{fmt, str::FromStr};

use bech32::{Bech32m, Hrp};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{ADDRESS_HASH_DOMAIN, ValidationError};

const ADDRESS_HRP: &str = "kcoin";
const ADDRESS_LENGTH: usize = 20;
const MAX_CHAIN_ID_BYTES: usize = 64;

/// A validated chain identifier included in every signable object.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize)]
pub struct ChainId(String);

impl ChainId {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let candidate = Self(value);
        candidate.validate()?;
        Ok(candidate)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        let bytes = self.0.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_CHAIN_ID_BYTES
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ValidationError::InvalidChainId);
        }
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ChainId {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// A compact wallet identifier, encoded for humans as canonical Bech32m.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Address([u8; ADDRESS_LENGTH]);

impl Address {
    #[must_use]
    pub fn from_public_key(public_key: &[u8; 32]) -> Self {
        let hash = hash_bytes(ADDRESS_HASH_DOMAIN, public_key);
        let mut bytes = [0_u8; ADDRESS_LENGTH];
        bytes.copy_from_slice(&hash.as_bytes()[..ADDRESS_LENGTH]);
        Self(bytes)
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; ADDRESS_LENGTH]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; ADDRESS_LENGTH] {
        &self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hrp = Hrp::parse(ADDRESS_HRP).map_err(|_| fmt::Error)?;
        let encoded = bech32::encode::<Bech32m>(hrp, &self.0).map_err(|_| fmt::Error)?;
        formatter.write_str(&encoded)
    }
}

impl FromStr for Address {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Generic decode accepts both checksum variants. Re-encoding with
        // Bech32m and requiring byte-for-byte equality enforces the intended
        // variant, lowercase form, HRP, and canonical representation.
        let (hrp, decoded) =
            bech32::decode(value).map_err(|_| ValidationError::MalformedAddress)?;
        if hrp.as_str() != ADDRESS_HRP || decoded.len() != ADDRESS_LENGTH {
            return Err(ValidationError::MalformedAddress);
        }
        let mut bytes = [0_u8; ADDRESS_LENGTH];
        bytes.copy_from_slice(&decoded);
        let address = Self(bytes);
        if address.to_string() != value {
            return Err(ValidationError::MalformedAddress);
        }
        Ok(address)
    }
}

/// A BLAKE3-256 digest used for transaction IDs, block IDs, and roots.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Hash32([u8; 32]);

impl Hash32 {
    pub const ZERO: Self = Self([0_u8; 32]);

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for Hash32 {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
            return Err(ValidationError::Malformed);
        }
        let decoded = hex::decode(value).map_err(|_| ValidationError::Malformed)?;
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }
}

#[must_use]
pub(crate) fn hash_bytes(domain: &'static str, bytes: &[u8]) -> Hash32 {
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(bytes);
    Hash32(*hasher.finalize().as_bytes())
}

#[must_use]
pub(crate) fn hash_pair(domain: &'static str, left: &Hash32, right: &Hash32) -> Hash32 {
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    Hash32(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_canonical_bech32m() {
        let public_key = [7_u8; 32];
        let address = Address::from_public_key(&public_key);
        let encoded = address.to_string();
        assert!(encoded.starts_with("kcoin1"));
        assert_eq!(encoded.parse::<Address>(), Ok(address));
        assert_eq!(
            encoded.to_ascii_uppercase().parse::<Address>(),
            Err(ValidationError::MalformedAddress)
        );
    }

    #[test]
    fn chain_id_has_a_tight_portable_alphabet() {
        assert!(ChainId::new("kcoin-local_1").is_ok());
        assert!(ChainId::new("").is_err());
        assert!(ChainId::new("spaces are not canonical").is_err());
    }

    #[test]
    fn hashes_round_trip_as_hex() {
        let hash = hash_bytes("kcoin.dev/test/hash", b"hello");
        assert_eq!(hash.to_string().parse::<Hash32>(), Ok(hash));
    }
}
