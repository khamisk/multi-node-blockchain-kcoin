use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("storage lock was poisoned")]
    Poisoned,
    #[error("height {height} already contains a different finalized block")]
    ConflictingFinality { height: u64 },
    #[error("validator refused to sign conflicting bytes for {slot}")]
    ConflictingSignature { slot: String },
    #[error("validator refused to reuse {slot} with a conflicting safety state")]
    ConflictingSafetyState { slot: String },
    #[error("validator refused to reuse {slot} with conflicting signed-message bytes")]
    ConflictingSignedMessage { slot: String },
    #[error("height {height} round {round} already contains a different proposal")]
    ConflictingProposal { height: u64, round: u32 },
    #[error("consensus signer slot is malformed: {slot}")]
    MalformedSignerSlot { slot: String },
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransactionProjection {
    pub id: String,
    pub index: u32,
    pub kind: String,
    pub sender: String,
    pub recipient: Option<String>,
    pub amount_atoms: u64,
    pub nonce: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountProjection {
    pub address: String,
    pub balance_atoms: u64,
    pub nonce: u64,
    pub display_name: Option<String>,
    pub transaction_count: u64,
}

#[derive(Debug, Clone)]
pub struct FinalizedProjection {
    pub height: u64,
    pub block_hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub proposer: String,
    pub round: u32,
    pub timestamp_ms: u64,
    pub block_bytes: Vec<u8>,
    pub certificate_bytes: Vec<u8>,
    pub transactions: Vec<TransactionProjection>,
    pub changed_accounts: Vec<AccountProjection>,
    pub issued_supply_atoms: u64,
    pub challenge_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockRow {
    pub height: u64,
    pub block_hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub proposer: String,
    pub round: u32,
    pub timestamp_ms: u64,
    pub transaction_count: u64,
    #[serde(skip_serializing)]
    pub block_bytes: Vec<u8>,
    #[serde(skip_serializing)]
    pub certificate_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRow {
    pub id: String,
    pub block_height: u64,
    pub index: u32,
    pub kind: String,
    pub sender: String,
    pub recipient: Option<String>,
    pub amount_atoms: u64,
    pub nonce: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedConsensusDecision {
    pub slot: String,
    pub sign_bytes: Vec<u8>,
    pub signature: Vec<u8>,
    pub safety_state: Vec<u8>,
    pub signed_message: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedConsensusProposal {
    pub height: u64,
    pub round: u32,
    pub block_id: Vec<u8>,
    pub block_bytes: Vec<u8>,
    pub signed_proposal: Vec<u8>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|_| StorageError::Poisoned)?;
        }
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        let has_safety_state = {
            let mut statement = connection.prepare("PRAGMA table_info(signer_state)")?;
            let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == "safety_state")
        };
        if !has_safety_state {
            connection.execute(
                "ALTER TABLE signer_state ADD COLUMN safety_state BLOB NOT NULL DEFAULT X''",
                [],
            )?;
        }
        let has_signed_message = {
            let mut statement = connection.prepare("PRAGMA table_info(signer_state)")?;
            let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == "signed_message")
        };
        if !has_signed_message {
            connection.execute(
                "ALTER TABLE signer_state ADD COLUMN signed_message BLOB NOT NULL DEFAULT X''",
                [],
            )?;
        }
        let has_safety_height = {
            let mut statement = connection.prepare("PRAGMA table_info(consensus_safety)")?;
            let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == "height")
        };
        if !has_safety_height {
            connection.execute(
                "ALTER TABLE consensus_safety ADD COLUMN height INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }

    pub fn persist_finalized(&self, block: &FinalizedProjection) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;

        let existing: Option<String> = transaction
            .query_row(
                "SELECT block_hash FROM blocks WHERE height = ?1",
                params![block.height],
                |row| row.get(0),
            )
            .optional()?;
        let block_exists = match existing {
            Some(existing_hash) if existing_hash == block.block_hash => true,
            Some(_) => {
                return Err(StorageError::ConflictingFinality {
                    height: block.height,
                });
            }
            None => false,
        };

        if !block_exists {
            transaction.execute(
                "INSERT INTO blocks (
                height, block_hash, parent_hash, state_root, proposer, round,
                timestamp_ms, transaction_count, block_bytes, certificate_bytes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    block.height,
                    block.block_hash,
                    block.parent_hash,
                    block.state_root,
                    block.proposer,
                    block.round,
                    block.timestamp_ms,
                    block.transactions.len() as u64,
                    block.block_bytes,
                    block.certificate_bytes,
                ],
            )?;
        }

        for tx in &block.transactions {
            transaction.execute(
                "INSERT INTO transactions (
                    id, block_height, tx_index, kind, sender, recipient, amount_atoms, nonce
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    block_height = excluded.block_height,
                    tx_index = excluded.tx_index,
                    kind = excluded.kind,
                    sender = excluded.sender,
                    recipient = excluded.recipient,
                    amount_atoms = excluded.amount_atoms,
                    nonce = excluded.nonce",
                params![
                    tx.id,
                    block.height,
                    tx.index,
                    tx.kind,
                    tx.sender,
                    tx.recipient,
                    tx.amount_atoms,
                    tx.nonce,
                ],
            )?;
        }

        for account in &block.changed_accounts {
            transaction.execute(
                "INSERT INTO accounts (
                    address, balance_atoms, nonce, display_name, transaction_count, last_height
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(address) DO UPDATE SET
                    balance_atoms = excluded.balance_atoms,
                    nonce = excluded.nonce,
                    display_name = excluded.display_name,
                    transaction_count = excluded.transaction_count,
                    last_height = excluded.last_height",
                params![
                    account.address,
                    account.balance_atoms,
                    account.nonce,
                    account.display_name,
                    account.transaction_count,
                    block.height,
                ],
            )?;
        }

        for (key, value) in [
            ("tip_height", block.height.to_string()),
            ("tip_hash", block.block_hash.clone()),
            ("state_root", block.state_root.clone()),
            ("issued_supply_atoms", block.issued_supply_atoms.to_string()),
            ("challenge", block.challenge_json.clone()),
        ] {
            transaction.execute(
                "INSERT INTO metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }

        // A durable finalized height supersedes its in-progress lock state.
        // Keeping this deletion in the block transaction prevents a restart
        // from observing finality and stale safety state independently.
        transaction.execute(
            "DELETE FROM consensus_safety WHERE height = ?1",
            params![block.height],
        )?;

        transaction.commit()?;
        Ok(())
    }

    /// Refresh columns derived from canonical block bytes while preserving the
    /// canonical block and certificate blobs themselves.
    ///
    /// The byte equality predicates make this safe for `reindex`: a row whose
    /// source-of-truth history changed or disappeared is never rewritten as a
    /// projection repair.
    pub fn refresh_block_projection(&self, block: &FinalizedProjection) -> Result<()> {
        let changed = self.connection()?.execute(
            "UPDATE blocks SET
                block_hash = ?2,
                parent_hash = ?3,
                state_root = ?4,
                proposer = ?5,
                round = ?6,
                timestamp_ms = ?7,
                transaction_count = ?8
             WHERE height = ?1 AND block_bytes = ?9 AND certificate_bytes = ?10",
            params![
                block.height,
                block.block_hash,
                block.parent_hash,
                block.state_root,
                block.proposer,
                block.round,
                block.timestamp_ms,
                block.transactions.len() as u64,
                block.block_bytes,
                block.certificate_bytes,
            ],
        )?;
        if changed != 1 {
            return Err(StorageError::ConflictingFinality {
                height: block.height,
            });
        }
        Ok(())
    }

    pub fn tip(&self) -> Result<Option<BlockRow>> {
        self.connection()?
            .query_row(
                "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                        timestamp_ms, transaction_count, block_bytes, certificate_bytes
                 FROM blocks ORDER BY height DESC LIMIT 1",
                [],
                block_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn block_by_height(&self, height: u64) -> Result<Option<BlockRow>> {
        self.connection()?
            .query_row(
                "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                        timestamp_ms, transaction_count, block_bytes, certificate_bytes
                 FROM blocks WHERE height = ?1",
                params![height],
                block_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn block_by_hash(&self, hash: &str) -> Result<Option<BlockRow>> {
        self.connection()?
            .query_row(
                "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                        timestamp_ms, transaction_count, block_bytes, certificate_bytes
                 FROM blocks WHERE block_hash = ?1",
                params![hash],
                block_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_blocks(&self, before: Option<u64>, limit: u32) -> Result<Vec<BlockRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                    timestamp_ms, transaction_count, block_bytes, certificate_bytes
             FROM blocks
             WHERE (?1 IS NULL OR height < ?1)
             ORDER BY height DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![before, limit.clamp(1, 101)], block_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn transaction(&self, id: &str) -> Result<Option<TransactionRow>> {
        self.connection()?
            .query_row(
                "SELECT id, block_height, tx_index, kind, sender, recipient, amount_atoms, nonce
                 FROM transactions WHERE id = ?1",
                params![id],
                transaction_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_transactions(
        &self,
        before: Option<(u64, u32)>,
        limit: u32,
    ) -> Result<Vec<TransactionRow>> {
        let connection = self.connection()?;
        let (before_height, before_index) =
            before.map_or((None, None), |(height, index)| (Some(height), Some(index)));
        let mut statement = connection.prepare(
            "SELECT id, block_height, tx_index, kind, sender, recipient, amount_atoms, nonce
             FROM transactions
             WHERE (?1 IS NULL OR block_height < ?1
                    OR (block_height = ?1 AND tx_index < ?2))
             ORDER BY block_height DESC, tx_index DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![before_height, before_index, limit.clamp(1, 101)],
            transaction_from_row,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn address_transactions(&self, address: &str, limit: u32) -> Result<Vec<TransactionRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, block_height, tx_index, kind, sender, recipient, amount_atoms, nonce
             FROM transactions
             WHERE sender = ?1 OR recipient = ?1
             ORDER BY block_height DESC, tx_index DESC LIMIT ?2",
        )?;
        let rows =
            statement.query_map(params![address, limit.clamp(1, 100)], transaction_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn account_first_seen_height(&self, address: &str) -> Result<Option<u64>> {
        self.connection()?
            .query_row(
                "SELECT MIN(block_height) FROM transactions
                 WHERE sender = ?1 OR recipient = ?1",
                params![address],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn account(&self, address: &str) -> Result<Option<AccountProjection>> {
        self.connection()?
            .query_row(
                "SELECT address, balance_atoms, nonce, display_name, transaction_count
                 FROM accounts WHERE address = ?1",
                params![address],
                |row| {
                    Ok(AccountProjection {
                        address: row.get(0)?,
                        balance_atoms: row.get(1)?,
                        nonce: row.get(2)?,
                        display_name: row.get(3)?,
                        transaction_count: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn leaderboard(&self, limit: u32) -> Result<Vec<AccountProjection>> {
        self.leaderboard_page(limit, 0)
    }

    pub fn leaderboard_page(&self, limit: u32, offset: u32) -> Result<Vec<AccountProjection>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT address, balance_atoms, nonce, display_name, transaction_count
             FROM accounts WHERE balance_atoms > 0
             ORDER BY balance_atoms DESC, address ASC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit.clamp(1, 101), offset], |row| {
            Ok(AccountProjection {
                address: row.get(0)?,
                balance_atoms: row.get(1)?,
                nonce: row.get(2)?,
                display_name: row.get(3)?,
                transaction_count: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn metadata(&self, key: &str) -> Result<Option<String>> {
        self.connection()?
            .query_row(
                "SELECT value FROM metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn canonical_blocks(&self) -> Result<Vec<Vec<u8>>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT block_bytes FROM blocks ORDER BY height")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn canonical_block_rows(&self) -> Result<Vec<BlockRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                    timestamp_ms, transaction_count, block_bytes, certificate_bytes
             FROM blocks ORDER BY height ASC",
        )?;
        let rows = statement.query_map([], block_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn finalized_range(&self, from_height: u64, limit: u16) -> Result<Vec<BlockRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT height, block_hash, parent_hash, state_root, proposer, round,
                    timestamp_ms, transaction_count, block_bytes, certificate_bytes
             FROM blocks WHERE height >= ?1 ORDER BY height ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![from_height, u64::from(limit.min(128))],
            block_from_row,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn append_consensus_event(&self, kind: &str, bytes: &[u8]) -> Result<u64> {
        let checksum = blake3::hash(bytes).to_hex().to_string();
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO consensus_wal (kind, bytes, checksum) VALUES (?1, ?2, ?3)",
            params![kind, bytes, checksum],
        )?;
        Ok(connection.last_insert_rowid() as u64)
    }

    pub fn persist_signed_message(
        &self,
        slot: &str,
        sign_bytes: &[u8],
        signature: &[u8],
    ) -> Result<()> {
        self.persist_consensus_signature(slot, sign_bytes, signature, &[])
            .map(|_| ())
    }

    /// Atomically records the exact Ed25519 input, resulting signature, and
    /// consensus safety state (including lock and valid-round proof) before
    /// the caller is allowed to broadcast.
    /// Repeating the same decision is idempotent and returns the durable
    /// signature; changing either the signed bytes or safety state fails closed.
    pub fn persist_consensus_signature(
        &self,
        slot: &str,
        sign_bytes: &[u8],
        signature: &[u8],
        safety_state: &[u8],
    ) -> Result<Vec<u8>> {
        self.persist_consensus_decision(slot, sign_bytes, signature, safety_state, &[])
    }

    pub fn persist_consensus_decision(
        &self,
        slot: &str,
        sign_bytes: &[u8],
        signature: &[u8],
        safety_state: &[u8],
        signed_message: &[u8],
    ) -> Result<Vec<u8>> {
        let safety_height = slot
            .split('/')
            .next()
            .and_then(|height| height.parse::<u64>().ok())
            .ok_or_else(|| StorageError::MalformedSignerSlot {
                slot: slot.to_owned(),
            })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let existing: Option<PersistedConsensusDecision> = transaction
            .query_row(
                "SELECT sign_bytes, signature, safety_state, signed_message
                 FROM signer_state WHERE slot = ?1",
                params![slot],
                |row| {
                    Ok(PersistedConsensusDecision {
                        slot: slot.to_owned(),
                        sign_bytes: row.get(0)?,
                        signature: row.get(1)?,
                        safety_state: row.get(2)?,
                        signed_message: row.get(3)?,
                    })
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.sign_bytes != sign_bytes {
                return Err(StorageError::ConflictingSignature {
                    slot: slot.to_owned(),
                });
            }
            if existing.safety_state != safety_state {
                return Err(StorageError::ConflictingSafetyState {
                    slot: slot.to_owned(),
                });
            }
            if !existing.signed_message.is_empty()
                && !signed_message.is_empty()
                && existing.signed_message != signed_message
            {
                return Err(StorageError::ConflictingSignedMessage {
                    slot: slot.to_owned(),
                });
            }
            if existing.signed_message.is_empty() && !signed_message.is_empty() {
                transaction.execute(
                    "UPDATE signer_state SET signed_message = ?2 WHERE slot = ?1",
                    params![slot, signed_message],
                )?;
                transaction.commit()?;
            }
            return Ok(existing.signature);
        }
        transaction.execute(
            "INSERT INTO signer_state (
                slot, sign_bytes, signature, safety_state, signed_message
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![slot, sign_bytes, signature, safety_state, signed_message],
        )?;
        transaction.execute(
            "INSERT INTO consensus_safety (singleton, height, safety_state)
             VALUES (1, ?1, ?2)
             ON CONFLICT(singleton) DO UPDATE SET
                height = excluded.height,
                safety_state = excluded.safety_state",
            params![safety_height, safety_state],
        )?;
        transaction.commit()?;
        Ok(signature.to_vec())
    }

    pub fn consensus_safety_state(&self) -> Result<Option<(u64, Vec<u8>)>> {
        self.connection()?
            .query_row(
                "SELECT height, safety_state FROM consensus_safety WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn consensus_decisions(&self, height: u64) -> Result<Vec<PersistedConsensusDecision>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT slot, sign_bytes, signature, safety_state, signed_message FROM signer_state
             WHERE slot LIKE ?1 AND length(signed_message) > 0 ORDER BY rowid ASC",
        )?;
        let prefix = format!("{height}/%");
        let rows = statement.query_map(params![prefix], |row| {
            Ok(PersistedConsensusDecision {
                slot: row.get(0)?,
                sign_bytes: row.get(1)?,
                signature: row.get(2)?,
                safety_state: row.get(3)?,
                signed_message: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn persist_consensus_proposal(&self, proposal: &PersistedConsensusProposal) -> Result<()> {
        let connection = self.connection()?;
        let existing: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = connection
            .query_row(
                "SELECT block_id, block_bytes, signed_proposal
                 FROM consensus_proposals WHERE height = ?1 AND round = ?2",
                params![proposal.height, proposal.round],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((block_id, block_bytes, signed_proposal)) = existing {
            if block_id == proposal.block_id
                && block_bytes == proposal.block_bytes
                && signed_proposal == proposal.signed_proposal
            {
                return Ok(());
            }
            return Err(StorageError::ConflictingProposal {
                height: proposal.height,
                round: proposal.round,
            });
        }
        connection.execute(
            "INSERT INTO consensus_proposals (
                height, round, block_id, block_bytes, signed_proposal
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                proposal.height,
                proposal.round,
                proposal.block_id,
                proposal.block_bytes,
                proposal.signed_proposal,
            ],
        )?;
        Ok(())
    }

    pub fn consensus_proposals(&self, height: u64) -> Result<Vec<PersistedConsensusProposal>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT height, round, block_id, block_bytes, signed_proposal
             FROM consensus_proposals WHERE height = ?1 ORDER BY round ASC",
        )?;
        let rows = statement.query_map(params![height], |row| {
            Ok(PersistedConsensusProposal {
                height: row.get(0)?,
                round: row.get(1)?,
                block_id: row.get(2)?,
                block_bytes: row.get(3)?,
                signed_proposal: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn clear_projections(&self) -> Result<()> {
        let connection = self.connection()?;
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             DELETE FROM transactions;
             DELETE FROM accounts;
             DELETE FROM metadata;
             COMMIT;",
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn corrupt_block_projection_for_test(&self, height: u64) -> Result<()> {
        self.connection()?.execute(
            "UPDATE blocks SET
                block_hash = 'corrupt-hash',
                parent_hash = 'corrupt-parent',
                state_root = 'corrupt-root',
                proposer = 'corrupt-proposer',
                round = 999,
                timestamp_ms = 999,
                transaction_count = 999
             WHERE height = ?1",
            params![height],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn corrupt_metadata_for_test(&self, key: &str) -> Result<()> {
        self.connection()?.execute(
            "UPDATE metadata SET value = '{malformed' WHERE key = ?1",
            params![key],
        )?;
        Ok(())
    }
}

fn block_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BlockRow> {
    Ok(BlockRow {
        height: row.get(0)?,
        block_hash: row.get(1)?,
        parent_hash: row.get(2)?,
        state_root: row.get(3)?,
        proposer: row.get(4)?,
        round: row.get(5)?,
        timestamp_ms: row.get(6)?,
        transaction_count: row.get(7)?,
        block_bytes: row.get(8)?,
        certificate_bytes: row.get(9)?,
    })
}

fn transaction_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TransactionRow> {
    Ok(TransactionRow {
        id: row.get(0)?,
        block_height: row.get(1)?,
        index: row.get(2)?,
        kind: row.get(3)?,
        sender: row.get(4)?,
        recipient: row.get(5)?,
        amount_atoms: row.get(6)?,
        nonce: row.get(7)?,
    })
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS blocks (
    height INTEGER PRIMARY KEY,
    block_hash TEXT NOT NULL UNIQUE,
    parent_hash TEXT NOT NULL,
    state_root TEXT NOT NULL,
    proposer TEXT NOT NULL,
    round INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    transaction_count INTEGER NOT NULL,
    block_bytes BLOB NOT NULL,
    certificate_bytes BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS transactions (
    id TEXT PRIMARY KEY,
    block_height INTEGER NOT NULL REFERENCES blocks(height) ON DELETE CASCADE,
    tx_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipient TEXT,
    amount_atoms INTEGER NOT NULL,
    nonce INTEGER NOT NULL,
    UNIQUE(block_height, tx_index)
);
CREATE INDEX IF NOT EXISTS transactions_sender_idx ON transactions(sender, block_height DESC);
CREATE INDEX IF NOT EXISTS transactions_recipient_idx ON transactions(recipient, block_height DESC);
CREATE TABLE IF NOT EXISTS accounts (
    address TEXT PRIMARY KEY,
    balance_atoms INTEGER NOT NULL,
    nonce INTEGER NOT NULL,
    display_name TEXT,
    transaction_count INTEGER NOT NULL,
    last_height INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS accounts_balance_idx ON accounts(balance_atoms DESC, address);
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS consensus_wal (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    bytes BLOB NOT NULL,
    checksum TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS signer_state (
    slot TEXT PRIMARY KEY,
    sign_bytes BLOB NOT NULL,
    signature BLOB NOT NULL,
    safety_state BLOB NOT NULL,
    signed_message BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS consensus_safety (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    height INTEGER NOT NULL,
    safety_state BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS consensus_proposals (
    height INTEGER NOT NULL,
    round INTEGER NOT NULL,
    block_id BLOB NOT NULL,
    block_bytes BLOB NOT NULL,
    signed_proposal BLOB NOT NULL,
    PRIMARY KEY(height, round)
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn projected_block(hash: &str) -> FinalizedProjection {
        FinalizedProjection {
            height: 1,
            block_hash: hash.into(),
            parent_hash: "00".repeat(32),
            state_root: "11".repeat(32),
            proposer: "validator-1".into(),
            round: 0,
            timestamp_ms: 1,
            block_bytes: vec![1, 2, 3],
            certificate_bytes: vec![4, 5, 6],
            transactions: vec![TransactionProjection {
                id: "tx-1".into(),
                index: 0,
                kind: "claimReward".into(),
                sender: "kcoin1sender".into(),
                recipient: None,
                amount_atoms: 100_000_000,
                nonce: 1,
            }],
            changed_accounts: vec![AccountProjection {
                address: "kcoin1sender".into(),
                balance_atoms: 100_000_000,
                nonce: 1,
                display_name: Some("Ada".into()),
                transaction_count: 1,
            }],
            issued_supply_atoms: 100_000_000,
            challenge_json: "{}".into(),
        }
    }

    #[test]
    fn finalized_block_and_projections_commit_together() {
        let store = Store::in_memory().unwrap();
        store.persist_finalized(&projected_block("abc")).unwrap();

        assert_eq!(store.tip().unwrap().unwrap().block_hash, "abc");
        assert_eq!(
            store
                .account("kcoin1sender")
                .unwrap()
                .unwrap()
                .balance_atoms,
            100_000_000
        );
        assert_eq!(store.transaction("tx-1").unwrap().unwrap().block_height, 1);
    }

    #[test]
    fn conflicting_finality_is_rejected() {
        let store = Store::in_memory().unwrap();
        store.persist_finalized(&projected_block("abc")).unwrap();
        let error = store
            .persist_finalized(&projected_block("def"))
            .unwrap_err();
        assert!(matches!(
            error,
            StorageError::ConflictingFinality { height: 1 }
        ));
    }

    #[test]
    fn signer_refuses_conflicting_bytes() {
        let store = Store::in_memory().unwrap();
        store
            .persist_signed_message("1/0/prevote", b"one", b"sig")
            .unwrap();
        store
            .persist_signed_message("1/0/prevote", b"one", b"sig")
            .unwrap();
        assert!(matches!(
            store
                .persist_signed_message("1/0/prevote", b"two", b"sig")
                .unwrap_err(),
            StorageError::ConflictingSignature { .. }
        ));
    }

    #[test]
    fn consensus_signature_and_safety_state_are_idempotent_and_fail_closed() {
        let store = Store::in_memory().unwrap();
        let first = store
            .persist_consensus_signature(
                "9/2/precommit",
                b"exact-signing-input",
                b"durable-signature",
                b"locked-block-a",
            )
            .unwrap();
        assert_eq!(first, b"durable-signature");
        let replay = store
            .persist_consensus_signature(
                "9/2/precommit",
                b"exact-signing-input",
                b"newly-generated-signature-is-ignored",
                b"locked-block-a",
            )
            .unwrap();
        assert_eq!(replay, b"durable-signature");
        assert!(matches!(
            store
                .persist_consensus_signature(
                    "9/2/precommit",
                    b"exact-signing-input",
                    b"durable-signature",
                    b"locked-block-b",
                )
                .unwrap_err(),
            StorageError::ConflictingSafetyState { .. }
        ));

        let store = Store::in_memory().unwrap();
        store
            .persist_consensus_decision(
                "1/0/prevote",
                b"vote-input",
                b"vote-signature",
                b"height-one-safety",
                b"signed-vote",
            )
            .unwrap();
        assert_eq!(
            store.consensus_safety_state().unwrap().unwrap(),
            (1, b"height-one-safety".to_vec())
        );
        let decisions = store.consensus_decisions(1).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].signed_message, b"signed-vote");
        assert_eq!(decisions[0].sign_bytes, b"vote-input");
        store.persist_finalized(&projected_block("abc")).unwrap();
        assert_eq!(store.consensus_safety_state().unwrap(), None);

        store
            .persist_consensus_decision(
                "2/0/prevote",
                b"next-height-input",
                b"next-height-signature",
                b"height-two-safety",
                b"next-height-signed-vote",
            )
            .unwrap();
        store.persist_finalized(&projected_block("abc")).unwrap();
        assert_eq!(
            store.consensus_safety_state().unwrap().unwrap(),
            (2, b"height-two-safety".to_vec())
        );

        let proposal = PersistedConsensusProposal {
            height: 2,
            round: 3,
            block_id: vec![7; 32],
            block_bytes: vec![8; 64],
            signed_proposal: vec![9; 96],
        };
        store.persist_consensus_proposal(&proposal).unwrap();
        store.persist_consensus_proposal(&proposal).unwrap();
        assert_eq!(
            store.consensus_proposals(2).unwrap(),
            vec![proposal.clone()]
        );
        let mut conflicting = proposal;
        conflicting.block_id[0] ^= 1;
        assert!(matches!(
            store.persist_consensus_proposal(&conflicting).unwrap_err(),
            StorageError::ConflictingProposal {
                height: 2,
                round: 3
            }
        ));
    }
}
