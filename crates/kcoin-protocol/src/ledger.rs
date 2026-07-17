use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    ATOMS_PER_KCOIN, Block, CHALLENGE_HASH_DOMAIN, ChainId, CommitCertificate, Hash32,
    MAX_SUPPLY_ATOMS, PROTOCOL_VERSION, SignedTransaction, TransactionAction, ValidationError,
    ValidatorId, crypto::hash_bytes,
};

const REWARD_BAND_ATOMS: u64 = 20_000 * ATOMS_PER_KCOIN;
const REWARDS_KCOIN: [u64; 5] = [100, 50, 25, 10, 5];

/// Consensus state associated with one wallet address.
#[derive(Clone, Debug, Default, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountState {
    pub balance_atoms: u64,
    /// The next nonce this account must sign. New accounts start at zero.
    pub next_nonce: u64,
    /// Public, non-unique explorer metadata. It never replaces the address.
    pub display_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub enum ChallengeOperation {
    Add,
    Subtract,
    Multiply,
}

impl ChallengeOperation {
    #[must_use]
    pub const fn symbol(self) -> char {
        match self {
            Self::Add => '+',
            Self::Subtract => '-',
            Self::Multiply => '×',
        }
    }
}

/// The single persistent arithmetic challenge currently open for issuance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Challenge {
    pub id: u64,
    pub operation: ChallengeOperation,
    pub left: u8,
    pub right: u8,
}

impl Challenge {
    /// Challenge content is a pure function of its ID, so all nodes generate it
    /// without randomness, clocks, or shared mutable state.
    #[must_use]
    pub fn for_id(id: u64) -> Self {
        let digest = hash_bytes(CHALLENGE_HASH_DOMAIN, &id.to_le_bytes());
        let bytes = digest.as_bytes();
        let operation = match bytes[0] % 3 {
            0 => ChallengeOperation::Add,
            1 => ChallengeOperation::Subtract,
            _ => ChallengeOperation::Multiply,
        };
        let mut left = 1 + (bytes[1] % 9);
        let mut right = 1 + (bytes[2] % 9);
        if operation == ChallengeOperation::Subtract && right > left {
            std::mem::swap(&mut left, &mut right);
        }
        Self {
            id,
            operation,
            left,
            right,
        }
    }

    #[must_use]
    pub const fn answer(self) -> u16 {
        match self.operation {
            ChallengeOperation::Add => self.left as u16 + self.right as u16,
            ChallengeOperation::Subtract => self.left as u16 - self.right as u16,
            ChallengeOperation::Multiply => self.left as u16 * self.right as u16,
        }
    }
}

/// Information returned after validation but before any state mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedTransaction {
    pub id: Hash32,
    pub sender: crate::Address,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum TransactionOutcome {
    Transfer {
        recipient: crate::Address,
        amount_atoms: u64,
    },
    RewardClaimed {
        challenge_id: u64,
        reward_atoms: u64,
    },
    DisplayNameUpdated {
        display_name: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TransactionReceipt {
    pub transaction_id: Hash32,
    pub sender: crate::Address,
    pub nonce: u64,
    pub outcome: TransactionOutcome,
}

/// Complete deterministic ledger state for one chain.
///
/// `tip_hash` is deliberately excluded from `state_root()`: the block header
/// contains the state root and determines the block/tip hash, so including it
/// would create a circular commitment.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LedgerState {
    chain_id: ChainId,
    height: u64,
    tip_hash: Hash32,
    last_block_timestamp_ms: u64,
    total_supply_atoms: u64,
    active_challenge_id: u64,
    accounts: BTreeMap<crate::Address, AccountState>,
    applied_transactions: BTreeSet<Hash32>,
}

#[derive(BorshSerialize)]
struct StateCommitment {
    protocol_version: u16,
    chain_id: ChainId,
    height: u64,
    last_block_timestamp_ms: u64,
    total_supply_atoms: u64,
    active_challenge_id: u64,
    accounts: BTreeMap<crate::Address, AccountState>,
    applied_transactions: BTreeSet<Hash32>,
}

impl LedgerState {
    #[must_use]
    pub fn new(chain_id: ChainId) -> Self {
        Self {
            chain_id,
            height: 0,
            tip_hash: Hash32::ZERO,
            last_block_timestamp_ms: 0,
            total_supply_atoms: 0,
            active_challenge_id: 0,
            accounts: BTreeMap::new(),
            applied_transactions: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn chain_id(&self) -> &ChainId {
        &self.chain_id
    }

    #[must_use]
    pub const fn height(&self) -> u64 {
        self.height
    }

    #[must_use]
    pub const fn tip_hash(&self) -> Hash32 {
        self.tip_hash
    }

    #[must_use]
    pub const fn last_block_timestamp_ms(&self) -> u64 {
        self.last_block_timestamp_ms
    }

    #[must_use]
    pub const fn total_supply_atoms(&self) -> u64 {
        self.total_supply_atoms
    }

    #[must_use]
    pub const fn active_challenge_id(&self) -> u64 {
        self.active_challenge_id
    }

    #[must_use]
    pub fn current_challenge(&self) -> Challenge {
        Challenge::for_id(self.active_challenge_id)
    }

    #[must_use]
    pub fn account(&self, address: &crate::Address) -> Option<&AccountState> {
        self.accounts.get(address)
    }

    #[must_use]
    pub fn account_or_default(&self, address: &crate::Address) -> AccountState {
        self.accounts.get(address).cloned().unwrap_or_default()
    }

    #[must_use]
    pub fn accounts(&self) -> &BTreeMap<crate::Address, AccountState> {
        &self.accounts
    }

    #[must_use]
    pub fn contains_transaction(&self, transaction_id: &Hash32) -> bool {
        self.applied_transactions.contains(transaction_id)
    }

    #[must_use]
    pub fn applied_transaction_count(&self) -> usize {
        self.applied_transactions.len()
    }

    #[must_use]
    pub fn state_root(&self) -> Hash32 {
        let commitment = StateCommitment {
            protocol_version: PROTOCOL_VERSION,
            chain_id: self.chain_id.clone(),
            height: self.height,
            last_block_timestamp_ms: self.last_block_timestamp_ms,
            total_supply_atoms: self.total_supply_atoms,
            active_challenge_id: self.active_challenge_id,
            accounts: self.accounts.clone(),
            applied_transactions: self.applied_transactions.clone(),
        };
        let bytes = borsh::to_vec(&commitment)
            .expect("serializing deterministic in-memory state cannot fail");
        hash_bytes(crate::STATE_HASH_DOMAIN, &bytes)
    }

    pub fn validate_transaction(
        &self,
        transaction: &SignedTransaction,
        execution_height: u64,
    ) -> Result<ValidatedTransaction, ValidationError> {
        transaction.verify_signature()?;
        if transaction.unsigned.chain_id != self.chain_id {
            return Err(ValidationError::WrongChain {
                expected: self.chain_id.to_string(),
                actual: transaction.unsigned.chain_id.to_string(),
            });
        }

        let id = transaction.id();
        if self.applied_transactions.contains(&id) {
            return Err(ValidationError::DuplicateTransaction);
        }
        if transaction.unsigned.expiry_height < execution_height {
            return Err(ValidationError::Expired {
                expiry_height: transaction.unsigned.expiry_height,
                current_height: execution_height,
            });
        }

        let sender = transaction.sender_address();
        let sender_account = self.accounts.get(&sender).cloned().unwrap_or_default();
        if transaction.unsigned.nonce != sender_account.next_nonce {
            return Err(ValidationError::NonceMismatch {
                expected: sender_account.next_nonce,
                actual: transaction.unsigned.nonce,
            });
        }
        sender_account
            .next_nonce
            .checked_add(1)
            .ok_or(ValidationError::ArithmeticOverflow)?;

        match &transaction.unsigned.action {
            TransactionAction::Transfer { amount_atoms, .. } => {
                if *amount_atoms == 0 {
                    return Err(ValidationError::ZeroAmount);
                }
                if sender_account.balance_atoms < *amount_atoms {
                    return Err(ValidationError::InsufficientBalance {
                        available: sender_account.balance_atoms,
                        required: *amount_atoms,
                    });
                }
            }
            TransactionAction::ClaimReward {
                challenge_id,
                answer,
            } => {
                if *challenge_id != self.active_challenge_id {
                    return Err(ValidationError::StaleChallenge {
                        expected: self.active_challenge_id,
                        actual: *challenge_id,
                    });
                }
                if *answer != self.current_challenge().answer() {
                    return Err(ValidationError::WrongChallengeAnswer);
                }
                let reward = reward_for_supply(self.total_supply_atoms)
                    .ok_or(ValidationError::SupplyExhausted)?;
                sender_account
                    .balance_atoms
                    .checked_add(reward)
                    .ok_or(ValidationError::ArithmeticOverflow)?;
                self.active_challenge_id
                    .checked_add(1)
                    .ok_or(ValidationError::ArithmeticOverflow)?;
            }
            TransactionAction::SetDisplayName { display_name } => {
                crate::transaction::validate_display_name(display_name)?;
            }
        }

        Ok(ValidatedTransaction { id, sender })
    }

    /// Validate and atomically apply one transaction.
    pub fn apply_transaction(
        &mut self,
        transaction: &SignedTransaction,
        execution_height: u64,
    ) -> Result<TransactionReceipt, ValidationError> {
        let validated = self.validate_transaction(transaction, execution_height)?;
        let mut sender_account = self
            .accounts
            .get(&validated.sender)
            .cloned()
            .unwrap_or_default();
        let next_nonce = sender_account
            .next_nonce
            .checked_add(1)
            .ok_or(ValidationError::ArithmeticOverflow)?;

        let outcome = match &transaction.unsigned.action {
            TransactionAction::Transfer {
                recipient,
                amount_atoms,
            } => {
                if recipient != &validated.sender {
                    let mut recipient_account =
                        self.accounts.get(recipient).cloned().unwrap_or_default();
                    let recipient_balance = recipient_account
                        .balance_atoms
                        .checked_add(*amount_atoms)
                        .ok_or(ValidationError::ArithmeticOverflow)?;
                    let sender_balance = sender_account
                        .balance_atoms
                        .checked_sub(*amount_atoms)
                        .ok_or(ValidationError::ArithmeticOverflow)?;
                    sender_account.balance_atoms = sender_balance;
                    recipient_account.balance_atoms = recipient_balance;
                    self.accounts.insert(*recipient, recipient_account);
                }
                TransactionOutcome::Transfer {
                    recipient: *recipient,
                    amount_atoms: *amount_atoms,
                }
            }
            TransactionAction::ClaimReward { challenge_id, .. } => {
                let reward_atoms = reward_for_supply(self.total_supply_atoms)
                    .ok_or(ValidationError::SupplyExhausted)?;
                let new_balance = sender_account
                    .balance_atoms
                    .checked_add(reward_atoms)
                    .ok_or(ValidationError::ArithmeticOverflow)?;
                let new_supply = self
                    .total_supply_atoms
                    .checked_add(reward_atoms)
                    .ok_or(ValidationError::ArithmeticOverflow)?;
                let next_challenge = self
                    .active_challenge_id
                    .checked_add(1)
                    .ok_or(ValidationError::ArithmeticOverflow)?;
                sender_account.balance_atoms = new_balance;
                self.total_supply_atoms = new_supply;
                self.active_challenge_id = next_challenge;
                TransactionOutcome::RewardClaimed {
                    challenge_id: *challenge_id,
                    reward_atoms,
                }
            }
            TransactionAction::SetDisplayName { display_name } => {
                sender_account.display_name.clone_from(display_name);
                TransactionOutcome::DisplayNameUpdated {
                    display_name: display_name.clone(),
                }
            }
        };

        sender_account.next_nonce = next_nonce;
        self.accounts.insert(validated.sender, sender_account);
        self.applied_transactions.insert(validated.id);

        Ok(TransactionReceipt {
            transaction_id: validated.id,
            sender: validated.sender,
            nonce: transaction.unsigned.nonce,
            outcome,
        })
    }

    /// Execute transactions against a clone and create a self-consistent block.
    /// The original state is unchanged until `apply_block` is called after finality.
    pub fn build_block(
        &self,
        proposer: ValidatorId,
        round: u32,
        timestamp_ms: u64,
        transactions: Vec<SignedTransaction>,
    ) -> Result<Block, ValidationError> {
        let height = self
            .height
            .checked_add(1)
            .ok_or(ValidationError::ArithmeticOverflow)?;
        let timestamp_ms = self.normalized_proposal_timestamp(timestamp_ms);
        let mut candidate = self.clone();
        for transaction in &transactions {
            candidate.apply_transaction(transaction, height)?;
        }
        candidate.height = height;
        candidate.last_block_timestamp_ms = timestamp_ms;
        Ok(Block::new(
            self.chain_id.clone(),
            height,
            self.tip_hash,
            proposer,
            round,
            timestamp_ms,
            candidate.state_root(),
            transactions,
        ))
    }

    /// Validate and atomically apply a finalized block.
    pub fn apply_block(
        &mut self,
        block: &Block,
    ) -> Result<Vec<TransactionReceipt>, ValidationError> {
        block.validate_commitments()?;
        if block.header.chain_id != self.chain_id {
            return Err(ValidationError::WrongChain {
                expected: self.chain_id.to_string(),
                actual: block.header.chain_id.to_string(),
            });
        }
        let expected_height = self
            .height
            .checked_add(1)
            .ok_or(ValidationError::ArithmeticOverflow)?;
        if block.header.height != expected_height {
            return Err(ValidationError::InvalidBlockHeight {
                expected: expected_height,
                actual: block.header.height,
            });
        }
        if block.header.parent_hash != self.tip_hash {
            return Err(ValidationError::InvalidParentHash);
        }
        if block.header.timestamp_ms > crate::MAX_BLOCK_TIMESTAMP_MS
            || (self.height > 0
                && (block.header.timestamp_ms < self.last_block_timestamp_ms
                    || block.header.timestamp_ms
                        > self
                            .last_block_timestamp_ms
                            .saturating_add(crate::MAX_BLOCK_TIMESTAMP_STEP_MS)))
        {
            return Err(ValidationError::InvalidTimestamp);
        }

        let mut candidate = self.clone();
        let mut receipts = Vec::with_capacity(block.transactions.len());
        for transaction in &block.transactions {
            receipts.push(candidate.apply_transaction(transaction, block.header.height)?);
        }
        candidate.height = block.header.height;
        candidate.last_block_timestamp_ms = block.header.timestamp_ms;
        if candidate.state_root() != block.header.state_root {
            return Err(ValidationError::InvalidStateRoot);
        }
        candidate.tip_hash = block.hash();
        *self = candidate;
        Ok(receipts)
    }

    fn normalized_proposal_timestamp(&self, requested_ms: u64) -> u64 {
        if self.height == 0 {
            return requested_ms.min(crate::MAX_BLOCK_TIMESTAMP_MS);
        }
        requested_ms
            .max(self.last_block_timestamp_ms)
            .min(
                self.last_block_timestamp_ms
                    .saturating_add(crate::MAX_BLOCK_TIMESTAMP_STEP_MS),
            )
            .min(crate::MAX_BLOCK_TIMESTAMP_MS)
    }

    /// Certificate verification plus atomic block application for sync/replay.
    pub fn apply_finalized_block(
        &mut self,
        block: &Block,
        certificate: &CommitCertificate,
        validators: &[ValidatorId],
    ) -> Result<Vec<TransactionReceipt>, ValidationError> {
        certificate.verify_for_block(block, validators)?;
        self.apply_block(block)
    }
}

/// Reward for the next successful challenge claim, capped at maximum supply.
#[must_use]
pub const fn reward_for_supply(total_supply_atoms: u64) -> Option<u64> {
    if total_supply_atoms >= MAX_SUPPLY_ATOMS {
        return None;
    }
    let band = (total_supply_atoms / REWARD_BAND_ATOMS) as usize;
    let reward_kcoin = REWARDS_KCOIN[if band < REWARDS_KCOIN.len() {
        band
    } else {
        REWARDS_KCOIN.len() - 1
    }];
    let reward_atoms = reward_kcoin * ATOMS_PER_KCOIN;
    let remaining = MAX_SUPPLY_ATOMS - total_supply_atoms;
    Some(if reward_atoms < remaining {
        reward_atoms
    } else {
        remaining
    })
}

/// Deterministically reconstruct all ledger state from canonical blocks.
pub fn replay_blocks(chain_id: ChainId, blocks: &[Block]) -> Result<LedgerState, ValidationError> {
    let mut state = LedgerState::new(chain_id);
    for block in blocks {
        state.apply_block(block)?;
    }
    Ok(state)
}

/// Reconstruct state while also authenticating every quorum certificate.
pub fn replay_finalized_blocks(
    chain_id: ChainId,
    history: &[(Block, CommitCertificate)],
    validators: &[ValidatorId],
) -> Result<LedgerState, ValidationError> {
    let mut state = LedgerState::new(chain_id);
    for (block, certificate) in history {
        state.apply_finalized_block(block, certificate, validators)?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use proptest::prelude::*;

    use super::*;
    use crate::{Address, UnsignedTransaction};

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32])
    }

    fn sign(
        chain_id: &ChainId,
        key: &SigningKey,
        nonce: u64,
        action: TransactionAction,
    ) -> SignedTransaction {
        SignedTransaction::sign(
            UnsignedTransaction::new(
                chain_id.clone(),
                key.verifying_key().to_bytes(),
                nonce,
                100,
                action,
            ),
            key,
        )
        .unwrap()
    }

    fn claim(state: &LedgerState, key: &SigningKey, nonce: u64) -> SignedTransaction {
        let challenge = state.current_challenge();
        sign(
            state.chain_id(),
            key,
            nonce,
            TransactionAction::ClaimReward {
                challenge_id: challenge.id,
                answer: challenge.answer(),
            },
        )
    }

    #[test]
    fn earn_name_and_transfer_flow_uses_addresses_for_value() {
        let chain_id = ChainId::new("testnet").unwrap();
        let alice_key = key(1);
        let bob_key = key(2);
        let alice = Address::from_public_key(&alice_key.verifying_key().to_bytes());
        let bob = Address::from_public_key(&bob_key.verifying_key().to_bytes());
        let mut state = LedgerState::new(chain_id.clone());

        let reward = claim(&state, &alice_key, 0);
        state.apply_transaction(&reward, 1).unwrap();
        let name = sign(
            &chain_id,
            &alice_key,
            1,
            TransactionAction::SetDisplayName {
                display_name: Some("Alice".into()),
            },
        );
        state.apply_transaction(&name, 1).unwrap();
        let transfer = sign(
            &chain_id,
            &alice_key,
            2,
            TransactionAction::Transfer {
                recipient: bob,
                amount_atoms: 25 * ATOMS_PER_KCOIN,
            },
        );
        state.apply_transaction(&transfer, 1).unwrap();

        assert_eq!(
            state.account(&alice).unwrap().display_name.as_deref(),
            Some("Alice")
        );
        assert_eq!(
            state.account(&alice).unwrap().balance_atoms,
            75 * ATOMS_PER_KCOIN
        );
        assert_eq!(
            state.account(&bob).unwrap().balance_atoms,
            25 * ATOMS_PER_KCOIN
        );
        assert_eq!(state.total_supply_atoms(), 100 * ATOMS_PER_KCOIN);
    }

    #[test]
    fn replay_nonce_expiry_overspend_and_forgery_do_not_mutate_state() {
        let chain_id = ChainId::new("testnet").unwrap();
        let alice_key = key(1);
        let bob = Address::from_public_key(&key(2).verifying_key().to_bytes());
        let mut state = LedgerState::new(chain_id.clone());
        state
            .apply_transaction(&claim(&state, &alice_key, 0), 1)
            .unwrap();

        let valid = sign(
            &chain_id,
            &alice_key,
            1,
            TransactionAction::Transfer {
                recipient: bob,
                amount_atoms: 1,
            },
        );
        let before = state.clone();
        let mut forged = valid.clone();
        forged.signature[0] ^= 1;
        assert_eq!(
            state.apply_transaction(&forged, 2),
            Err(ValidationError::InvalidSignature)
        );
        assert_eq!(state, before);

        let overspend = sign(
            &chain_id,
            &alice_key,
            1,
            TransactionAction::Transfer {
                recipient: bob,
                amount_atoms: MAX_SUPPLY_ATOMS,
            },
        );
        assert!(matches!(
            state.apply_transaction(&overspend, 2),
            Err(ValidationError::InsufficientBalance { .. })
        ));
        assert_eq!(state, before);

        let mut expired = valid.clone();
        expired.unsigned.expiry_height = 1;
        expired = SignedTransaction::sign(expired.unsigned, &alice_key).unwrap();
        assert!(matches!(
            state.apply_transaction(&expired, 2),
            Err(ValidationError::Expired { .. })
        ));
        assert_eq!(state, before);

        state.apply_transaction(&valid, 2).unwrap();
        let after = state.clone();
        assert_eq!(
            state.apply_transaction(&valid, 2),
            Err(ValidationError::DuplicateTransaction)
        );
        assert_eq!(state, after);
    }

    #[test]
    fn distinct_same_nonce_and_cross_chain_transactions_are_rejected() {
        let chain_id = ChainId::new("testnet").unwrap();
        let alice_key = key(1);
        let bob = Address::from_public_key(&key(2).verifying_key().to_bytes());
        let mut state = LedgerState::new(chain_id.clone());
        state
            .apply_transaction(&claim(&state, &alice_key, 0), 1)
            .unwrap();

        let same_nonce = sign(
            &chain_id,
            &alice_key,
            0,
            TransactionAction::Transfer {
                recipient: bob,
                amount_atoms: 1,
            },
        );
        assert_eq!(
            state.apply_transaction(&same_nonce, 1),
            Err(ValidationError::NonceMismatch {
                expected: 1,
                actual: 0,
            })
        );

        let other_chain = sign(
            &ChainId::new("othernet").unwrap(),
            &alice_key,
            1,
            TransactionAction::Transfer {
                recipient: bob,
                amount_atoms: 1,
            },
        );
        assert_eq!(
            state.apply_transaction(&other_chain, 1),
            Err(ValidationError::WrongChain {
                expected: "testnet".into(),
                actual: "othernet".into(),
            })
        );
    }

    #[test]
    fn first_valid_claim_wins_and_advances_the_persistent_challenge() {
        let chain_id = ChainId::new("testnet").unwrap();
        let alice_key = key(1);
        let bob_key = key(2);
        let mut state = LedgerState::new(chain_id);
        let alice_claim = claim(&state, &alice_key, 0);
        let bob_claim = claim(&state, &bob_key, 0);
        state.apply_transaction(&alice_claim, 1).unwrap();
        assert_eq!(state.active_challenge_id(), 1);
        assert_eq!(
            state.apply_transaction(&bob_claim, 1),
            Err(ValidationError::StaleChallenge {
                expected: 1,
                actual: 0
            })
        );
    }

    #[test]
    fn reward_bands_and_final_cap_are_exact() {
        assert_eq!(reward_for_supply(0), Some(100 * ATOMS_PER_KCOIN));
        assert_eq!(
            reward_for_supply(20_000 * ATOMS_PER_KCOIN),
            Some(50 * ATOMS_PER_KCOIN)
        );
        assert_eq!(
            reward_for_supply(40_000 * ATOMS_PER_KCOIN),
            Some(25 * ATOMS_PER_KCOIN)
        );
        assert_eq!(
            reward_for_supply(60_000 * ATOMS_PER_KCOIN),
            Some(10 * ATOMS_PER_KCOIN)
        );
        assert_eq!(
            reward_for_supply(80_000 * ATOMS_PER_KCOIN),
            Some(5 * ATOMS_PER_KCOIN)
        );
        assert_eq!(reward_for_supply(MAX_SUPPLY_ATOMS - 1), Some(1));
        assert_eq!(reward_for_supply(MAX_SUPPLY_ATOMS), None);
    }

    #[test]
    fn block_application_is_atomic_and_replay_reconstructs_the_same_state() {
        let chain_id = ChainId::new("testnet").unwrap();
        let alice_key = key(1);
        let state = LedgerState::new(chain_id.clone());
        let block = state
            .build_block(
                ValidatorId::from_bytes([9; 32]),
                0,
                1_000,
                vec![claim(&state, &alice_key, 0)],
            )
            .unwrap();
        let mut applied = state.clone();
        applied.apply_block(&block).unwrap();
        let replayed = replay_blocks(chain_id, std::slice::from_ref(&block)).unwrap();
        assert_eq!(replayed, applied);
        assert_eq!(replayed.state_root(), block.header.state_root);

        let mut invalid = block.clone();
        invalid.header.state_root = Hash32::from_bytes([0xFF; 32]);
        let before = state.clone();
        let mut target = state;
        assert_eq!(
            target.apply_block(&invalid),
            Err(ValidationError::InvalidStateRoot)
        );
        assert_eq!(target, before);
    }

    #[test]
    fn timestamps_are_bounded_without_allowing_a_future_parent_to_halt_proposals() {
        let chain_id = ChainId::new("timestamp-test").unwrap();
        let proposer = ValidatorId::from_bytes([9; 32]);
        let mut state = LedgerState::new(chain_id.clone());
        let first = state.build_block(proposer, 0, 1_000, Vec::new()).unwrap();
        state.apply_block(&first).unwrap();

        let mut too_far = state.build_block(proposer, 0, 1_001, Vec::new()).unwrap();
        too_far.header.timestamp_ms =
            first.header.timestamp_ms + crate::MAX_BLOCK_TIMESTAMP_STEP_MS + 1;
        assert_eq!(
            state.apply_block(&too_far),
            Err(ValidationError::InvalidTimestamp)
        );

        // Even a maximally future-dated genesis block cannot make honest
        // construction fail forever: the next proposal reuses the parent
        // time, and the absolute ceiling prevents an unrenderable u64 value.
        let mut poisoned = LedgerState::new(chain_id);
        let future = poisoned
            .build_block(proposer, 0, u64::MAX, Vec::new())
            .unwrap();
        assert_eq!(future.header.timestamp_ms, crate::MAX_BLOCK_TIMESTAMP_MS);
        poisoned.apply_block(&future).unwrap();
        let recovered = poisoned
            .build_block(proposer, 0, 1_000, Vec::new())
            .unwrap();
        assert_eq!(recovered.header.timestamp_ms, crate::MAX_BLOCK_TIMESTAMP_MS);
        poisoned.apply_block(&recovered).unwrap();
        assert_eq!(poisoned.height(), 2);
    }

    #[test]
    fn challenges_are_deterministic_bounded_and_non_negative() {
        for id in 0..10_000 {
            let first = Challenge::for_id(id);
            let second = Challenge::for_id(id);
            assert_eq!(first, second);
            assert!((1..=9).contains(&first.left));
            assert!((1..=9).contains(&first.right));
            if first.operation == ChallengeOperation::Subtract {
                assert!(first.left >= first.right);
            }
            assert!(first.answer() <= 81);
        }
    }

    proptest! {
        #[test]
        fn arbitrary_valid_transfer_conserves_issued_supply(
            amount in 1_u64..=(100 * ATOMS_PER_KCOIN)
        ) {
            let chain_id = ChainId::new("property").unwrap();
            let alice_key = key(1);
            let bob = Address::from_public_key(&key(2).verifying_key().to_bytes());
            let alice = Address::from_public_key(&alice_key.verifying_key().to_bytes());
            let mut state = LedgerState::new(chain_id.clone());
            state.apply_transaction(&claim(&state, &alice_key, 0), 1).unwrap();
            let transfer = sign(
                &chain_id,
                &alice_key,
                1,
                TransactionAction::Transfer { recipient: bob, amount_atoms: amount },
            );
            state.apply_transaction(&transfer, 1).unwrap();
            let account_sum: u64 = state.accounts().values().map(|account| account.balance_atoms).sum();
            prop_assert_eq!(account_sum, state.total_supply_atoms());
            prop_assert_eq!(
                state.account(&alice).unwrap().balance_atoms + state.account(&bob).unwrap().balance_atoms,
                100 * ATOMS_PER_KCOIN,
            );
        }
    }
}
