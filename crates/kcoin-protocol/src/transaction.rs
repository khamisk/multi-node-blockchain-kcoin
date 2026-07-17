use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::{
    Address, ChainId, Hash32, MAX_DISPLAY_NAME_BYTES, MAX_DISPLAY_NAME_CHARS,
    MAX_TRANSACTION_BYTES, PROTOCOL_VERSION, TRANSACTION_HASH_DOMAIN, TX_SIGNATURE_PREFIX,
    ValidationError, crypto::hash_bytes,
};

/// The state transition requested by a wallet.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum TransactionAction {
    Transfer {
        recipient: Address,
        amount_atoms: u64,
    },
    ClaimReward {
        challenge_id: u64,
        answer: u16,
    },
    /// Set or clear a public, non-unique, cosmetic explorer label.
    SetDisplayName {
        display_name: Option<String>,
    },
}

impl TransactionAction {
    pub(crate) fn validate_shape(&self) -> Result<(), ValidationError> {
        match self {
            Self::Transfer { amount_atoms, .. } if *amount_atoms == 0 => {
                Err(ValidationError::ZeroAmount)
            }
            Self::SetDisplayName { display_name } => validate_display_name(display_name),
            _ => Ok(()),
        }
    }
}

/// Canonical wallet-authored transaction data.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct UnsignedTransaction {
    pub protocol_version: u16,
    pub chain_id: ChainId,
    pub sender_public_key: [u8; 32],
    pub nonce: u64,
    pub expiry_height: u64,
    pub action: TransactionAction,
}

impl UnsignedTransaction {
    #[must_use]
    pub fn new(
        chain_id: ChainId,
        sender_public_key: [u8; 32],
        nonce: u64,
        expiry_height: u64,
        action: TransactionAction,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            chain_id,
            sender_public_key,
            nonce,
            expiry_height,
            action,
        }
    }

    /// Bytes that Ed25519 signs. The prefix prevents cross-protocol signing.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let canonical =
            borsh::to_vec(self).expect("serializing an in-memory transaction cannot fail");
        let mut bytes = Vec::with_capacity(TX_SIGNATURE_PREFIX.len() + canonical.len());
        bytes.extend_from_slice(TX_SIGNATURE_PREFIX);
        bytes.extend_from_slice(&canonical);
        bytes
    }

    pub fn validate_shape(&self) -> Result<(), ValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedProtocolVersion {
                expected: PROTOCOL_VERSION,
                actual: self.protocol_version,
            });
        }
        self.chain_id.validate()?;
        self.action.validate_shape()
    }
}

/// A canonical transaction plus its wallet's Ed25519 signature.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SignedTransaction {
    pub unsigned: UnsignedTransaction,
    pub signature: [u8; 64],
}

impl SignedTransaction {
    pub fn sign(
        unsigned: UnsignedTransaction,
        signing_key: &SigningKey,
    ) -> Result<Self, ValidationError> {
        if unsigned.sender_public_key != signing_key.verifying_key().to_bytes() {
            return Err(ValidationError::InvalidPublicKey);
        }
        unsigned.validate_shape()?;
        let signature = signing_key.sign(&unsigned.signing_bytes()).to_bytes();
        Ok(Self {
            unsigned,
            signature,
        })
    }

    pub fn from_parts(
        unsigned: UnsignedTransaction,
        signature: [u8; 64],
    ) -> Result<Self, ValidationError> {
        let transaction = Self {
            unsigned,
            signature,
        };
        transaction.verify_signature()?;
        Ok(transaction)
    }

    pub fn verify_signature(&self) -> Result<(), ValidationError> {
        self.unsigned.validate_shape()?;
        let verifying_key = VerifyingKey::from_bytes(&self.unsigned.sender_public_key)
            .map_err(|_| ValidationError::InvalidPublicKey)?;
        let signature = Signature::from_bytes(&self.signature);
        verifying_key
            .verify_strict(&self.unsigned.signing_bytes(), &signature)
            .map_err(|_| ValidationError::InvalidSignature)
    }

    #[must_use]
    pub fn sender_address(&self) -> Address {
        Address::from_public_key(&self.unsigned.sender_public_key)
    }

    #[must_use]
    pub fn id(&self) -> Hash32 {
        hash_bytes(TRANSACTION_HASH_DOMAIN, &self.canonical_bytes())
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("serializing an in-memory transaction cannot fail")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_TRANSACTION_BYTES {
            return Err(ValidationError::PayloadTooLarge);
        }
        let transaction = Self::try_from_slice(bytes).map_err(|_| ValidationError::Malformed)?;
        transaction.verify_signature()?;
        Ok(transaction)
    }
}

pub(crate) fn validate_display_name(name: &Option<String>) -> Result<(), ValidationError> {
    let Some(name) = name else {
        return Ok(());
    };
    if name.is_empty()
        || name.len() > MAX_DISPLAY_NAME_BYTES
        || name.chars().count() > MAX_DISPLAY_NAME_CHARS
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(ValidationError::InvalidDisplayName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32])
    }

    fn transaction() -> SignedTransaction {
        let signing_key = key(1);
        let recipient = Address::from_public_key(&key(2).verifying_key().to_bytes());
        let unsigned = UnsignedTransaction::new(
            ChainId::new("testnet").unwrap(),
            signing_key.verifying_key().to_bytes(),
            0,
            10,
            TransactionAction::Transfer {
                recipient,
                amount_atoms: 1,
            },
        );
        SignedTransaction::sign(unsigned, &signing_key).unwrap()
    }

    #[test]
    fn signature_is_strict_and_covers_every_field() {
        let transaction = transaction();
        assert_eq!(transaction.verify_signature(), Ok(()));
        let mut tampered = transaction;
        tampered.unsigned.nonce = 1;
        assert_eq!(
            tampered.verify_signature(),
            Err(ValidationError::InvalidSignature)
        );
    }

    #[test]
    fn canonical_round_trip_preserves_transaction_id() {
        let transaction = transaction();
        let decoded = SignedTransaction::decode(&transaction.canonical_bytes()).unwrap();
        assert_eq!(decoded, transaction);
        assert_eq!(decoded.id(), transaction.id());
    }

    #[test]
    fn decode_rejects_trailing_or_oversized_data() {
        let transaction = transaction();
        let mut trailing = transaction.canonical_bytes();
        trailing.push(0);
        assert_eq!(
            SignedTransaction::decode(&trailing),
            Err(ValidationError::Malformed)
        );
        assert_eq!(
            SignedTransaction::decode(&vec![0; MAX_TRANSACTION_BYTES + 1]),
            Err(ValidationError::PayloadTooLarge)
        );
    }

    #[test]
    fn display_names_are_bounded_but_cosmetic_and_non_unique() {
        assert_eq!(validate_display_name(&None), Ok(()));
        assert_eq!(validate_display_name(&Some("Ada".into())), Ok(()));
        assert_eq!(
            validate_display_name(&Some(" padded ".into())),
            Err(ValidationError::InvalidDisplayName)
        );
        assert_eq!(
            validate_display_name(&Some("\n".into())),
            Err(ValidationError::InvalidDisplayName)
        );
    }
}
