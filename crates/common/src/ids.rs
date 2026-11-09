//! Domain identifiers — wrappers that prevent mixing up instrument IDs,
//! wallet IDs, broker IDs, etc. at compile time.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name([u8; 16]);

        impl $name {
            pub fn new(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
                if bytes.len() != 16 {
                    return None;
                }
                let mut arr = [0u8; 16];
                arr.copy_from_slice(bytes);
                Some(Self(arr))
            }

            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, hex::encode_upper(&self.0[..8]))
            }
        }

        impl FromStr for $name {
            type Err = hex::FromHexError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let stripped = s.strip_prefix($prefix).unwrap_or(s);
                let bytes = hex::decode(stripped)?;
                if bytes.len() != 16 {
                    return Err(hex::FromHexError::InvalidStringLength);
                }
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&bytes);
                Ok(Self(arr))
            }
        }
    };
}

// Venue instrument ids are plain strings (`types::InstrumentId`).
// These opaque wrappers are for internal bookkeeping only.
define_id!(OpaqueAssetId, "ASSET_");
define_id!(WalletId, "WAL_");
define_id!(BrokerId, "BRO_");

/// Unique identifier for a trade execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TradeId([u8; 16]);

impl fmt::Display for TradeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TRADE_{}", hex::encode_upper(&self.0[..8]))
    }
}
