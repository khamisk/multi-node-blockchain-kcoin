use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::{
    Block, COMMIT_SIGNATURE_PREFIX, ChainId, Hash32, MAX_COMMIT_CERTIFICATE_BYTES,
    PROTOCOL_VERSION, ValidationError,
};

/// The Ed25519 public key that identifies a fixed validator.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct ValidatorId([u8; 32]);

impl ValidatorId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn from_signing_key(key: &SigningKey) -> Self {
        Self(key.verifying_key().to_bytes())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey, ValidationError> {
        VerifyingKey::from_bytes(&self.0).map_err(|_| ValidationError::InvalidPublicKey)
    }
}

/// The canonical finality statement signed by every precommitting validator.
/// `block_hash` is the canonical block identity. A later round can certify the
/// exact same block bytes without changing that identity.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct CommitVote {
    pub protocol_version: u16,
    pub chain_id: ChainId,
    pub height: u64,
    pub round: u32,
    pub block_hash: Hash32,
}

impl CommitVote {
    #[must_use]
    pub fn new(chain_id: ChainId, height: u64, round: u32, block_hash: Hash32) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            chain_id,
            height,
            round,
            block_hash,
        }
    }

    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let canonical = borsh::to_vec(self).expect("serializing an in-memory vote cannot fail");
        let mut bytes = Vec::with_capacity(COMMIT_SIGNATURE_PREFIX.len() + canonical.len());
        bytes.extend_from_slice(COMMIT_SIGNATURE_PREFIX);
        bytes.extend_from_slice(&canonical);
        bytes
    }

    fn validate_shape(&self) -> Result<(), ValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedProtocolVersion {
                expected: PROTOCOL_VERSION,
                actual: self.protocol_version,
            });
        }
        self.chain_id.validate()
    }
}

/// One validator's signature in a commit certificate.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct CommitSignature {
    pub validator: ValidatorId,
    pub signature: [u8; 64],
}

impl CommitSignature {
    #[must_use]
    pub fn sign(vote: &CommitVote, signing_key: &SigningKey) -> Self {
        Self {
            validator: ValidatorId::from_signing_key(signing_key),
            signature: signing_key.sign(&vote.signing_bytes()).to_bytes(),
        }
    }
}

/// Three (in the default four-validator network) or more precommit signatures.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct CommitCertificate {
    pub protocol_version: u16,
    pub chain_id: ChainId,
    pub height: u64,
    pub round: u32,
    pub block_hash: Hash32,
    pub signatures: Vec<CommitSignature>,
}

impl CommitCertificate {
    #[must_use]
    pub fn new(vote: CommitVote, mut signatures: Vec<CommitSignature>) -> Self {
        signatures.sort_by_key(|signature| signature.validator);
        Self {
            protocol_version: vote.protocol_version,
            chain_id: vote.chain_id,
            height: vote.height,
            round: vote.round,
            block_hash: vote.block_hash,
            signatures,
        }
    }

    #[must_use]
    pub fn vote(&self) -> CommitVote {
        CommitVote {
            protocol_version: self.protocol_version,
            chain_id: self.chain_id.clone(),
            height: self.height,
            round: self.round,
            block_hash: self.block_hash,
        }
    }

    pub fn verify(&self, validators: &[ValidatorId]) -> Result<(), ValidationError> {
        if self.canonical_bytes().len() > MAX_COMMIT_CERTIFICATE_BYTES {
            return Err(ValidationError::PayloadTooLarge);
        }
        let vote = self.vote();
        vote.validate_shape()?;

        let authorized: BTreeSet<_> = validators.iter().copied().collect();
        // A duplicated key in genesis would make quorum accounting ambiguous.
        if authorized.is_empty() || authorized.len() != validators.len() {
            return Err(ValidationError::InvalidCommitCertificate);
        }

        let required = quorum_size(authorized.len());
        let mut seen = BTreeSet::new();
        let mut previous = None;
        for commit_signature in &self.signatures {
            if !authorized.contains(&commit_signature.validator) {
                return Err(ValidationError::UnauthorizedValidator);
            }
            if !seen.insert(commit_signature.validator) {
                return Err(ValidationError::DuplicateValidator);
            }
            if previous.is_some_and(|validator| validator >= commit_signature.validator) {
                return Err(ValidationError::InvalidCommitCertificate);
            }
            previous = Some(commit_signature.validator);
            let verifying_key = commit_signature.validator.verifying_key()?;
            let signature = Signature::from_bytes(&commit_signature.signature);
            verifying_key
                .verify_strict(&vote.signing_bytes(), &signature)
                .map_err(|_| ValidationError::InvalidSignature)?;
        }
        if seen.len() < required {
            return Err(ValidationError::InsufficientQuorum {
                required,
                actual: seen.len(),
            });
        }
        Ok(())
    }

    pub fn verify_for_block(
        &self,
        block: &Block,
        validators: &[ValidatorId],
    ) -> Result<(), ValidationError> {
        if self.chain_id != block.header.chain_id
            || self.height != block.header.height
            || self.block_hash != block.consensus_hash()
            || self.round < block.header.round
        {
            return Err(ValidationError::InvalidCommitCertificate);
        }
        let validator_count = validators.len();
        if validator_count == 0 {
            return Err(ValidationError::InvalidCommitCertificate);
        }
        let height_offset = block.header.height.saturating_sub(1) % validator_count as u64;
        // The header's declared slot must follow deterministic rotation. The
        // certificate can come from that construction round or a later round
        // whose proposer re-proposed the locked bytes unchanged.
        let round_offset = u64::from(block.header.round) % validator_count as u64;
        let proposer_index = ((height_offset + round_offset) % validator_count as u64) as usize;
        if block.header.proposer != validators[proposer_index] {
            return Err(ValidationError::InvalidCommitCertificate);
        }
        self.verify(validators)
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("serializing an in-memory certificate cannot fail")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_COMMIT_CERTIFICATE_BYTES {
            return Err(ValidationError::PayloadTooLarge);
        }
        Self::try_from_slice(bytes).map_err(|_| ValidationError::Malformed)
    }
}

/// `floor(2N/3) + 1`, expressed without overflow-prone multiplication.
#[must_use]
pub const fn quorum_size(validator_count: usize) -> usize {
    if validator_count == 0 {
        0
    } else {
        validator_count - ((validator_count - 1) / 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<SigningKey> {
        (1_u8..=4)
            .map(|byte| SigningKey::from_bytes(&[byte; 32]))
            .collect()
    }

    #[test]
    fn four_validator_network_requires_three_signatures() {
        assert_eq!(quorum_size(4), 3);
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let vote = CommitVote::new(
            ChainId::new("testnet").unwrap(),
            7,
            0,
            Hash32::from_bytes([8; 32]),
        );
        let signatures = keys[..3]
            .iter()
            .map(|key| CommitSignature::sign(&vote, key))
            .collect();
        assert_eq!(
            CommitCertificate::new(vote, signatures).verify(&validators),
            Ok(())
        );
    }

    #[test]
    fn duplicate_signer_does_not_count_twice() {
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let vote = CommitVote::new(
            ChainId::new("testnet").unwrap(),
            7,
            0,
            Hash32::from_bytes([8; 32]),
        );
        let signature = CommitSignature::sign(&vote, &keys[0]);
        let certificate =
            CommitCertificate::new(vote, vec![signature.clone(), signature.clone(), signature]);
        assert_eq!(
            certificate.verify(&validators),
            Err(ValidationError::DuplicateValidator)
        );
    }

    #[test]
    fn vote_signature_cannot_be_reused_for_another_block() {
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let vote = CommitVote::new(
            ChainId::new("testnet").unwrap(),
            7,
            0,
            Hash32::from_bytes([8; 32]),
        );
        let signatures = keys[..3]
            .iter()
            .map(|key| CommitSignature::sign(&vote, key))
            .collect();
        let mut certificate = CommitCertificate::new(vote, signatures);
        certificate.block_hash = Hash32::from_bytes([9; 32]);
        assert_eq!(
            certificate.verify(&validators),
            Err(ValidationError::InvalidSignature)
        );
    }

    #[test]
    fn certificates_from_two_rounds_authenticate_one_canonical_block() {
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let block = Block::new(
            ChainId::new("testnet").unwrap(),
            1,
            Hash32::ZERO,
            validators[0],
            0,
            100,
            Hash32::from_bytes([8; 32]),
            Vec::new(),
        );
        let block_id = block.hash();

        let certificate = |round, signers: &[SigningKey]| {
            let vote = CommitVote::new(
                block.header.chain_id.clone(),
                block.header.height,
                round,
                block_id,
            );
            let signatures = signers
                .iter()
                .map(|key| CommitSignature::sign(&vote, key))
                .collect();
            CommitCertificate::new(vote, signatures)
        };
        let round_zero = certificate(0, &keys[..3]);
        let round_one = certificate(1, &keys[1..]);

        assert_eq!(round_zero.verify_for_block(&block, &validators), Ok(()));
        assert_eq!(round_one.verify_for_block(&block, &validators), Ok(()));
        assert_eq!(round_zero.block_hash, round_one.block_hash);

        let mut mutated = block.clone();
        mutated.header.round = 1;
        mutated.header.proposer = validators[1];
        assert_ne!(mutated.hash(), block_id);
        assert_eq!(
            round_one.verify_for_block(&mutated, &validators),
            Err(ValidationError::InvalidCommitCertificate)
        );
    }

    #[test]
    fn noncanonical_signature_order_is_rejected() {
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let vote = CommitVote::new(
            ChainId::new("testnet").unwrap(),
            7,
            0,
            Hash32::from_bytes([8; 32]),
        );
        let signatures = keys[..3]
            .iter()
            .map(|key| CommitSignature::sign(&vote, key))
            .collect();
        let mut certificate = CommitCertificate::new(vote, signatures);
        certificate.signatures.swap(0, 1);
        assert_eq!(
            certificate.verify(&validators),
            Err(ValidationError::InvalidCommitCertificate)
        );
    }

    #[test]
    fn two_of_four_is_not_a_quorum_and_outsiders_are_rejected() {
        let keys = keys();
        let validators: Vec<_> = keys.iter().map(ValidatorId::from_signing_key).collect();
        let vote = CommitVote::new(
            ChainId::new("testnet").unwrap(),
            7,
            0,
            Hash32::from_bytes([8; 32]),
        );
        let two_signatures = keys[..2]
            .iter()
            .map(|key| CommitSignature::sign(&vote, key))
            .collect();
        assert_eq!(
            CommitCertificate::new(vote.clone(), two_signatures).verify(&validators),
            Err(ValidationError::InsufficientQuorum {
                required: 3,
                actual: 2,
            })
        );

        let outsider = SigningKey::from_bytes(&[99; 32]);
        let signatures = vec![
            CommitSignature::sign(&vote, &keys[0]),
            CommitSignature::sign(&vote, &keys[1]),
            CommitSignature::sign(&vote, &outsider),
        ];
        assert_eq!(
            CommitCertificate::new(vote, signatures).verify(&validators),
            Err(ValidationError::UnauthorizedValidator)
        );
    }
}
