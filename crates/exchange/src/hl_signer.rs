//! Hyperliquid L1 action signing (msgpack hash + EIP-712 Agent).
//!
//! Matches hyperliquid-python-sdk `sign_l1_action` / `action_hash`.

use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip712::TypedData;
use ethers::types::{Address, Signature};
use rmpv::Value;
use sha3::{Digest, Keccak256};
use tracing::debug;

use crate::{TradingError, TradingResult};

/// Wire-format float: up to 8 decimals, no scientific notation, no trailing junk.
pub fn float_to_wire(x: f64) -> TradingResult<String> {
    if !x.is_finite() {
        return Err(TradingError::InvalidOrder(format!("non-finite price/size: {x}")));
    }
    let rounded = format!("{x:.8}");
    let parsed: f64 = rounded.parse().unwrap_or(x);
    if (parsed - x).abs() >= 1e-12 && x.abs() > 1e-12 {
        // allow tiny float noise on large numbers
        if (parsed - x).abs() / x.abs() > 1e-10 {
            return Err(TradingError::InvalidOrder(format!(
                "float_to_wire rounding: {x} -> {rounded}"
            )));
        }
    }
    let mut s = rounded;
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    Ok(s)
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let out = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn address_bytes(addr: &str) -> TradingResult<[u8; 20]> {
    let h = addr.trim().trim_start_matches("0x");
    let bytes = hex::decode(h).map_err(|e| TradingError::Signing(e.to_string()))?;
    if bytes.len() != 20 {
        return Err(TradingError::Signing(format!(
            "address must be 20 bytes, got {}",
            bytes.len()
        )));
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&bytes);
    Ok(a)
}

/// Hash of action for phantom agent connectionId.
pub fn action_hash(
    action: &Value,
    vault_address: Option<&str>,
    nonce: u64,
    expires_after: Option<u64>,
) -> TradingResult<[u8; 32]> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, action)
        .map_err(|e| TradingError::Signing(format!("msgpack: {e}")))?;
    buf.extend_from_slice(&nonce.to_be_bytes());
    match vault_address {
        None => buf.push(0x00),
        Some(v) => {
            buf.push(0x01);
            buf.extend_from_slice(&address_bytes(v)?);
        }
    }
    if let Some(exp) = expires_after {
        buf.push(0x00);
        buf.extend_from_slice(&exp.to_be_bytes());
    }
    Ok(keccak256(&buf))
}

/// Build limit order action (msgpack map with SDK field order).
pub fn order_action_value(
    asset: u32,
    is_buy: bool,
    price: f64,
    size: f64,
    reduce_only: bool,
    tif: &str,
) -> TradingResult<Value> {
    let p = float_to_wire(price)?;
    let s = float_to_wire(size)?;
    let order = Value::Map(vec![
        (Value::String("a".into()), Value::from(asset)),
        (Value::String("b".into()), Value::Boolean(is_buy)),
        (Value::String("p".into()), Value::String(p.into())),
        (Value::String("s".into()), Value::String(s.into())),
        (Value::String("r".into()), Value::Boolean(reduce_only)),
        (
            Value::String("t".into()),
            Value::Map(vec![(
                Value::String("limit".into()),
                Value::Map(vec![(
                    Value::String("tif".into()),
                    Value::String(tif.into()),
                )]),
            )]),
        ),
    ]);
    Ok(Value::Map(vec![
        (Value::String("type".into()), Value::String("order".into())),
        (Value::String("orders".into()), Value::Array(vec![order])),
        (Value::String("grouping".into()), Value::String("na".into())),
    ]))
}

pub fn cancel_action_value(asset: u32, oid: u64) -> Value {
    let cancel = Value::Map(vec![
        (Value::String("a".into()), Value::from(asset)),
        (Value::String("o".into()), Value::from(oid)),
    ]);
    Value::Map(vec![
        (Value::String("type".into()), Value::String("cancel".into())),
        (Value::String("cancels".into()), Value::Array(vec![cancel])),
    ])
}

/// Sign L1 action; returns (r, s, v) as hex strings + v int for JSON API.
pub async fn sign_l1_action(
    wallet: &LocalWallet,
    action: &Value,
    vault_address: Option<&str>,
    nonce: u64,
    expires_after: Option<u64>,
    is_mainnet: bool,
) -> TradingResult<HlSignature> {
    let hash = action_hash(action, vault_address, nonce, expires_after)?;
    let source = if is_mainnet { "a" } else { "b" };
    let connection_id = format!("0x{}", hex::encode(hash));

    // EIP-712 TypedData matching Python l1_payload
    let typed_json = serde_json::json!({
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ],
            "Agent": [
                {"name": "source", "type": "string"},
                {"name": "connectionId", "type": "bytes32"}
            ]
        },
        "primaryType": "Agent",
        "domain": {
            "name": "Exchange",
            "version": "1",
            "chainId": 1337,
            "verifyingContract": "0x0000000000000000000000000000000000000000"
        },
        "message": {
            "source": source,
            "connectionId": connection_id
        }
    });

    let typed: TypedData = serde_json::from_value(typed_json)
        .map_err(|e| TradingError::Signing(format!("typed data: {e}")))?;

    let sig: Signature = wallet
        .sign_typed_data(&typed)
        .await
        .map_err(|e| TradingError::Signing(format!("sign_typed_data: {e}")))?;

    debug!(
        "HL L1 signed by {:?} v={}",
        wallet.address(),
        sig.v
    );

    Ok(HlSignature {
        r: format!("{:#x}", sig.r),
        s: format!("{:#x}", sig.s),
        v: sig.v as u8,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HlSignature {
    pub r: String,
    pub s: String,
    pub v: u8,
}

/// Parse private key from env-style hex (with or without 0x).
pub fn wallet_from_private_key(pk: &str) -> TradingResult<LocalWallet> {
    let pk = pk.trim().trim_start_matches("0x");
    pk.parse::<LocalWallet>()
        .map_err(|e| TradingError::Signing(format!("invalid private key: {e}")))
}

/// Recover address from signature for tests (optional).
#[allow(dead_code)]
pub fn dummy_address() -> Address {
    Address::zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_wire_strips_zeros() {
        assert_eq!(float_to_wire(1.5).unwrap(), "1.5");
        assert_eq!(float_to_wire(100.0).unwrap(), "100");
        assert_eq!(float_to_wire(0.0001).unwrap(), "0.0001");
    }

    #[test]
    fn order_action_msgpacks() {
        let v = order_action_value(0, true, 100.5, 0.01, false, "Ioc").unwrap();
        let h = action_hash(&v, None, 1_700_000_000_000, None).unwrap();
        assert_eq!(h.len(), 32);
        // deterministic
        let h2 = action_hash(&v, None, 1_700_000_000_000, None).unwrap();
        assert_eq!(h, h2);
    }
}
