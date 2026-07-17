use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, VerifyingKey};
use kcoin_protocol::ChainId as ProtocolChainId;
use thiserror::Error;

use crate::types::{
    BlockId, CommitCertificate, ConsensusConfig, Effect, Event, Evidence, Finalization, Height,
    Phase, Proposal, Round, SafetyState, SignableMessage, SignedMessage, SignedProposal,
    SignedVote, Timeout, ValidatorId, ValidatorSet, Vote, VoteKind, VoteValue,
};

/// Verify a proposal's Ed25519 signature over its canonical consensus bytes.
/// Application block validation remains the caller's responsibility.
pub fn verify_ed25519_proposal(proposal: &SignedProposal) -> Result<(), ValidationError> {
    verify_ed25519(
        proposal.proposal.proposer,
        &SignableMessage::Proposal(proposal.proposal.clone()),
        &proposal.signature,
    )
}

/// Verify a vote's Ed25519 signature. Block precommits automatically use the
/// protocol commit domain, allowing their signatures to enter a protocol
/// certificate without re-signing.
pub fn verify_ed25519_vote(vote: &SignedVote) -> Result<(), ValidationError> {
    verify_ed25519(
        vote.vote.validator,
        &SignableMessage::Vote(vote.vote.clone()),
        &vote.signature,
    )
}

fn verify_ed25519(
    signer: ValidatorId,
    message: &SignableMessage,
    signature: &[u8],
) -> Result<(), ValidationError> {
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| ValidationError::InvalidSignature)?;
    let verifying_key =
        VerifyingKey::from_bytes(&signer.0).map_err(|_| ValidationError::InvalidSignature)?;
    verifying_key
        .verify_strict(
            &message
                .signing_bytes()
                .map_err(|error| ValidationError::Other(error.to_string()))?,
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| ValidationError::InvalidSignature)
}

/// Application and cryptographic validation supplied by the embedding node.
///
/// Implementations normally verify Ed25519 signatures and confirm that a
/// proposal refers to an available, application-valid block. The consensus
/// crate intentionally knows neither the signature algorithm nor block bytes.
pub trait MessageValidator {
    fn validate_proposal(&self, proposal: &SignedProposal) -> Result<(), ValidationError>;

    fn validate_vote(&self, vote: &SignedVote) -> Result<(), ValidationError>;

    /// Optional final application check (for example, block availability).
    fn validate_certificate(
        &self,
        _certificate: &CommitCertificate,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ValidationError {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("proposal is not valid for the application")]
    InvalidProposal,
    #[error("commit certificate is not valid for the application")]
    InvalidCertificate,
    #[error("validation failed: {0}")]
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ConsensusError {
    #[error("height must be greater than zero")]
    ZeroHeight,
    #[error("local validator {0} is not in the validator set")]
    LocalValidatorNotFound(ValidatorId),
    #[error("invalid consensus configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("consensus was already started")]
    AlreadyStarted,
    #[error("consensus has not been started")]
    NotStarted,
    #[error("persisted safety state belongs to height {actual}, expected {expected}")]
    RestoredHeightMismatch { expected: Height, actual: Height },
    #[error("persisted consensus safety state is malformed")]
    MalformedSafetyState,
    #[error("persisted round {0} cannot be resumed within the configured round limit")]
    RestoredRoundOutOfRange(Round),
    #[error("persisted local decision was signed by {actual}, expected {expected}")]
    RestoredDecisionSigner {
        expected: ValidatorId,
        actual: ValidatorId,
    },
    #[error("message belongs to chain {actual:?}, expected {expected:?}")]
    WrongChain { expected: String, actual: String },
    #[error("message belongs to height {actual}, expected {expected}")]
    WrongHeight { expected: Height, actual: Height },
    #[error("unknown validator {0}")]
    UnknownValidator(ValidatorId),
    #[error("validator {actual} proposed round {round}; expected {expected}")]
    UnexpectedProposer {
        round: Round,
        expected: ValidatorId,
        actual: ValidatorId,
    },
    #[error("round {round} is outside the bounded acceptance window")]
    RoundOutsideWindow { round: Round },
    #[error("round limit reached at {0}")]
    RoundLimitReached(Round),
    #[error("signature is larger than the configured bound")]
    SignatureTooLarge,
    #[error("proposal's valid-round proof is malformed")]
    InvalidValidRoundProof,
    #[error("commit certificate is malformed")]
    InvalidCertificate,
    #[error("no proposal was requested for height {height}, round {round}")]
    ProposalNotRequested { height: Height, round: Round },
    #[error("the persisted message does not match a pending local decision")]
    UnexpectedPersistedMessage,
    #[error("a conflicting block was certified: finalized {finalized}, received {received}")]
    ConflictingCommit {
        finalized: BlockId,
        received: BlockId,
    },
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DecisionKey {
    Proposal(Round),
    Vote(Round, VoteKind),
}

impl DecisionKey {
    fn of_signable(message: &SignableMessage) -> Self {
        match message {
            SignableMessage::Proposal(proposal) => Self::Proposal(proposal.round),
            SignableMessage::Vote(vote) => Self::Vote(vote.round, vote.kind),
        }
    }
}

/// One validator's deterministic consensus state for a single height.
pub struct Consensus {
    local_id: ValidatorId,
    validators: ValidatorSet,
    config: ConsensusConfig,
    height: Height,
    round: Round,
    phase: Phase,
    started: bool,
    finalized: Option<Finalization>,
    locked_round: Option<Round>,
    locked_block: Option<BlockId>,
    valid_round: Option<Round>,
    valid_block: Option<BlockId>,
    valid_proof: Vec<SignedVote>,
    proposals: BTreeMap<Round, SignedProposal>,
    votes: BTreeMap<(Round, VoteKind), BTreeMap<ValidatorId, SignedVote>>,
    requested_proposals: BTreeSet<Round>,
    local_decisions: BTreeMap<DecisionKey, SignableMessage>,
    pending_persistence: BTreeMap<DecisionKey, SignableMessage>,
}

impl Consensus {
    pub fn new(
        local_id: ValidatorId,
        validators: ValidatorSet,
        height: Height,
        config: ConsensusConfig,
    ) -> Result<Self, ConsensusError> {
        if height == 0 {
            return Err(ConsensusError::ZeroHeight);
        }
        if !validators.contains(local_id) {
            return Err(ConsensusError::LocalValidatorNotFound(local_id));
        }
        if ProtocolChainId::new(config.chain_id.clone()).is_err() {
            return Err(ConsensusError::InvalidConfig(
                "chain_id must be a valid KCoin protocol chain ID",
            ));
        }
        if config.max_cached_rounds == 0 {
            return Err(ConsensusError::InvalidConfig(
                "max_cached_rounds must be greater than zero",
            ));
        }
        if config.max_rounds_per_height == 0 {
            return Err(ConsensusError::InvalidConfig(
                "max_rounds_per_height must be greater than zero",
            ));
        }
        if config.max_signature_bytes == 0 {
            return Err(ConsensusError::InvalidConfig(
                "max_signature_bytes must be greater than zero",
            ));
        }

        Ok(Self {
            local_id,
            validators,
            config,
            height,
            round: 0,
            phase: Phase::Propose,
            started: false,
            finalized: None,
            locked_round: None,
            locked_block: None,
            valid_round: None,
            valid_block: None,
            valid_proof: Vec::new(),
            proposals: BTreeMap::new(),
            votes: BTreeMap::new(),
            requested_proposals: BTreeSet::new(),
            local_decisions: BTreeMap::new(),
            pending_persistence: BTreeMap::new(),
        })
    }

    /// Restore the durable lock and authenticated valid-value proof after a restart.
    ///
    /// A recovered validator resumes in the round after its most recent
    /// persisted signing decision. Skipping the partially completed round
    /// avoids requesting a second decision for an already-used signer slot;
    /// the restored lock still constrains every later prevote, while the
    /// restored PoLC lets a future proposer make progress across asymmetric
    /// locks.
    pub fn new_with_safety_state<V: MessageValidator>(
        local_id: ValidatorId,
        validators: ValidatorSet,
        height: Height,
        config: ConsensusConfig,
        safety_state: SafetyState,
        validator: &V,
    ) -> Result<Self, ConsensusError> {
        if safety_state.height != height {
            return Err(ConsensusError::RestoredHeightMismatch {
                expected: height,
                actual: safety_state.height,
            });
        }
        let resume_round = safety_state
            .round
            .checked_add(1)
            .filter(|round| *round < config.max_rounds_per_height)
            .ok_or(ConsensusError::RestoredRoundOutOfRange(safety_state.round))?;
        let mut consensus = Self::new(local_id, validators, height, config)?;
        consensus.validate_restored_safety_state(&safety_state, validator)?;
        consensus.round = resume_round;
        consensus.locked_round = safety_state.locked_round;
        consensus.locked_block = safety_state.locked_block;
        consensus.valid_round = safety_state.valid_round;
        consensus.valid_block = safety_state.valid_block;
        consensus.valid_proof = safety_state.valid_round_proof;
        Ok(consensus)
    }

    /// Reserve an already-durable local signer slot before `Start`.
    ///
    /// The embedding node must authenticate the stored signed message before
    /// calling this method. Restoring the decision prevents a restarted engine
    /// from requesting a different message for the same round and phase.
    pub fn restore_local_decision(
        &mut self,
        message: SignableMessage,
    ) -> Result<(), ConsensusError> {
        if self.started {
            return Err(ConsensusError::AlreadyStarted);
        }
        let (chain_id, height, signer) = match &message {
            SignableMessage::Proposal(proposal) => {
                let expected = self.validators.proposer(self.height, proposal.round);
                if proposal.proposer != expected {
                    return Err(ConsensusError::UnexpectedProposer {
                        round: proposal.round,
                        expected,
                        actual: proposal.proposer,
                    });
                }
                (&proposal.chain_id, proposal.height, proposal.proposer)
            }
            SignableMessage::Vote(vote) => (&vote.chain_id, vote.height, vote.validator),
        };
        self.validate_context(chain_id, height)?;
        if signer != self.local_id {
            return Err(ConsensusError::RestoredDecisionSigner {
                expected: self.local_id,
                actual: signer,
            });
        }
        if message.round() >= self.round || message.round() >= self.config.max_rounds_per_height {
            return Err(ConsensusError::MalformedSafetyState);
        }
        let key = DecisionKey::of_signable(&message);
        if let Some(existing) = self.local_decisions.get(&key) {
            return if existing == &message {
                Ok(())
            } else {
                Err(ConsensusError::UnexpectedPersistedMessage)
            };
        }
        self.local_decisions.insert(key, message);
        Ok(())
    }

    #[must_use]
    pub const fn height(&self) -> Height {
        self.height
    }

    #[must_use]
    pub const fn round(&self) -> Round {
        self.round
    }

    /// Authenticate an externally received message before the caller commits
    /// any accompanying bytes to durable storage.
    ///
    /// `Ok(true)` means the message is structurally valid and inside the
    /// engine's bounded round window. `Ok(false)` means it is otherwise valid
    /// but too old or too far ahead to cache. This method never mutates the
    /// consensus state.
    pub fn preflight_message<V: MessageValidator>(
        &self,
        message: &SignedMessage,
        validator: &V,
    ) -> Result<bool, ConsensusError> {
        self.validate_message(message, validator)?;
        let round = match message {
            SignedMessage::Proposal(proposal) => proposal.proposal.round,
            SignedMessage::Vote(vote) => vote.vote.round,
        };
        Ok(self.round_is_cacheable(round))
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub const fn local_id(&self) -> ValidatorId {
        self.local_id
    }

    #[must_use]
    pub const fn locked(&self) -> Option<(Round, BlockId)> {
        match (self.locked_round, self.locked_block) {
            (Some(round), Some(block)) => Some((round, block)),
            _ => None,
        }
    }

    #[must_use]
    pub const fn valid_value(&self) -> Option<(Round, BlockId)> {
        match (self.valid_round, self.valid_block) {
            (Some(round), Some(block)) => Some((round, block)),
            _ => None,
        }
    }

    #[must_use]
    pub fn finalization(&self) -> Option<&Finalization> {
        self.finalized.as_ref()
    }

    #[must_use]
    pub fn safety_state(&self) -> SafetyState {
        SafetyState {
            height: self.height,
            round: self.round,
            locked_round: self.locked_round,
            locked_block: self.locked_block,
            valid_round: self.valid_round,
            valid_block: self.valid_block,
            valid_round_proof: self.valid_proof.clone(),
        }
    }

    fn validate_restored_safety_state<V: MessageValidator>(
        &self,
        safety_state: &SafetyState,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        let lock_is_well_formed = match (safety_state.locked_round, safety_state.locked_block) {
            (None, None) => true,
            (Some(round), Some(_)) => round <= safety_state.round,
            _ => false,
        };
        let valid_value_is_well_formed = match (safety_state.valid_round, safety_state.valid_block)
        {
            (None, None) => safety_state.valid_round_proof.is_empty(),
            (Some(round), Some(_)) => {
                round <= safety_state.round
                    && safety_state.valid_round_proof.len() >= self.validators.quorum()
                    && safety_state.valid_round_proof.len() <= self.validators.len()
            }
            _ => false,
        };
        let lock_is_covered_by_valid_value = match (
            safety_state.locked_round,
            safety_state.locked_block,
            safety_state.valid_round,
            safety_state.valid_block,
        ) {
            (None, None, _, _) => true,
            (Some(locked_round), Some(locked_block), Some(valid_round), Some(valid_block)) => {
                valid_round > locked_round
                    || (valid_round == locked_round && valid_block == locked_block)
            }
            _ => false,
        };
        if !lock_is_well_formed || !valid_value_is_well_formed || !lock_is_covered_by_valid_value {
            return Err(ConsensusError::MalformedSafetyState);
        }

        let (Some(valid_round), Some(valid_block)) =
            (safety_state.valid_round, safety_state.valid_block)
        else {
            return Ok(());
        };
        let mut signers = BTreeSet::new();
        for proof_vote in &safety_state.valid_round_proof {
            if proof_vote.signature.len() > self.config.max_signature_bytes {
                return Err(ConsensusError::SignatureTooLarge);
            }
            self.validate_vote_structure(proof_vote)?;
            let vote = &proof_vote.vote;
            if vote.round != valid_round
                || vote.kind != VoteKind::Prevote
                || vote.value != VoteValue::Block(valid_block)
                || !signers.insert(vote.validator)
            {
                return Err(ConsensusError::MalformedSafetyState);
            }
            validator.validate_vote(proof_vote)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn cached_round_count(&self) -> usize {
        let mut rounds = BTreeSet::new();
        rounds.extend(self.proposals.keys().copied());
        rounds.extend(self.votes.keys().map(|(round, _)| *round));
        rounds.len()
    }

    #[must_use]
    pub fn vote_count(&self, round: Round, kind: VoteKind) -> usize {
        self.votes.get(&(round, kind)).map_or(0, BTreeMap::len)
    }

    pub fn step<V: MessageValidator>(
        &mut self,
        event: Event,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        if let Event::Start = event {
            if self.started {
                return Err(ConsensusError::AlreadyStarted);
            }
            self.started = true;
            return self.enter_round(self.round, validator);
        }

        if let Event::Certificate(certificate) = event {
            self.validate_certificate(&certificate, validator)?;
            return self.finalize(certificate);
        }

        if !self.started {
            return Err(ConsensusError::NotStarted);
        }
        if self.finalized.is_some() {
            return Ok(Vec::new());
        }

        match event {
            Event::Start | Event::Certificate(_) => unreachable!("handled above"),
            Event::ProposalReady {
                height,
                round,
                block_id,
            } => self.proposal_ready(height, round, block_id),
            Event::Message(message) => self.receive_message(message, validator),
            Event::Persisted(message) => self.persisted(message, validator),
            Event::Timeout(timeout) => self.handle_timeout(timeout, validator),
        }
    }

    fn enter_round<V: MessageValidator>(
        &mut self,
        round: Round,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        if round >= self.config.max_rounds_per_height {
            return Err(ConsensusError::RoundLimitReached(round));
        }
        self.round = round;
        self.phase = Phase::Propose;
        self.prune_caches();

        let mut effects = vec![Effect::ScheduleTimeout(self.timeout(Phase::Propose))];
        if self.proposals.contains_key(&round) {
            self.activate_current_round(&mut effects, validator)?;
        } else if self.validators.proposer(self.height, round) == self.local_id {
            self.requested_proposals.insert(round);
            effects.push(Effect::RequestProposal {
                height: self.height,
                round,
                valid_block: self.valid_block,
                valid_round: self.valid_round,
            });
        }
        Ok(effects)
    }

    fn proposal_ready(
        &mut self,
        height: Height,
        round: Round,
        block_id: BlockId,
    ) -> Result<Vec<Effect>, ConsensusError> {
        if height != self.height {
            return Err(ConsensusError::WrongHeight {
                expected: self.height,
                actual: height,
            });
        }
        if !self.requested_proposals.remove(&round) {
            return Err(ConsensusError::ProposalNotRequested { height, round });
        }

        let proof = match self.valid_round {
            Some(valid_round) if valid_round < round && self.valid_block == Some(block_id) => {
                self.valid_proof.clone()
            }
            _ => Vec::new(),
        };
        let valid_round = if proof.is_empty() {
            None
        } else {
            self.valid_round
        };
        let message = SignableMessage::Proposal(Proposal {
            chain_id: self.config.chain_id.clone(),
            height,
            round,
            proposer: self.local_id,
            block_id,
            valid_round,
            valid_round_proof: proof,
        });
        self.request_persistence(message)
    }

    fn receive_message<V: MessageValidator>(
        &mut self,
        message: SignedMessage,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        self.validate_message(&message, validator)?;
        self.accept_validated_message(message, validator)
    }

    fn persisted<V: MessageValidator>(
        &mut self,
        message: SignedMessage,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        let signable = message.signable();
        let key = DecisionKey::of_signable(&signable);
        if self.pending_persistence.get(&key) != Some(&signable) {
            return Err(ConsensusError::UnexpectedPersistedMessage);
        }
        self.validate_message(&message, validator)?;
        self.pending_persistence.remove(&key);

        let mut effects = vec![Effect::Broadcast(message.clone())];
        effects.extend(self.accept_validated_message(message, validator)?);
        Ok(effects)
    }

    fn accept_validated_message<V: MessageValidator>(
        &mut self,
        message: SignedMessage,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        match message {
            SignedMessage::Proposal(proposal) => self.accept_proposal(proposal, validator),
            SignedMessage::Vote(vote) => self.accept_vote(vote, validator),
        }
    }

    fn accept_proposal<V: MessageValidator>(
        &mut self,
        proposal: SignedProposal,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        let round = proposal.proposal.round;
        if !self.round_is_cacheable(round) {
            return Ok(Vec::new());
        }
        if let Some(first) = self.proposals.get(&round) {
            if first == &proposal {
                return Ok(Vec::new());
            }
            return Ok(vec![Effect::Evidence(Evidence::DoubleProposal {
                proposer: proposal.proposal.proposer,
                first: first.clone(),
                second: proposal,
            })]);
        }
        self.proposals.insert(round, proposal);

        let mut effects = Vec::new();
        if round == self.round {
            self.activate_current_round(&mut effects, validator)?;
        }
        Ok(effects)
    }

    fn accept_vote<V: MessageValidator>(
        &mut self,
        vote: SignedVote,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        let round = vote.vote.round;
        if !self.round_is_cacheable(round) {
            return Ok(Vec::new());
        }
        let key = (round, vote.vote.kind);
        let by_validator = self.votes.entry(key).or_default();
        if let Some(first) = by_validator.get(&vote.vote.validator) {
            if first == &vote {
                return Ok(Vec::new());
            }
            return Ok(vec![Effect::Evidence(Evidence::DoubleVote {
                validator: vote.vote.validator,
                first: first.clone(),
                second: vote,
            })]);
        }
        by_validator.insert(vote.vote.validator, vote);

        let mut effects = Vec::new();
        self.evaluate_round(round, &mut effects, validator)?;
        Ok(effects)
    }

    fn activate_current_round<V: MessageValidator>(
        &mut self,
        effects: &mut Vec<Effect>,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        if self.phase == Phase::Propose {
            if let Some(proposal) = self.proposals.get(&self.round).cloned() {
                let value = self.prevote_for(&proposal.proposal);
                self.cast_vote(VoteKind::Prevote, value, effects);
            }
        }
        self.evaluate_round(self.round, effects, validator)
    }

    fn evaluate_round<V: MessageValidator>(
        &mut self,
        round: Round,
        effects: &mut Vec<Effect>,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        if let Some((VoteValue::Block(block_id), proof)) =
            self.quorum_votes(round, VoteKind::Prevote)
        {
            let has_proposal = self
                .proposals
                .get(&round)
                .is_some_and(|proposal| proposal.proposal.block_id == block_id);
            if has_proposal
                && round <= self.round
                && self.valid_round.is_none_or(|known| round > known)
            {
                self.valid_round = Some(round);
                self.valid_block = Some(block_id);
                self.valid_proof = proof;
            }
            if has_proposal && round == self.round && self.phase == Phase::Prevote {
                self.locked_round = Some(round);
                self.locked_block = Some(block_id);
                self.cast_vote(VoteKind::Precommit, VoteValue::Block(block_id), effects);
            }
        } else if let Some((VoteValue::Nil, _)) = self.quorum_votes(round, VoteKind::Prevote) {
            if round == self.round && self.phase == Phase::Prevote {
                self.cast_vote(VoteKind::Precommit, VoteValue::Nil, effects);
            }
        }

        if let Some((VoteValue::Block(block_id), precommits)) =
            self.quorum_votes(round, VoteKind::Precommit)
        {
            let certificate = CommitCertificate {
                chain_id: self.config.chain_id.clone(),
                height: self.height,
                round,
                block_id,
                precommits,
            };
            validator.validate_certificate(&certificate)?;
            effects.extend(self.finalize(certificate)?);
        }
        Ok(())
    }

    fn prevote_for(&self, proposal: &Proposal) -> VoteValue {
        match (self.locked_round, self.locked_block) {
            (None, None) => VoteValue::Block(proposal.block_id),
            (_, Some(locked)) if locked == proposal.block_id => VoteValue::Block(proposal.block_id),
            (Some(locked_round), Some(_))
                if proposal
                    .valid_round
                    .is_some_and(|valid_round| valid_round > locked_round) =>
            {
                VoteValue::Block(proposal.block_id)
            }
            _ => VoteValue::Nil,
        }
    }

    fn cast_vote(&mut self, kind: VoteKind, value: VoteValue, effects: &mut Vec<Effect>) {
        let message = SignableMessage::Vote(Vote {
            chain_id: self.config.chain_id.clone(),
            height: self.height,
            round: self.round,
            kind,
            validator: self.local_id,
            value,
        });
        let key = DecisionKey::Vote(self.round, kind);
        if self.local_decisions.contains_key(&key) {
            return;
        }
        self.local_decisions.insert(key, message.clone());
        self.pending_persistence.insert(key, message.clone());
        effects.push(Effect::PersistBeforeBroadcast {
            message,
            safety_state: self.safety_state(),
        });

        self.phase = match kind {
            VoteKind::Prevote => Phase::Prevote,
            VoteKind::Precommit => Phase::Precommit,
        };
        effects.push(Effect::ScheduleTimeout(self.timeout(self.phase)));
    }

    fn request_persistence(
        &mut self,
        message: SignableMessage,
    ) -> Result<Vec<Effect>, ConsensusError> {
        let key = DecisionKey::of_signable(&message);
        if let Some(existing) = self.local_decisions.get(&key) {
            if existing == &message {
                return Ok(Vec::new());
            }
            return Err(ConsensusError::UnexpectedPersistedMessage);
        }
        self.local_decisions.insert(key, message.clone());
        self.pending_persistence.insert(key, message.clone());
        Ok(vec![Effect::PersistBeforeBroadcast {
            message,
            safety_state: self.safety_state(),
        }])
    }

    fn handle_timeout<V: MessageValidator>(
        &mut self,
        timeout: Timeout,
        validator: &V,
    ) -> Result<Vec<Effect>, ConsensusError> {
        if timeout.height != self.height
            || timeout.round != self.round
            || timeout.phase != self.phase
            || timeout.after != self.config.timeout_for(self.phase, self.round)
        {
            return Ok(Vec::new());
        }

        let mut effects = Vec::new();
        match self.phase {
            Phase::Propose => {
                self.cast_vote(VoteKind::Prevote, VoteValue::Nil, &mut effects);
                self.evaluate_round(self.round, &mut effects, validator)?;
            }
            Phase::Prevote => {
                self.cast_vote(VoteKind::Precommit, VoteValue::Nil, &mut effects);
                self.evaluate_round(self.round, &mut effects, validator)?;
            }
            Phase::Precommit => {
                let next = self
                    .round
                    .checked_add(1)
                    .ok_or(ConsensusError::RoundLimitReached(self.round))?;
                effects.extend(self.enter_round(next, validator)?);
            }
            Phase::Finalized => {}
        }
        Ok(effects)
    }

    fn finalize(&mut self, certificate: CommitCertificate) -> Result<Vec<Effect>, ConsensusError> {
        if let Some(finalized) = &self.finalized {
            if finalized.block_id == certificate.block_id {
                return Ok(Vec::new());
            }
            return Err(ConsensusError::ConflictingCommit {
                finalized: finalized.block_id,
                received: certificate.block_id,
            });
        }
        let finalization = Finalization {
            height: certificate.height,
            round: certificate.round,
            block_id: certificate.block_id,
            certificate,
        };
        self.phase = Phase::Finalized;
        self.finalized = Some(finalization.clone());
        Ok(vec![Effect::Finalize(finalization)])
    }

    fn validate_message<V: MessageValidator>(
        &self,
        message: &SignedMessage,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        if message.signature().len() > self.config.max_signature_bytes {
            return Err(ConsensusError::SignatureTooLarge);
        }
        match message {
            SignedMessage::Proposal(proposal) => {
                self.validate_proposal_structure(proposal, validator)?;
                validator.validate_proposal(proposal)?;
            }
            SignedMessage::Vote(vote) => {
                self.validate_vote_structure(vote)?;
                validator.validate_vote(vote)?;
            }
        }
        Ok(())
    }

    fn validate_proposal_structure<V: MessageValidator>(
        &self,
        signed: &SignedProposal,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        let proposal = &signed.proposal;
        self.validate_context(&proposal.chain_id, proposal.height)?;
        if !self.validators.contains(proposal.proposer) {
            return Err(ConsensusError::UnknownValidator(proposal.proposer));
        }
        let expected = self.validators.proposer(self.height, proposal.round);
        if proposal.proposer != expected {
            return Err(ConsensusError::UnexpectedProposer {
                round: proposal.round,
                expected,
                actual: proposal.proposer,
            });
        }
        if proposal.round >= self.config.max_rounds_per_height {
            return Err(ConsensusError::RoundOutsideWindow {
                round: proposal.round,
            });
        }
        match proposal.valid_round {
            None if proposal.valid_round_proof.is_empty() => {}
            Some(valid_round) if valid_round < proposal.round => {
                if proposal.valid_round_proof.len() < self.validators.quorum()
                    || proposal.valid_round_proof.len() > self.validators.len()
                {
                    return Err(ConsensusError::InvalidValidRoundProof);
                }
                let mut signers = BTreeSet::new();
                for proof_vote in &proposal.valid_round_proof {
                    if proof_vote.signature.len() > self.config.max_signature_bytes {
                        return Err(ConsensusError::SignatureTooLarge);
                    }
                    self.validate_vote_structure(proof_vote)?;
                    let vote = &proof_vote.vote;
                    if vote.round != valid_round
                        || vote.kind != VoteKind::Prevote
                        || vote.value != VoteValue::Block(proposal.block_id)
                        || !signers.insert(vote.validator)
                    {
                        return Err(ConsensusError::InvalidValidRoundProof);
                    }
                    validator.validate_vote(proof_vote)?;
                }
            }
            _ => return Err(ConsensusError::InvalidValidRoundProof),
        }
        Ok(())
    }

    fn validate_vote_structure(&self, signed: &SignedVote) -> Result<(), ConsensusError> {
        let vote = &signed.vote;
        self.validate_context(&vote.chain_id, vote.height)?;
        if !self.validators.contains(vote.validator) {
            return Err(ConsensusError::UnknownValidator(vote.validator));
        }
        if vote.round >= self.config.max_rounds_per_height {
            return Err(ConsensusError::RoundOutsideWindow { round: vote.round });
        }
        Ok(())
    }

    fn validate_certificate<V: MessageValidator>(
        &self,
        certificate: &CommitCertificate,
        validator: &V,
    ) -> Result<(), ConsensusError> {
        self.validate_context(&certificate.chain_id, certificate.height)?;
        if certificate.round >= self.config.max_rounds_per_height
            || certificate.precommits.len() < self.validators.quorum()
            || certificate.precommits.len() > self.validators.len()
        {
            return Err(ConsensusError::InvalidCertificate);
        }
        let mut signers = BTreeSet::new();
        for signed in &certificate.precommits {
            if signed.signature.len() > self.config.max_signature_bytes {
                return Err(ConsensusError::SignatureTooLarge);
            }
            self.validate_vote_structure(signed)?;
            let vote = &signed.vote;
            if vote.round != certificate.round
                || vote.kind != VoteKind::Precommit
                || vote.value != VoteValue::Block(certificate.block_id)
                || !signers.insert(vote.validator)
            {
                return Err(ConsensusError::InvalidCertificate);
            }
            validator.validate_vote(signed)?;
        }
        validator.validate_certificate(certificate)?;
        Ok(())
    }

    fn validate_context(&self, chain_id: &str, height: Height) -> Result<(), ConsensusError> {
        if chain_id != self.config.chain_id {
            return Err(ConsensusError::WrongChain {
                expected: self.config.chain_id.clone(),
                actual: chain_id.to_owned(),
            });
        }
        if height != self.height {
            return Err(ConsensusError::WrongHeight {
                expected: self.height,
                actual: height,
            });
        }
        Ok(())
    }

    fn quorum_votes(&self, round: Round, kind: VoteKind) -> Option<(VoteValue, Vec<SignedVote>)> {
        let votes = self.votes.get(&(round, kind))?;
        let mut by_value: BTreeMap<VoteValue, Vec<SignedVote>> = BTreeMap::new();
        for vote in votes.values() {
            by_value
                .entry(vote.vote.value)
                .or_default()
                .push(vote.clone());
        }
        by_value
            .into_iter()
            .find(|(_, votes)| votes.len() >= self.validators.quorum())
    }

    fn round_is_cacheable(&self, round: Round) -> bool {
        let retained_before = self.config.max_cached_rounds.saturating_sub(1) as Round;
        let minimum = self.round.saturating_sub(retained_before);
        let maximum = self.round.saturating_add(self.config.max_round_ahead);
        (minimum..=maximum).contains(&round)
    }

    fn prune_caches(&mut self) {
        let retained_before = self.config.max_cached_rounds.saturating_sub(1) as Round;
        let minimum = self.round.saturating_sub(retained_before);
        let maximum = self.round.saturating_add(self.config.max_round_ahead);
        let keep = |round: Round| (minimum..=maximum).contains(&round);
        self.proposals.retain(|round, _| keep(*round));
        self.votes.retain(|(round, _), _| keep(*round));
        self.requested_proposals.retain(|round| keep(*round));
        self.local_decisions.retain(|key, _| {
            keep(match key {
                DecisionKey::Proposal(round) | DecisionKey::Vote(round, _) => *round,
            })
        });
        self.pending_persistence.retain(|key, _| {
            keep(match key {
                DecisionKey::Proposal(round) | DecisionKey::Vote(round, _) => *round,
            })
        });
    }

    fn timeout(&self, phase: Phase) -> Timeout {
        Timeout {
            height: self.height,
            round: self.round,
            phase,
            after: self.config.timeout_for(phase, self.round),
        }
    }
}
