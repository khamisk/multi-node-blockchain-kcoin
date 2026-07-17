//! Narrow browser bindings. JavaScript keeps custody of the private key and
//! uses these exports only to reproduce Rust's address and signing formats.

use std::str::FromStr;

use wasm_bindgen::prelude::*;

use crate::{Address, ChainId, TransactionAction, UnsignedTransaction, ValidationError};

fn public_key(bytes: &[u8]) -> Result<[u8; 32], JsValue> {
    bytes
        .try_into()
        .map_err(|_| JsValue::from_str("public key must contain exactly 32 bytes"))
}

fn js_error(error: ValidationError) -> JsValue {
    JsValue::from_str(&format!("{}: {error}", error.code()))
}

#[wasm_bindgen]
pub fn wasm_address_from_public_key(public_key_bytes: &[u8]) -> Result<String, JsValue> {
    let public_key = public_key(public_key_bytes)?;
    Ok(Address::from_public_key(&public_key).to_string())
}

#[wasm_bindgen]
pub fn wasm_unsigned_transfer_signing_bytes(
    chain_id: String,
    sender_public_key: &[u8],
    nonce: u64,
    expiry_height: u64,
    recipient: String,
    amount_atoms: u64,
) -> Result<Vec<u8>, JsValue> {
    let transaction = UnsignedTransaction::new(
        ChainId::new(chain_id).map_err(js_error)?,
        public_key(sender_public_key)?,
        nonce,
        expiry_height,
        TransactionAction::Transfer {
            recipient: Address::from_str(&recipient).map_err(js_error)?,
            amount_atoms,
        },
    );
    transaction.validate_shape().map_err(js_error)?;
    Ok(transaction.signing_bytes())
}

#[wasm_bindgen]
pub fn wasm_unsigned_claim_reward_signing_bytes(
    chain_id: String,
    sender_public_key: &[u8],
    nonce: u64,
    expiry_height: u64,
    challenge_id: u64,
    answer: u16,
) -> Result<Vec<u8>, JsValue> {
    let transaction = UnsignedTransaction::new(
        ChainId::new(chain_id).map_err(js_error)?,
        public_key(sender_public_key)?,
        nonce,
        expiry_height,
        TransactionAction::ClaimReward {
            challenge_id,
            answer,
        },
    );
    transaction.validate_shape().map_err(js_error)?;
    Ok(transaction.signing_bytes())
}

#[wasm_bindgen]
pub fn wasm_unsigned_set_display_name_signing_bytes(
    chain_id: String,
    sender_public_key: &[u8],
    nonce: u64,
    expiry_height: u64,
    display_name: Option<String>,
) -> Result<Vec<u8>, JsValue> {
    let transaction = UnsignedTransaction::new(
        ChainId::new(chain_id).map_err(js_error)?,
        public_key(sender_public_key)?,
        nonce,
        expiry_height,
        TransactionAction::SetDisplayName { display_name },
    );
    transaction.validate_shape().map_err(js_error)?;
    Ok(transaction.signing_bytes())
}
