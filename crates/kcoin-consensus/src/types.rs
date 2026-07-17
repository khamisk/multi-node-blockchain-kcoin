use std::{collections::BTreeSet, fmt, time::Duration};

use borsh::{BorshDeserialize, BorshSerialize};
use kcoin_protocol::{
    ChainId as ProtocolChainId, CommitCertificate as ProtocolCommitCertificate,
    CommitSignature as ProtocolCommitSignature, CommitVote as ProtocolCommitVote,
    Hash32 as ProtocolHash32, ValidatorId as ProtocolValidatorId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Height = u64;
pub type Round = u32;

/// The hash of a proposed application block.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    BorshDeserialize,
    BorshSerialize,
    Deserialize,
    Serialize,
)]
pub struct BlockId(pub [u8; 32]);

impl BlockId {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<ProtocolHash32> for BlockId {
    fn from(hash: ProtocolHash32) -> Self {
        Self(*hash.as_bytes())
    }
}

impl From<BlockId> for ProtocolHash32 {
    fn from(block_id: BlockId) -> Self {
        Self::from_bytes(block_id.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..6] {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("...")
    }
}

/// A validator's stable consensus identity (normally its Ed25519 public key).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    BorshDeserialize,
    BorshSerialize,
    Deserialize,
    Serialize,
)]
pub struct ValidatorId(pub [u8; 32]);

impl ValidatorId {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<ProtocolValidatorId> for ValidatorId {
    fn from(validator: ProtocolValidatorId) -> Self {
        Self(*validator.as_bytes())
    }
}

impl From<ValidatorId> for ProtocolValidatorId {
    fn from(validator: ValidatorId) -> Self {
        Self::from_bytes(validator.0)
    }
}

impl fmt::Display for ValidatorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ValidatorSetError {
    #[error("the validator set must not be empty")]
    Empty,
    #[error("validator set contains a duplicate identity: {0}")]
    Duplicate(ValidatorId),
}

/// A fixed, equal-voting-power validator set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatorSet {
    validators: Vec<ValidatorId>,
}

impl ValidatorSet {
    pub fn new(validators: Vec<ValidatorId>) -> Result<Self, ValidatorSetError> {
        if validators.is_empty() {
            return Err(ValidatorSetError::Empty);
        }
        let mut seen = BTreeSet::new();
        for validator in &validators {
            if !seen.insert(*validator) {
                return Err(ValidatorSetError::Duplicate(*validator));
            }
        }
        Ok(Self { validators })
    }

    #[must_use]
    pub fn validators(&self) -> &[ValidatorId] {
        &self.validators
    }

    #[must_use]
    pub fn contains(&self, validator: ValidatorId) -> bool {
        self.validators.contains(&validator)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.validators.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// `floor(2N/3) + 1`; three for KCoin's normal four-validator network.
    #[must_use]
    pub fn quorum(&self) -> usize {
        kcoin_protocol::quorum_size(self.validators.len())
    }

    /// Height is one-based, so validator zero proposes height one, round zero.
    #[must_use]
    pub fn proposer(&self, height: Height, round: Round) -> ValidatorId {
        let height_offset = height.saturating_sub(1) as usize;
        let index = (height_offset + round as usize) % self.validators.len();
        self.validators[index]
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    BorshDeserialize,
    BorshSerialize,
    Deserialize,
    Serialize,
)]
pub enum VoteKind {
    Prevote,
    Precommit,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    BorshDeserialize,
    BorshSerialize,
    Deserialize,
    Serialize,
)]
pub enum VoteValue {
    Nil,
    Block(BlockId),
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct Vote {
    pub chain_id: String,
    pub height: Height,
    pub round: Round,
    pub kind: VoteKind,
    pub validator: ValidatorId,
    pub value: VoteValue,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct SignedVote {
    pub vote: Vote,
    #[serde(with = "signature_serde")]
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct Proposal {
    pub chain_id: String,
    pub height: Height,
    pub round: Round,
    pub proposer: ValidatorId,
    pub block_id: BlockId,
    /// The earlier round in which this block obtained a prevote quorum.
    pub valid_round: Option<Round>,
    /// Distinct signed prevotes proving `valid_round` (a PoLC/polka).
    pub valid_round_proof: Vec<SignedVote>,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct SignedProposal {
    pub proposal: Proposal,
    #[serde(with = "signature_serde")]
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub enum SignableMessage {
    Proposal(Proposal),
    Vote(Vote),
}

impl SignableMessage {
    /// Canonical bytes for an external signer.
    ///
    /// A block precommit uses the protocol crate's commit domain so the same
    /// Ed25519 signature can be placed directly into the persisted protocol
    /// certificate. Proposals, prevotes, and nil precommits use this enum's
    /// Borsh variant as their domain separation.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, SigningBytesError> {
        if let Self::Vote(Vote {
            chain_id,
            height,
            round,
            kind: VoteKind::Precommit,
            value: VoteValue::Block(block_id),
            ..
        }) = self
        {
            let chain_id = ProtocolChainId::new(chain_id.clone())
                .map_err(|_| SigningBytesError::InvalidChainId)?;
            return Ok(
                ProtocolCommitVote::new(chain_id, *height, *round, (*block_id).into())
                    .signing_bytes(),
            );
        }
        Ok(borsh::to_vec(self)?)
    }

    #[must_use]
    pub fn round(&self) -> Round {
        match self {
            Self::Proposal(proposal) => proposal.round,
            Self::Vote(vote) => vote.round,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub enum SignedMessage {
    Proposal(SignedProposal),
    Vote(SignedVote),
}

impl SignedMessage {
    #[must_use]
    pub fn signable(&self) -> SignableMessage {
        match self {
            Self::Proposal(proposal) => SignableMessage::Proposal(proposal.proposal.clone()),
            Self::Vote(vote) => SignableMessage::Vote(vote.vote.clone()),
        }
    }

    #[must_use]
    pub fn signature(&self) -> &[u8] {
        match self {
            Self::Proposal(proposal) => &proposal.signature,
            Self::Vote(vote) => &vote.signature,
        }
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, SigningBytesError> {
        self.signable().signing_bytes()
    }
}

#[derive(Debug, Error)]
pub enum SigningBytesError {
    #[error("chain ID is not valid for the KCoin protocol")]
    InvalidChainId,
    #[error("canonical serialization failed")]
    Serialization(#[from] std::io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct CommitCertificate {
    pub chain_id: String,
    pub height: Height,
    pub round: Round,
    pub block_id: BlockId,
    pub precommits: Vec<SignedVote>,
}

impl CommitCertificate {
    /// Convert consensus precommits into the canonical persisted protocol
    /// certificate without re-signing. This succeeds only for 64-byte Ed25519
    /// signatures; quorum and signature validity remain an explicit check on
    /// the returned protocol certificate.
    pub fn to_protocol(&self) -> Result<ProtocolCommitCertificate, ProtocolCertificateError> {
        let chain_id = ProtocolChainId::new(self.chain_id.clone())
            .map_err(|_| ProtocolCertificateError::InvalidChainId)?;
        let vote = ProtocolCommitVote::new(chain_id, self.height, self.round, self.block_id.into());
        let signatures = self
            .precommits
            .iter()
            .map(|precommit| {
                if precommit.vote.kind != VoteKind::Precommit
                    || precommit.vote.value != VoteValue::Block(self.block_id)
                    || precommit.vote.chain_id != self.chain_id
                    || precommit.vote.height != self.height
                    || precommit.vote.round != self.round
                {
                    return Err(ProtocolCertificateError::MismatchedPrecommit);
                }
                let signature: [u8; 64] =
                    precommit.signature.as_slice().try_into().map_err(|_| {
                        ProtocolCertificateError::InvalidSignatureLength {
                            actual: precommit.signature.len(),
                        }
                    })?;
                Ok(ProtocolCommitSignature {
                    validator: precommit.vote.validator.into(),
                    signature,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProtocolCommitCertificate::new(vote, signatures))
    }

    /// Adapt a wire or persistence certificate for late-commit handling.
    #[must_use]
    pub fn from_protocol(certificate: &ProtocolCommitCertificate) -> Self {
        let block_id = BlockId::from(certificate.block_hash);
        Self {
            chain_id: certificate.chain_id.as_str().to_owned(),
            height: certificate.height,
            round: certificate.round,
            block_id,
            precommits: certificate
                .signatures
                .iter()
                .map(|signature| SignedVote {
                    vote: Vote {
                        chain_id: certificate.chain_id.as_str().to_owned(),
                        height: certificate.height,
                        round: certificate.round,
                        kind: VoteKind::Precommit,
                        validator: signature.validator.into(),
                        value: VoteValue::Block(block_id),
                    },
                    signature: signature.signature.to_vec(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ProtocolCertificateError {
    #[error("chain ID is not valid for the KCoin protocol")]
    InvalidChainId,
    #[error("precommit does not match the certificate statement")]
    MismatchedPrecommit,
    #[error("expected a 64-byte Ed25519 signature, got {actual} bytes")]
    InvalidSignatureLength { actual: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Phase {
    Propose,
    Prevote,
    Precommit,
    Finalized,
}

#[derive(Clone, Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Deserialize, Serialize)]
pub struct SafetyState {
    pub height: Height,
    /// Round containing the most recent durable local signing decision.
    pub round: Round,
    pub locked_round: Option<Round>,
    pub locked_block: Option<BlockId>,
    /// Most recent round in which this validator authenticated a prevote quorum.
    ///
    /// These fields have serde defaults so an old unlocked record still
    /// decodes during an upgrade. A legacy locked record has no proof and is
    /// deliberately rejected by recovery rather than resumed unsafely.
    #[serde(default)]
    pub valid_round: Option<Round>,
    #[serde(default)]
    pub valid_block: Option<BlockId>,
    #[serde(default)]
    pub valid_round_proof: Vec<SignedVote>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeout {
    pub height: Height,
    pub round: Round,
    pub phase: Phase,
    pub after: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// Begin round zero. Must be delivered exactly once.
    Start,
    /// The application built the block requested by `RequestProposal`.
    ProposalReady {
        height: Height,
        round: Round,
        block_id: BlockId,
    },
    /// A signed proposal or vote received from the network.
    Message(SignedMessage),
    /// A local message after it and the accompanying safety state were durably stored.
    Persisted(SignedMessage),
    /// A scheduled timeout firing. Stale timeouts are harmlessly ignored.
    Timeout(Timeout),
    /// A complete certificate, including one learned long after its round.
    Certificate(CommitCertificate),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finalization {
    pub height: Height,
    pub round: Round,
    pub block_id: BlockId,
    pub certificate: CommitCertificate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Evidence {
    DoubleVote {
        validator: ValidatorId,
        first: SignedVote,
        second: SignedVote,
    },
    DoubleProposal {
        proposer: ValidatorId,
        first: SignedProposal,
        second: SignedProposal,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    /// Ask the application to construct a block for the local proposer.
    RequestProposal {
        height: Height,
        round: Round,
        valid_block: Option<BlockId>,
        valid_round: Option<Round>,
    },
    /// Sign and atomically persist this message plus safety state before acknowledging it.
    PersistBeforeBroadcast {
        message: SignableMessage,
        safety_state: SafetyState,
    },
    /// Safe to send only because a matching `Persisted` event was received first.
    Broadcast(SignedMessage),
    ScheduleTimeout(Timeout),
    Finalize(Finalization),
    Evidence(Evidence),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsensusConfig {
    pub chain_id: String,
    pub propose_timeout: Duration,
    pub prevote_timeout: Duration,
    pub precommit_timeout: Duration,
    /// Linear timeout increase per round.
    pub timeout_step: Duration,
    /// Number of rounds of proposals and votes retained in memory.
    pub max_cached_rounds: usize,
    /// Maximum future-round distance accepted into bounded caches.
    pub max_round_ahead: Round,
    /// Absolute defense against an infinitely advancing height.
    pub max_rounds_per_height: Round,
    pub max_signature_bytes: usize,
}

impl ConsensusConfig {
    #[must_use]
    pub fn for_chain(chain_id: impl Into<String>) -> Self {
        Self {
            chain_id: chain_id.into(),
            propose_timeout: Duration::from_millis(800),
            prevote_timeout: Duration::from_millis(500),
            precommit_timeout: Duration::from_millis(500),
            timeout_step: Duration::from_millis(100),
            max_cached_rounds: 64,
            max_round_ahead: 16,
            max_rounds_per_height: 10_000,
            max_signature_bytes: 128,
        }
    }

    #[must_use]
    pub fn timeout_for(&self, phase: Phase, round: Round) -> Duration {
        let base = match phase {
            Phase::Propose => self.propose_timeout,
            Phase::Prevote => self.prevote_timeout,
            Phase::Precommit => self.precommit_timeout,
            Phase::Finalized => Duration::ZERO,
        };
        let step = self.timeout_step.saturating_mul(round);
        base.saturating_add(step)
    }
}

mod signature_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<u8>::deserialize(deserializer)
    }
}
