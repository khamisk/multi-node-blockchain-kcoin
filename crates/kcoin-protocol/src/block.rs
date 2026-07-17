use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    BLOCK_HASH_DOMAIN, ChainId, Hash32, MAX_BLOCK_BYTES, MERKLE_EMPTY_DOMAIN, MERKLE_LEAF_DOMAIN,
    MERKLE_NODE_DOMAIN, PROTOCOL_VERSION, SignedTransaction, ValidationError, ValidatorId,
    crypto::{hash_bytes, hash_pair},
};

/// Canonical block metadata, including the declared proposer slot and
/// construction round.
///
/// A later proposer carrying a locked value into a new round must re-propose
/// these exact bytes. The later signed proposal authenticates its current
/// carrier, while the separate commit certificate records the round in which
/// a quorum actually finalized them.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub protocol_version: u16,
    pub chain_id: ChainId,
    pub height: u64,
    pub parent_hash: Hash32,
    pub proposer: ValidatorId,
    pub round: u32,
    pub timestamp_ms: u64,
    pub transactions_root: Hash32,
    pub state_root: Hash32,
}

/// An ordered batch of transactions and the header that commits to them.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<SignedTransaction>,
}

impl Block {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: ChainId,
        height: u64,
        parent_hash: Hash32,
        proposer: ValidatorId,
        round: u32,
        timestamp_ms: u64,
        state_root: Hash32,
        transactions: Vec<SignedTransaction>,
    ) -> Self {
        let transactions_root = merkle_root(&transactions);
        Self {
            header: BlockHeader {
                protocol_version: PROTOCOL_VERSION,
                chain_id,
                height,
                parent_hash,
                proposer,
                round,
                timestamp_ms,
                transactions_root,
                state_root,
            },
            transactions,
        }
    }

    /// Canonical finalized block hash used by parent links and explorer IDs.
    #[must_use]
    pub fn hash(&self) -> Hash32 {
        let bytes =
            borsh::to_vec(&self.header).expect("serializing an in-memory block header cannot fail");
        hash_bytes(BLOCK_HASH_DOMAIN, &bytes)
    }

    /// Stable consensus value across valid-round reproposals.
    ///
    /// Re-proposal carries the original block bytes unchanged, so the value
    /// voted on by consensus is exactly the canonical block identity used by
    /// parent links, storage, synchronization, and the explorer.
    #[must_use]
    pub fn consensus_hash(&self) -> Hash32 {
        self.hash()
    }

    pub fn validate_commitments(&self) -> Result<(), ValidationError> {
        if self.canonical_bytes().len() > MAX_BLOCK_BYTES {
            return Err(ValidationError::PayloadTooLarge);
        }
        if self.header.protocol_version != PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedProtocolVersion {
                expected: PROTOCOL_VERSION,
                actual: self.header.protocol_version,
            });
        }
        self.header.chain_id.validate()?;
        if merkle_root(&self.transactions) != self.header.transactions_root {
            return Err(ValidationError::InvalidTransactionsRoot);
        }
        Ok(())
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("serializing an in-memory block cannot fail")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_BLOCK_BYTES {
            return Err(ValidationError::PayloadTooLarge);
        }
        let block = Self::try_from_slice(bytes).map_err(|_| ValidationError::Malformed)?;
        block.validate_commitments()?;
        Ok(block)
    }
}

/// Deterministic binary Merkle root over transaction IDs.
///
/// Odd levels duplicate their final node. Empty blocks have a domain-separated
/// constant root, so no valid leaf or interior node can equal the empty root by
/// construction.
#[must_use]
pub fn merkle_root(transactions: &[SignedTransaction]) -> Hash32 {
    if transactions.is_empty() {
        return hash_bytes(MERKLE_EMPTY_DOMAIN, &[]);
    }

    let mut level: Vec<Hash32> = transactions
        .iter()
        .map(|transaction| hash_bytes(MERKLE_LEAF_DOMAIN, transaction.id().as_bytes()))
        .collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let left = pair[0];
            let right = pair.get(1).copied().unwrap_or(left);
            next.push(hash_pair(MERKLE_NODE_DOMAIN, &left, &right));
        }
        level = next;
    }
    level[0]
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::{Address, TransactionAction, UnsignedTransaction};

    fn transaction(nonce: u64) -> SignedTransaction {
        let key = SigningKey::from_bytes(&[3; 32]);
        SignedTransaction::sign(
            UnsignedTransaction::new(
                ChainId::new("testnet").unwrap(),
                key.verifying_key().to_bytes(),
                nonce,
                100,
                TransactionAction::Transfer {
                    recipient: Address::from_bytes([4; 20]),
                    amount_atoms: nonce + 1,
                },
            ),
            &key,
        )
        .unwrap()
    }

    #[test]
    fn merkle_root_commits_to_order_and_content() {
        let first = transaction(0);
        let second = transaction(1);
        assert_ne!(merkle_root(&[]), merkle_root(std::slice::from_ref(&first)));
        assert_ne!(
            merkle_root(&[first.clone(), second.clone()]),
            merkle_root(&[second, first])
        );
    }

    #[test]
    fn consensus_value_is_the_canonical_block_hash() {
        let mut block = Block::new(
            ChainId::new("testnet").unwrap(),
            1,
            Hash32::ZERO,
            ValidatorId::from_bytes([9; 32]),
            0,
            100,
            Hash32::from_bytes([8; 32]),
            vec![transaction(0)],
        );
        assert_eq!(block.consensus_hash(), block.hash());
        let original = block.consensus_hash();
        block.header.round = 1;
        block.header.proposer = ValidatorId::from_bytes([7; 32]);
        assert_ne!(block.consensus_hash(), original);
        assert_eq!(block.consensus_hash(), block.hash());
    }

    #[test]
    fn tampered_block_is_rejected() {
        let mut block = Block::new(
            ChainId::new("testnet").unwrap(),
            1,
            Hash32::ZERO,
            ValidatorId::from_bytes([9; 32]),
            0,
            100,
            Hash32::from_bytes([8; 32]),
            vec![transaction(0)],
        );
        block.transactions.push(transaction(1));
        assert_eq!(
            block.validate_commitments(),
            Err(ValidationError::InvalidTransactionsRoot)
        );
    }
}
