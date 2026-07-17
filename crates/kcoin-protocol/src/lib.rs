//! The deterministic KCoin protocol and ledger.
//!
//! This crate is intentionally free of networking, persistence, async runtimes,
//! and wall-clock access. Every honest node can feed it the same canonical data
//! and obtain the same transaction IDs, block hash, and post-block state root.

mod block;
mod commit;
mod crypto;
mod error;
mod ledger;
mod transaction;

#[cfg(feature = "wasm")]
mod wasm;

pub use block::{Block, BlockHeader, merkle_root};
pub use commit::{CommitCertificate, CommitSignature, CommitVote, ValidatorId, quorum_size};
pub use crypto::{Address, ChainId, Hash32};
pub use error::ValidationError;
pub use ledger::{
    AccountState, Challenge, ChallengeOperation, LedgerState, TransactionOutcome,
    TransactionReceipt, ValidatedTransaction, replay_blocks, replay_finalized_blocks,
    reward_for_supply,
};
pub use transaction::{SignedTransaction, TransactionAction, UnsignedTransaction};

/// The only wire-compatible protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u16 = 1;
/// Number of decimal places displayed for KCoin balances.
pub const DECIMAL_PLACES: u32 = 6;
/// Atomic units in one KCoin.
pub const ATOMS_PER_KCOIN: u64 = 1_000_000;
/// Maximum supply, in atomic units (100,000 KCoin).
pub const MAX_SUPPLY_ATOMS: u64 = 100_000 * ATOMS_PER_KCOIN;
/// Maximum accepted encoded transaction size.
pub const MAX_TRANSACTION_BYTES: usize = 4 * 1024;
/// Maximum accepted canonical block size.
pub const MAX_BLOCK_BYTES: usize = 2 * 1024 * 1024;
/// Maximum accepted canonical commit-certificate size.
pub const MAX_COMMIT_CERTIFICATE_BYTES: usize = 64 * 1024;
/// Maximum deterministic timestamp advance after genesis (one minute).
pub const MAX_BLOCK_TIMESTAMP_STEP_MS: u64 = 60_000;
/// Absolute v1 timestamp ceiling (3000-01-01T00:00:00Z).
pub const MAX_BLOCK_TIMESTAMP_MS: u64 = 32_503_680_000_000;
/// Maximum UTF-8 bytes in a public display name.
pub const MAX_DISPLAY_NAME_BYTES: usize = 64;
/// Maximum Unicode scalar values in a public display name.
pub const MAX_DISPLAY_NAME_CHARS: usize = 32;

pub(crate) const TX_SIGNATURE_PREFIX: &[u8] = b"KCOIN_TX_V1\0";
pub(crate) const COMMIT_SIGNATURE_PREFIX: &[u8] = b"KCOIN_COMMIT_V1\0";
pub(crate) const ADDRESS_HASH_DOMAIN: &str = "kcoin.dev/v1/address";
pub(crate) const TRANSACTION_HASH_DOMAIN: &str = "kcoin.dev/v1/transaction-id";
pub(crate) const BLOCK_HASH_DOMAIN: &str = "kcoin.dev/v1/block-id";
pub(crate) const STATE_HASH_DOMAIN: &str = "kcoin.dev/v1/state-root";
pub(crate) const MERKLE_EMPTY_DOMAIN: &str = "kcoin.dev/v1/tx-merkle-empty";
pub(crate) const MERKLE_LEAF_DOMAIN: &str = "kcoin.dev/v1/tx-merkle-leaf";
pub(crate) const MERKLE_NODE_DOMAIN: &str = "kcoin.dev/v1/tx-merkle-node";
pub(crate) const CHALLENGE_HASH_DOMAIN: &str = "kcoin.dev/v1/challenge";
