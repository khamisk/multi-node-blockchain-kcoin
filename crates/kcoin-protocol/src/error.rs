use thiserror::Error;

/// Deterministic protocol validation failures.
///
/// `code()` is stable and suitable for the public HTTP API. Human-readable
/// messages may become more descriptive without changing client behavior.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ValidationError {
    #[error("unsupported protocol version {actual}; expected {expected}")]
    UnsupportedProtocolVersion { expected: u16, actual: u16 },
    #[error("invalid chain id")]
    InvalidChainId,
    #[error("transaction belongs to chain {actual}, not {expected}")]
    WrongChain { expected: String, actual: String },
    #[error("public key is not a valid Ed25519 key")]
    InvalidPublicKey,
    #[error("Ed25519 signature verification failed")]
    InvalidSignature,
    #[error("transaction has already been applied")]
    DuplicateTransaction,
    #[error("transaction expired at height {expiry_height}; execution height is {current_height}")]
    Expired {
        expiry_height: u64,
        current_height: u64,
    },
    #[error("nonce mismatch: expected {expected}, received {actual}")]
    NonceMismatch { expected: u64, actual: u64 },
    #[error("transfer amount must be positive")]
    ZeroAmount,
    #[error("insufficient balance: available {available}, required {required}")]
    InsufficientBalance { available: u64, required: u64 },
    #[error("challenge {actual} is stale; active challenge is {expected}")]
    StaleChallenge { expected: u64, actual: u64 },
    #[error("incorrect challenge answer")]
    WrongChallengeAnswer,
    #[error("the maximum KCoin supply has already been issued")]
    SupplyExhausted,
    #[error("display name is invalid")]
    InvalidDisplayName,
    #[error("integer arithmetic overflow")]
    ArithmeticOverflow,
    #[error("block height mismatch: expected {expected}, received {actual}")]
    InvalidBlockHeight { expected: u64, actual: u64 },
    #[error("block parent does not match the current tip")]
    InvalidParentHash,
    #[error("block timestamp precedes the previous finalized block")]
    InvalidTimestamp,
    #[error("transaction Merkle root is invalid")]
    InvalidTransactionsRoot,
    #[error("post-execution state root is invalid")]
    InvalidStateRoot,
    #[error("commit certificate does not describe the expected block")]
    InvalidCommitCertificate,
    #[error("validator appears more than once in a commit certificate")]
    DuplicateValidator,
    #[error("commit signature is from an unauthorized validator")]
    UnauthorizedValidator,
    #[error("commit certificate has {actual} signatures; quorum is {required}")]
    InsufficientQuorum { required: usize, actual: usize },
    #[error("address is malformed or not canonical Bech32m")]
    MalformedAddress,
    #[error("canonical payload is malformed")]
    Malformed,
    #[error("payload exceeds its protocol size limit")]
    PayloadTooLarge,
}

impl ValidationError {
    /// Stable, machine-readable code used at API boundaries.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedProtocolVersion { .. } => "UNSUPPORTED_VERSION",
            Self::InvalidChainId | Self::WrongChain { .. } => "WRONG_CHAIN",
            Self::InvalidPublicKey | Self::Malformed => "MALFORMED",
            Self::InvalidSignature => "INVALID_SIGNATURE",
            Self::DuplicateTransaction => "DUPLICATE_TRANSACTION",
            Self::Expired { .. } => "EXPIRED",
            Self::NonceMismatch { .. } => "NONCE_MISMATCH",
            Self::ZeroAmount => "ZERO_AMOUNT",
            Self::InsufficientBalance { .. } => "INSUFFICIENT_BALANCE",
            Self::StaleChallenge { .. } => "STALE_CHALLENGE",
            Self::WrongChallengeAnswer => "WRONG_CHALLENGE_ANSWER",
            Self::SupplyExhausted => "SUPPLY_EXHAUSTED",
            Self::InvalidDisplayName => "INVALID_DISPLAY_NAME",
            Self::ArithmeticOverflow => "ARITHMETIC_OVERFLOW",
            Self::InvalidBlockHeight { .. } => "INVALID_BLOCK_HEIGHT",
            Self::InvalidParentHash => "INVALID_PARENT_HASH",
            Self::InvalidTimestamp => "INVALID_TIMESTAMP",
            Self::InvalidTransactionsRoot => "INVALID_TRANSACTIONS_ROOT",
            Self::InvalidStateRoot => "INVALID_STATE_ROOT",
            Self::InvalidCommitCertificate
            | Self::DuplicateValidator
            | Self::UnauthorizedValidator
            | Self::InsufficientQuorum { .. } => "INVALID_COMMIT_CERTIFICATE",
            Self::MalformedAddress => "MALFORMED_ADDRESS",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
        }
    }
}
