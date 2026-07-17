//! Deterministic virtual time and networking for consensus tests and demos.
//!
//! Enable the `simulation` feature to use this module outside this crate's
//! tests. Signatures here are intentionally fake and must never be used by a
//! real node.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use crate::{
    BlockId, CommitCertificate, Consensus, ConsensusConfig, ConsensusError, Effect, Event,
    Finalization, MessageValidator, SignableMessage, SignedMessage, SignedProposal, SignedVote,
    Timeout, ValidationError, ValidatorId, ValidatorSet,
};

/// A validator used in deterministic simulations. Its "signature" is the
/// validator ID itself, making forged-message tests easy without cryptography.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicValidator;

impl MessageValidator for DeterministicValidator {
    fn validate_proposal(&self, proposal: &SignedProposal) -> Result<(), ValidationError> {
        validate_signature(proposal.proposal.proposer, &proposal.signature)
    }

    fn validate_vote(&self, vote: &SignedVote) -> Result<(), ValidationError> {
        validate_signature(vote.vote.validator, &vote.signature)
    }
}

fn validate_signature(signer: ValidatorId, signature: &[u8]) -> Result<(), ValidationError> {
    if signature == signer.0 {
        Ok(())
    } else {
        Err(ValidationError::InvalidSignature)
    }
}

#[must_use]
pub fn sign(message: SignableMessage) -> SignedMessage {
    match message {
        SignableMessage::Proposal(proposal) => {
            let signature = proposal.proposer.0.to_vec();
            SignedMessage::Proposal(SignedProposal {
                proposal,
                signature,
            })
        }
        SignableMessage::Vote(vote) => {
            let signature = vote.validator.0.to_vec();
            SignedMessage::Vote(SignedVote { vote, signature })
        }
    }
}

#[must_use]
pub const fn validator_id(index: u8) -> ValidatorId {
    ValidatorId([index; 32])
}

#[must_use]
pub fn four_validators() -> ValidatorSet {
    ValidatorSet::new((0..4).map(validator_id).collect())
        .expect("the four deterministic validator IDs are distinct")
}

#[derive(Clone, Debug)]
struct ScheduledTimeout {
    at: Duration,
    node: ValidatorId,
    timeout: Timeout,
}

enum Work {
    Event(ValidatorId, Event),
    Effect(ValidatorId, Effect),
}

/// A single-threaded, virtual-time network. Links are directed internally but
/// the public connect/disconnect helpers update both directions.
pub struct VirtualNetwork {
    nodes: BTreeMap<ValidatorId, Consensus>,
    verifier: DeterministicValidator,
    online: BTreeSet<ValidatorId>,
    started: BTreeSet<ValidatorId>,
    links: BTreeSet<(ValidatorId, ValidatorId)>,
    timers: Vec<ScheduledTimeout>,
    now: Duration,
    finalizations: BTreeMap<ValidatorId, Finalization>,
    broadcast_log: Vec<(ValidatorId, SignedMessage)>,
    effect_log: Vec<(ValidatorId, Effect)>,
}

impl VirtualNetwork {
    pub fn new(
        validators: ValidatorSet,
        height: u64,
        config: ConsensusConfig,
    ) -> Result<Self, ConsensusError> {
        let mut nodes = BTreeMap::new();
        let mut online = BTreeSet::new();
        let mut links = BTreeSet::new();
        for validator in validators.validators() {
            nodes.insert(
                *validator,
                Consensus::new(*validator, validators.clone(), height, config.clone())?,
            );
            online.insert(*validator);
        }
        for from in validators.validators() {
            for to in validators.validators() {
                if from != to {
                    links.insert((*from, *to));
                }
            }
        }
        Ok(Self {
            nodes,
            verifier: DeterministicValidator,
            online,
            started: BTreeSet::new(),
            links,
            timers: Vec::new(),
            now: Duration::ZERO,
            finalizations: BTreeMap::new(),
            broadcast_log: Vec::new(),
            effect_log: Vec::new(),
        })
    }

    #[must_use]
    pub const fn now(&self) -> Duration {
        self.now
    }

    #[must_use]
    pub fn node(&self, validator: ValidatorId) -> Option<&Consensus> {
        self.nodes.get(&validator)
    }

    #[must_use]
    pub fn finalizations(&self) -> &BTreeMap<ValidatorId, Finalization> {
        &self.finalizations
    }

    #[must_use]
    pub fn broadcasts(&self) -> &[(ValidatorId, SignedMessage)] {
        &self.broadcast_log
    }

    #[must_use]
    pub fn effects(&self) -> &[(ValidatorId, Effect)] {
        &self.effect_log
    }

    pub fn set_online(
        &mut self,
        validator: ValidatorId,
        online: bool,
    ) -> Result<(), ConsensusError> {
        if online {
            self.online.insert(validator);
            if self.started.insert(validator) {
                self.process([Work::Event(validator, Event::Start)])?;
            }
        } else {
            self.online.remove(&validator);
        }
        Ok(())
    }

    pub fn connect(&mut self, a: ValidatorId, b: ValidatorId) {
        self.links.insert((a, b));
        self.links.insert((b, a));
    }

    pub fn disconnect(&mut self, a: ValidatorId, b: ValidatorId) {
        self.links.remove(&(a, b));
        self.links.remove(&(b, a));
    }

    pub fn connect_all(&mut self) {
        self.links.clear();
        let validators: Vec<_> = self.nodes.keys().copied().collect();
        for from in &validators {
            for to in &validators {
                if from != to {
                    self.links.insert((*from, *to));
                }
            }
        }
    }

    /// Replace network connectivity with disjoint, fully connected groups.
    pub fn partition(&mut self, groups: &[&[ValidatorId]]) {
        self.links.clear();
        for group in groups {
            for from in *group {
                for to in *group {
                    if from != to {
                        self.links.insert((*from, *to));
                    }
                }
            }
        }
    }

    /// Starts every online validator before processing any start-up effect, so
    /// the round-zero proposal cannot race an unstarted peer in the simulation.
    pub fn start(&mut self) -> Result<(), ConsensusError> {
        let nodes: Vec<_> = self.online.iter().copied().collect();
        let mut effects = Vec::new();
        for node in nodes {
            if self.started.insert(node) {
                let emitted = self
                    .nodes
                    .get_mut(&node)
                    .expect("online node exists")
                    .step(Event::Start, &self.verifier)?;
                effects.extend(emitted.into_iter().map(|effect| Work::Effect(node, effect)));
            }
        }
        self.process(effects)
    }

    pub fn inject(
        &mut self,
        recipient: ValidatorId,
        message: SignedMessage,
    ) -> Result<(), ConsensusError> {
        self.process([Work::Event(recipient, Event::Message(message))])
    }

    pub fn deliver_certificate(
        &mut self,
        recipient: ValidatorId,
        certificate: CommitCertificate,
    ) -> Result<(), ConsensusError> {
        self.process([Work::Event(recipient, Event::Certificate(certificate))])
    }

    /// Advance to the next scheduled timeout among online nodes. Returns false
    /// if no online timer remains.
    pub fn advance_to_next_timeout(&mut self) -> Result<bool, ConsensusError> {
        let next = self
            .timers
            .iter()
            .filter(|timer| self.online.contains(&timer.node))
            .map(|timer| timer.at.max(self.now))
            .min();
        let Some(next) = next else {
            return Ok(false);
        };
        self.now = next;

        let mut due = Vec::new();
        let mut retained = Vec::new();
        for timer in self.timers.drain(..) {
            if timer.at <= next && self.online.contains(&timer.node) {
                due.push(timer);
            } else {
                retained.push(timer);
            }
        }
        self.timers = retained;
        due.sort_by_key(|timer| timer.node);
        self.process(
            due.into_iter()
                .map(|timer| Work::Event(timer.node, Event::Timeout(timer.timeout))),
        )?;
        Ok(true)
    }

    fn process(&mut self, work: impl IntoIterator<Item = Work>) -> Result<(), ConsensusError> {
        let mut queue: VecDeque<_> = work.into_iter().collect();
        let mut processed = 0_usize;
        while let Some(work) = queue.pop_front() {
            processed += 1;
            assert!(processed < 100_000, "virtual network did not quiesce");
            match work {
                Work::Event(node, event) => {
                    if !self.online.contains(&node) {
                        continue;
                    }
                    let effects = self
                        .nodes
                        .get_mut(&node)
                        .expect("event recipient exists")
                        .step(event, &self.verifier)?;
                    queue.extend(effects.into_iter().map(|effect| Work::Effect(node, effect)));
                }
                Work::Effect(node, effect) => {
                    self.effect_log.push((node, effect.clone()));
                    if !self.online.contains(&node) {
                        continue;
                    }
                    match effect {
                        Effect::RequestProposal {
                            height,
                            round,
                            valid_block,
                            ..
                        } => queue.push_back(Work::Event(
                            node,
                            Event::ProposalReady {
                                height,
                                round,
                                block_id: valid_block
                                    .unwrap_or_else(|| deterministic_block(height, round)),
                            },
                        )),
                        Effect::PersistBeforeBroadcast { message, .. } => {
                            queue.push_back(Work::Event(node, Event::Persisted(sign(message))))
                        }
                        Effect::Broadcast(message) => {
                            self.broadcast_log.push((node, message.clone()));
                            let recipients: Vec<_> = self
                                .online
                                .iter()
                                .copied()
                                .filter(|recipient| {
                                    *recipient != node && self.links.contains(&(node, *recipient))
                                })
                                .collect();
                            queue.extend(recipients.into_iter().map(|recipient| {
                                Work::Event(recipient, Event::Message(message.clone()))
                            }));
                        }
                        Effect::ScheduleTimeout(timeout) => {
                            self.timers.push(ScheduledTimeout {
                                at: self.now.saturating_add(timeout.after),
                                node,
                                timeout,
                            });
                        }
                        Effect::Finalize(finalization) => {
                            self.finalizations.insert(node, finalization);
                        }
                        Effect::Evidence(_) => {}
                    }
                }
            }
        }
        Ok(())
    }
}

#[must_use]
pub fn deterministic_block(height: u64, round: u32) -> BlockId {
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&height.to_be_bytes());
    bytes[8..12].copy_from_slice(&round.to_be_bytes());
    BlockId(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConsensusError, Evidence, Phase, Proposal, SafetyState, Vote, VoteKind, VoteValue,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use kcoin_protocol::{
        ChainId as ProtocolChainId, CommitVote as ProtocolCommitVote, Hash32 as ProtocolHash32,
        ValidatorId as ProtocolValidatorId,
    };

    fn config() -> ConsensusConfig {
        let mut config = ConsensusConfig::for_chain("kcoin-test");
        config.propose_timeout = Duration::from_millis(10);
        config.prevote_timeout = Duration::from_millis(10);
        config.precommit_timeout = Duration::from_millis(10);
        config.timeout_step = Duration::from_millis(1);
        config
    }

    fn signed_proposal(
        round: u32,
        block_id: BlockId,
        valid_round: Option<u32>,
        proof: Vec<SignedVote>,
    ) -> SignedMessage {
        sign(SignableMessage::Proposal(Proposal {
            chain_id: "kcoin-test".to_owned(),
            height: 1,
            round,
            proposer: four_validators().proposer(1, round),
            block_id,
            valid_round,
            valid_round_proof: proof,
        }))
    }

    fn signed_vote(
        validator: ValidatorId,
        round: u32,
        kind: VoteKind,
        value: VoteValue,
    ) -> SignedVote {
        match sign(SignableMessage::Vote(Vote {
            chain_id: "kcoin-test".to_owned(),
            height: 1,
            round,
            kind,
            validator,
            value,
        })) {
            SignedMessage::Vote(vote) => vote,
            SignedMessage::Proposal(_) => unreachable!(),
        }
    }

    #[test]
    fn quorum_is_floor_two_thirds_plus_one() {
        assert_eq!(four_validators().quorum(), 3);
        let seven = ValidatorSet::new((0..7).map(validator_id).collect()).unwrap();
        assert_eq!(seven.quorum(), 5);
    }

    #[test]
    fn three_of_four_finalize_with_one_offline() {
        let validators = four_validators();
        let mut network = VirtualNetwork::new(validators, 1, config()).unwrap();
        network.set_online(validator_id(3), false).unwrap();
        network.start().unwrap();

        assert_eq!(network.finalizations().len(), 3);
        let blocks: BTreeSet<_> = network
            .finalizations()
            .values()
            .map(|finalization| finalization.block_id)
            .collect();
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn offline_proposer_advances_round_then_finalizes() {
        let validators = four_validators();
        let mut network = VirtualNetwork::new(validators, 1, config()).unwrap();
        network.set_online(validator_id(0), false).unwrap();
        network.start().unwrap();

        for _ in 0..20 {
            if network.finalizations().len() == 3 {
                break;
            }
            assert!(network.advance_to_next_timeout().unwrap());
        }
        assert_eq!(network.finalizations().len(), 3);
        assert!(
            network
                .finalizations()
                .values()
                .all(|finalization| finalization.round >= 1)
        );
    }

    #[test]
    fn two_two_partition_halts_then_converges_after_healing() {
        let validators = four_validators();
        let mut network = VirtualNetwork::new(validators, 1, config()).unwrap();
        let left = [validator_id(0), validator_id(1)];
        let right = [validator_id(2), validator_id(3)];
        network.partition(&[&left, &right]);
        network.start().unwrap();

        for _ in 0..80 {
            assert!(network.advance_to_next_timeout().unwrap());
        }
        assert!(network.finalizations().is_empty());

        network.connect_all();
        for _ in 0..160 {
            if network.finalizations().len() == 4 {
                break;
            }
            assert!(network.advance_to_next_timeout().unwrap());
        }
        assert_eq!(network.finalizations().len(), 4);
        let finalized_values: BTreeSet<_> = network
            .finalizations()
            .values()
            .map(|finalization| finalization.block_id)
            .collect();
        assert_eq!(finalized_values.len(), 1);
    }

    #[test]
    fn overdue_timer_after_restart_never_moves_virtual_time_backward() {
        let validators = four_validators();
        let mut network = VirtualNetwork::new(validators, 1, config()).unwrap();
        let left = [validator_id(0), validator_id(1)];
        let right = [validator_id(2), validator_id(3)];
        network.partition(&[&left, &right]);
        network.start().unwrap();
        network.set_online(validator_id(3), false).unwrap();
        for _ in 0..5 {
            assert!(network.advance_to_next_timeout().unwrap());
        }
        let before_restart = network.now();
        network.set_online(validator_id(3), true).unwrap();
        assert!(network.advance_to_next_timeout().unwrap());
        assert!(network.now() >= before_restart);
    }

    #[test]
    fn duplicate_and_equivocating_votes_count_only_once() {
        let validators = four_validators();
        let mut consensus = Consensus::new(validator_id(3), validators, 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        let block_a = deterministic_block(1, 0);
        let block_b = BlockId([99; 32]);
        let first = signed_vote(
            validator_id(0),
            0,
            VoteKind::Prevote,
            VoteValue::Block(block_a),
        );
        consensus
            .step(
                Event::Message(SignedMessage::Vote(first.clone())),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(
            consensus
                .step(
                    Event::Message(SignedMessage::Vote(first.clone())),
                    &DeterministicValidator,
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(consensus.vote_count(0, VoteKind::Prevote), 1);

        let conflicting = signed_vote(
            validator_id(0),
            0,
            VoteKind::Prevote,
            VoteValue::Block(block_b),
        );
        let effects = consensus
            .step(
                Event::Message(SignedMessage::Vote(conflicting)),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(matches!(
            effects.as_slice(),
            [Effect::Evidence(Evidence::DoubleVote { .. })]
        ));
        assert_eq!(consensus.vote_count(0, VoteKind::Prevote), 1);
    }

    #[test]
    fn forged_vote_is_rejected_before_entering_cache() {
        let validators = four_validators();
        let mut consensus = Consensus::new(validator_id(3), validators, 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        let mut forged = signed_vote(validator_id(0), 0, VoteKind::Prevote, VoteValue::Nil);
        forged.signature = vec![42; 32];
        let result = consensus.step(
            Event::Message(SignedMessage::Vote(forged)),
            &DeterministicValidator,
        );
        assert_eq!(
            result,
            Err(ConsensusError::Validation(
                ValidationError::InvalidSignature
            ))
        );
        assert_eq!(consensus.vote_count(0, VoteKind::Prevote), 0);
    }

    #[test]
    fn future_polc_waits_for_its_round_and_local_timeout_state_survives_restart() {
        let validators = four_validators();
        let local = validator_id(3);
        let future_block = deterministic_block(1, 1);
        let mut consensus = Consensus::new(local, validators.clone(), 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();

        // A complete round-one PoLC may arrive while this validator is still
        // in round zero. It is authenticated and cached, but must not become
        // durable valid-value state until the local machine reaches round one.
        consensus
            .step(
                Event::Message(signed_proposal(1, future_block, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        for index in 0..3 {
            consensus
                .step(
                    Event::Message(SignedMessage::Vote(signed_vote(
                        validator_id(index),
                        1,
                        VoteKind::Prevote,
                        VoteValue::Block(future_block),
                    ))),
                    &DeterministicValidator,
                )
                .unwrap();
        }
        assert_eq!(consensus.vote_count(1, VoteKind::Prevote), 3);
        assert_eq!(consensus.valid_value(), None);

        // The next local timeout decision is therefore self-consistent and
        // can be persisted and restored after an immediate crash.
        let effects = consensus
            .step(
                Event::Timeout(Timeout {
                    height: 1,
                    round: 0,
                    phase: Phase::Propose,
                    after: config().timeout_for(Phase::Propose, 0),
                }),
                &DeterministicValidator,
            )
            .unwrap();
        let (local_prevote, durable_safety) = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::PersistBeforeBroadcast {
                    message: SignableMessage::Vote(vote),
                    safety_state,
                } if vote.kind == VoteKind::Prevote => {
                    Some((SignableMessage::Vote(vote.clone()), safety_state.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(durable_safety.round, 0);
        assert_eq!(durable_safety.valid_round, None);
        assert!(durable_safety.valid_round_proof.is_empty());

        let decoded_safety: SafetyState =
            serde_json::from_slice(&serde_json::to_vec(&durable_safety).unwrap()).unwrap();
        let mut restored = Consensus::new_with_safety_state(
            local,
            validators,
            1,
            config(),
            decoded_safety,
            &DeterministicValidator,
        )
        .unwrap();
        restored.restore_local_decision(local_prevote).unwrap();
        restored
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        assert_eq!(restored.round(), 1);
        assert_eq!(restored.valid_value(), None);

        // Without a crash, entering round one re-evaluates the cached quorum
        // and promotes it at the point where round <= self.round is true.
        persist_all(&mut consensus, effects);
        let effects = consensus
            .step(
                Event::Timeout(Timeout {
                    height: 1,
                    round: 0,
                    phase: Phase::Prevote,
                    after: config().timeout_for(Phase::Prevote, 0),
                }),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut consensus, effects);
        advance_from_precommit(&mut consensus, 0);
        assert_eq!(consensus.round(), 1);
        assert_eq!(consensus.valid_value(), Some((1, future_block)));
        assert_eq!(consensus.locked(), Some((1, future_block)));
        let promoted = consensus.safety_state();
        assert_eq!(promoted.round, 1);
        assert_eq!(promoted.valid_round, Some(1));
        assert_eq!(promoted.valid_round_proof.len(), 3);
    }

    #[test]
    fn lock_rejects_new_value_without_newer_polc_then_accepts_proof() {
        let validators = four_validators();
        let mut consensus = Consensus::new(validator_id(3), validators, 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        let block_a = deterministic_block(1, 0);
        let block_b = BlockId([77; 32]);

        let proposal_effects = consensus
            .step(
                Event::Message(signed_proposal(0, block_a, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut consensus, proposal_effects);
        for index in 0..2 {
            let effects = consensus
                .step(
                    Event::Message(SignedMessage::Vote(signed_vote(
                        validator_id(index),
                        0,
                        VoteKind::Prevote,
                        VoteValue::Block(block_a),
                    ))),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut consensus, effects);
        }
        assert_eq!(consensus.locked(), Some((0, block_a)));
        assert_eq!(consensus.phase(), Phase::Precommit);

        advance_from_precommit(&mut consensus, 0);
        let effects = consensus
            .step(
                Event::Message(signed_proposal(1, block_b, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::PersistBeforeBroadcast {
                message: SignableMessage::Vote(Vote {
                    kind: VoteKind::Prevote,
                    value: VoteValue::Nil,
                    ..
                }),
                ..
            }
        )));
        persist_all(&mut consensus, effects);

        let timeout = Timeout {
            height: 1,
            round: 1,
            phase: Phase::Prevote,
            after: config().timeout_for(Phase::Prevote, 1),
        };
        let effects = consensus
            .step(Event::Timeout(timeout), &DeterministicValidator)
            .unwrap();
        persist_all(&mut consensus, effects);
        advance_from_precommit(&mut consensus, 1);

        let proof: Vec<_> = (0..3)
            .map(|index| {
                signed_vote(
                    validator_id(index),
                    1,
                    VoteKind::Prevote,
                    VoteValue::Block(block_b),
                )
            })
            .collect();
        let effects = consensus
            .step(
                Event::Message(signed_proposal(2, block_b, Some(1), proof)),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::PersistBeforeBroadcast {
                message: SignableMessage::Vote(Vote {
                    kind: VoteKind::Prevote,
                    value: VoteValue::Block(block),
                    ..
                }),
                ..
            } if *block == block_b
        )));
    }

    #[test]
    fn restart_restores_lock_and_reserves_prior_signer_slots() {
        let validators = four_validators();
        let local = validator_id(3);
        let mut consensus = Consensus::new(local, validators.clone(), 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        let block_a = deterministic_block(1, 0);
        let block_b = BlockId([88; 32]);

        let proposal_effects = consensus
            .step(
                Event::Message(signed_proposal(0, block_a, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        let local_prevote = proposal_effects
            .iter()
            .find_map(|effect| match effect {
                Effect::PersistBeforeBroadcast {
                    message: SignableMessage::Vote(vote),
                    ..
                } if vote.kind == VoteKind::Prevote => Some(SignableMessage::Vote(vote.clone())),
                _ => None,
            })
            .unwrap();
        persist_all(&mut consensus, proposal_effects);
        for index in 0..2 {
            let effects = consensus
                .step(
                    Event::Message(SignedMessage::Vote(signed_vote(
                        validator_id(index),
                        0,
                        VoteKind::Prevote,
                        VoteValue::Block(block_a),
                    ))),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut consensus, effects);
        }
        let safety_state = consensus.safety_state();
        assert_eq!(safety_state.locked_block, Some(block_a));

        let mut restored = Consensus::new_with_safety_state(
            local,
            validators,
            1,
            config(),
            safety_state,
            &DeterministicValidator,
        )
        .unwrap();
        restored
            .restore_local_decision(local_prevote.clone())
            .unwrap();
        restored.restore_local_decision(local_prevote).unwrap();
        restored
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        assert_eq!(restored.round(), 1);
        assert_eq!(restored.locked(), Some((0, block_a)));

        let effects = restored
            .step(
                Event::Message(signed_proposal(1, block_b, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::PersistBeforeBroadcast {
                message: SignableMessage::Vote(Vote {
                    kind: VoteKind::Prevote,
                    value: VoteValue::Nil,
                    ..
                }),
                ..
            }
        )));
    }

    #[test]
    fn legacy_safety_json_decodes_but_an_unproved_legacy_lock_fails_closed() {
        let legacy_unlocked: SafetyState = serde_json::from_str(
            r#"{"height":1,"round":0,"locked_round":null,"locked_block":null}"#,
        )
        .unwrap();
        assert_eq!(legacy_unlocked.valid_round, None);
        assert!(legacy_unlocked.valid_round_proof.is_empty());
        Consensus::new_with_safety_state(
            validator_id(0),
            four_validators(),
            1,
            config(),
            legacy_unlocked,
            &DeterministicValidator,
        )
        .unwrap();

        let mut legacy_locked = serde_json::to_value(SafetyState {
            height: 1,
            round: 0,
            locked_round: Some(0),
            locked_block: Some(deterministic_block(1, 0)),
            valid_round: Some(0),
            valid_block: Some(deterministic_block(1, 0)),
            valid_round_proof: (0..3)
                .map(|index| {
                    signed_vote(
                        validator_id(index),
                        0,
                        VoteKind::Prevote,
                        VoteValue::Block(deterministic_block(1, 0)),
                    )
                })
                .collect(),
        })
        .unwrap();
        let object = legacy_locked.as_object_mut().unwrap();
        object.remove("valid_round");
        object.remove("valid_block");
        object.remove("valid_round_proof");
        let decoded: SafetyState = serde_json::from_value(legacy_locked).unwrap();
        assert_eq!(
            Consensus::new_with_safety_state(
                validator_id(0),
                four_validators(),
                1,
                config(),
                decoded,
                &DeterministicValidator,
            )
            .err(),
            Some(ConsensusError::MalformedSafetyState)
        );
    }

    #[test]
    fn restart_preserves_newer_polc_and_breaks_asymmetric_lock_stall() {
        let validators = four_validators();
        let block_x = deterministic_block(1, 0);
        let block_y = BlockId([91; 32]);

        // H0 alone sees the round-zero quorum for X and locks it. H1 prevotes
        // X, but never receives the other two votes and therefore stays
        // unlocked. This delivery schedule is legal in an asynchronous
        // network even though all messages are correctly signed.
        let mut h0 = Consensus::new(validator_id(0), validators.clone(), 1, config()).unwrap();
        h0.step(Event::Start, &DeterministicValidator).unwrap();
        let effects = h0
            .step(
                Event::Message(signed_proposal(0, block_x, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut h0, effects);
        for index in [1, 3] {
            let effects = h0
                .step(
                    Event::Message(SignedMessage::Vote(signed_vote(
                        validator_id(index),
                        0,
                        VoteKind::Prevote,
                        VoteValue::Block(block_x),
                    ))),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut h0, effects);
        }
        assert_eq!(h0.locked(), Some((0, block_x)));

        let mut h1 = Consensus::new(validator_id(1), validators.clone(), 1, config()).unwrap();
        h1.step(Event::Start, &DeterministicValidator).unwrap();
        let effects = h1
            .step(
                Event::Message(signed_proposal(0, block_x, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut h1, effects);
        let effects = h1
            .step(
                Event::Timeout(Timeout {
                    height: 1,
                    round: 0,
                    phase: Phase::Prevote,
                    after: config().timeout_for(Phase::Prevote, 0),
                }),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut h1, effects);
        advance_from_precommit(&mut h1, 0);

        // In round one, H1/H2/B prevote Y. Only H1 sees that quorum, so H1
        // takes a newer lock while H0 keeps its older X lock and prevotes nil.
        advance_from_precommit(&mut h0, 0);
        let effects = h0
            .step(
                Event::Message(signed_proposal(1, block_y, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::PersistBeforeBroadcast {
                message: SignableMessage::Vote(Vote {
                    kind: VoteKind::Prevote,
                    value: VoteValue::Nil,
                    ..
                }),
                ..
            }
        )));
        persist_all(&mut h0, effects);

        let effects = h1
            .step(
                Event::Message(signed_proposal(1, block_y, None, Vec::new())),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut h1, effects);
        for index in [2, 3] {
            let effects = h1
                .step(
                    Event::Message(SignedMessage::Vote(signed_vote(
                        validator_id(index),
                        1,
                        VoteKind::Prevote,
                        VoteValue::Block(block_y),
                    ))),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut h1, effects);
        }
        assert_eq!(h1.locked(), Some((1, block_y)));
        let durable_safety = h1.safety_state();
        assert_eq!(durable_safety.valid_round, Some(1));
        assert_eq!(durable_safety.valid_block, Some(block_y));
        assert_eq!(durable_safety.valid_round_proof.len(), 3);

        // A corrupted durable PoLC fails closed during recovery.
        let mut forged_safety = durable_safety.clone();
        forged_safety.valid_round_proof[0].signature[0] ^= 1;
        assert_eq!(
            Consensus::new_with_safety_state(
                validator_id(1),
                validators.clone(),
                1,
                config(),
                forged_safety,
                &DeterministicValidator,
            )
            .err(),
            Some(ConsensusError::Validation(
                ValidationError::InvalidSignature
            ))
        );

        // H1 crashes after its precommit is durable. Validator B goes silent.
        // The restarted H1 retains Y's authenticated round-one PoLC.
        let mut restarted_h1 = Consensus::new_with_safety_state(
            validator_id(1),
            validators,
            1,
            config(),
            durable_safety,
            &DeterministicValidator,
        )
        .unwrap();
        restarted_h1
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        assert_eq!(restarted_h1.valid_value(), Some((1, block_y)));

        let effects = h0
            .step(
                Event::Timeout(Timeout {
                    height: 1,
                    round: 1,
                    phase: Phase::Prevote,
                    after: config().timeout_for(Phase::Prevote, 1),
                }),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut h0, effects);
        advance_from_precommit(&mut h0, 1);
        for round in 2..5 {
            timeout_through_round(&mut h0, round);
            timeout_through_round(&mut restarted_h1, round);
        }
        assert_eq!(h0.round(), 5);
        assert_eq!(restarted_h1.round(), 5);

        // H1's next proposer turn carries the recovered proof. It unlocks H0,
        // and H0/H1/H2 finalize Y even though B remains unavailable.
        let proposal_effects = restarted_h1
            .step(
                Event::ProposalReady {
                    height: 1,
                    round: 5,
                    block_id: block_y,
                },
                &DeterministicValidator,
            )
            .unwrap();
        let proposal = proposal_effects
            .iter()
            .find_map(|effect| match effect {
                Effect::PersistBeforeBroadcast {
                    message: SignableMessage::Proposal(proposal),
                    ..
                } => Some(proposal.clone()),
                _ => None,
            })
            .expect("round-five proposer includes the recovered valid value");
        assert_eq!(proposal.valid_round, Some(1));
        assert_eq!(proposal.valid_round_proof.len(), 3);
        let signed_proposal = sign(SignableMessage::Proposal(proposal));
        let effects = restarted_h1
            .step(
                Event::Persisted(signed_proposal.clone()),
                &DeterministicValidator,
            )
            .unwrap();
        persist_all(&mut restarted_h1, effects);

        let effects = h0
            .step(Event::Message(signed_proposal), &DeterministicValidator)
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::PersistBeforeBroadcast {
                message: SignableMessage::Vote(Vote {
                    kind: VoteKind::Prevote,
                    value: VoteValue::Block(block),
                    ..
                }),
                ..
            } if *block == block_y
        )));
        persist_all(&mut h0, effects);

        for vote in [
            signed_vote(
                validator_id(1),
                5,
                VoteKind::Prevote,
                VoteValue::Block(block_y),
            ),
            signed_vote(
                validator_id(2),
                5,
                VoteKind::Prevote,
                VoteValue::Block(block_y),
            ),
        ] {
            let effects = h0
                .step(
                    Event::Message(SignedMessage::Vote(vote)),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut h0, effects);
        }
        for vote in [
            signed_vote(
                validator_id(0),
                5,
                VoteKind::Prevote,
                VoteValue::Block(block_y),
            ),
            signed_vote(
                validator_id(2),
                5,
                VoteKind::Prevote,
                VoteValue::Block(block_y),
            ),
        ] {
            let effects = restarted_h1
                .step(
                    Event::Message(SignedMessage::Vote(vote)),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut restarted_h1, effects);
        }
        assert_eq!(h0.locked(), Some((5, block_y)));
        assert_eq!(restarted_h1.locked(), Some((5, block_y)));

        for vote in [
            signed_vote(
                validator_id(1),
                5,
                VoteKind::Precommit,
                VoteValue::Block(block_y),
            ),
            signed_vote(
                validator_id(2),
                5,
                VoteKind::Precommit,
                VoteValue::Block(block_y),
            ),
        ] {
            let effects = h0
                .step(
                    Event::Message(SignedMessage::Vote(vote)),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut h0, effects);
        }
        for vote in [
            signed_vote(
                validator_id(0),
                5,
                VoteKind::Precommit,
                VoteValue::Block(block_y),
            ),
            signed_vote(
                validator_id(2),
                5,
                VoteKind::Precommit,
                VoteValue::Block(block_y),
            ),
        ] {
            let effects = restarted_h1
                .step(
                    Event::Message(SignedMessage::Vote(vote)),
                    &DeterministicValidator,
                )
                .unwrap();
            persist_all(&mut restarted_h1, effects);
        }
        assert_eq!(h0.finalization().unwrap().block_id, block_y);
        assert_eq!(restarted_h1.finalization().unwrap().block_id, block_y);
    }

    #[test]
    fn late_commit_certificate_finalizes_after_round_advanced() {
        let validators = four_validators();
        let mut consensus = Consensus::new(validator_id(3), validators, 1, config()).unwrap();
        consensus
            .step(Event::Start, &DeterministicValidator)
            .unwrap();
        timeout_through_round(&mut consensus, 0);
        assert_eq!(consensus.round(), 1);

        let block = deterministic_block(1, 0);
        let certificate = CommitCertificate {
            chain_id: "kcoin-test".to_owned(),
            height: 1,
            round: 0,
            block_id: block,
            precommits: (0..3)
                .map(|index| {
                    signed_vote(
                        validator_id(index),
                        0,
                        VoteKind::Precommit,
                        VoteValue::Block(block),
                    )
                })
                .collect(),
        };
        let effects = consensus
            .step(Event::Certificate(certificate), &DeterministicValidator)
            .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::Finalize(_)]));
        assert_eq!(consensus.finalization().unwrap().block_id, block);
    }

    #[test]
    fn precommit_signatures_convert_to_verifiable_protocol_certificate() {
        let keys: Vec<_> = (1_u8..=4)
            .map(|byte| SigningKey::from_bytes(&[byte; 32]))
            .collect();
        let protocol_validators: Vec<_> = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect();
        let block_id = BlockId([55; 32]);
        let protocol_vote = ProtocolCommitVote::new(
            ProtocolChainId::new("kcoin-test").unwrap(),
            1,
            7,
            ProtocolHash32::from_bytes(block_id.0),
        );
        let mut precommits: Vec<_> = keys[..3]
            .iter()
            .zip(protocol_validators.iter())
            .map(|(key, validator)| {
                let vote = Vote {
                    chain_id: "kcoin-test".to_owned(),
                    height: 1,
                    round: 7,
                    kind: VoteKind::Precommit,
                    validator: (*validator).into(),
                    value: VoteValue::Block(block_id),
                };
                let signing_bytes = SignableMessage::Vote(vote.clone()).signing_bytes().unwrap();
                assert_eq!(signing_bytes, protocol_vote.signing_bytes());
                SignedVote {
                    vote,
                    signature: key.sign(&signing_bytes).to_bytes().to_vec(),
                }
            })
            .collect();
        precommits.sort_by_key(|precommit| precommit.vote.validator);
        let consensus_certificate = CommitCertificate {
            chain_id: "kcoin-test".to_owned(),
            height: 1,
            round: 7,
            block_id,
            precommits,
        };

        for precommit in &consensus_certificate.precommits {
            assert_eq!(crate::verify_ed25519_vote(precommit), Ok(()));
        }

        let protocol_certificate = consensus_certificate.to_protocol().unwrap();
        assert_eq!(protocol_certificate.verify(&protocol_validators), Ok(()));
        assert_eq!(
            CommitCertificate::from_protocol(&protocol_certificate),
            consensus_certificate
        );
    }

    #[test]
    fn one_equivocator_cannot_make_honest_nodes_finalize_conflicts() {
        let validators = four_validators();
        let mut network = VirtualNetwork::new(validators, 1, config()).unwrap();
        network.set_online(validator_id(0), false).unwrap();
        let majority = [validator_id(1), validator_id(2)];
        let minority = [validator_id(3)];
        network.partition(&[&majority, &minority]);
        network.start().unwrap();

        let block_a = BlockId([10; 32]);
        let block_b = BlockId([11; 32]);
        for recipient in majority {
            network
                .inject(recipient, signed_proposal(0, block_a, None, Vec::new()))
                .unwrap();
        }
        network
            .inject(
                validator_id(3),
                signed_proposal(0, block_b, None, Vec::new()),
            )
            .unwrap();
        for recipient in majority {
            network
                .inject(
                    recipient,
                    SignedMessage::Vote(signed_vote(
                        validator_id(0),
                        0,
                        VoteKind::Prevote,
                        VoteValue::Block(block_a),
                    )),
                )
                .unwrap();
        }
        network
            .inject(
                validator_id(3),
                SignedMessage::Vote(signed_vote(
                    validator_id(0),
                    0,
                    VoteKind::Prevote,
                    VoteValue::Block(block_b),
                )),
            )
            .unwrap();
        for recipient in majority {
            network
                .inject(
                    recipient,
                    SignedMessage::Vote(signed_vote(
                        validator_id(0),
                        0,
                        VoteKind::Precommit,
                        VoteValue::Block(block_a),
                    )),
                )
                .unwrap();
        }

        assert_eq!(network.finalizations().len(), 2);
        assert!(
            network
                .finalizations()
                .values()
                .all(|finalization| finalization.block_id == block_a)
        );
        assert!(
            network
                .node(validator_id(3))
                .unwrap()
                .finalization()
                .is_none()
        );

        let mut precommits = vec![signed_vote(
            validator_id(0),
            0,
            VoteKind::Precommit,
            VoteValue::Block(block_a),
        )];
        for honest in [validator_id(1), validator_id(2)] {
            let vote = network
                .broadcasts()
                .iter()
                .find_map(|(sender, message)| match message {
                    SignedMessage::Vote(vote)
                        if *sender == honest
                            && vote.vote.kind == VoteKind::Precommit
                            && vote.vote.value == VoteValue::Block(block_a) =>
                    {
                        Some(vote.clone())
                    }
                    _ => None,
                })
                .unwrap();
            precommits.push(vote);
        }
        network
            .deliver_certificate(
                validator_id(3),
                CommitCertificate {
                    chain_id: "kcoin-test".to_owned(),
                    height: 1,
                    round: 0,
                    block_id: block_a,
                    precommits,
                },
            )
            .unwrap();
        assert_eq!(
            network
                .node(validator_id(3))
                .unwrap()
                .finalization()
                .unwrap()
                .block_id,
            block_a
        );
    }

    fn persist_all(consensus: &mut Consensus, initial: Vec<Effect>) -> Vec<Effect> {
        let mut queue: VecDeque<_> = initial.into();
        let mut retained = Vec::new();
        while let Some(effect) = queue.pop_front() {
            if let Effect::PersistBeforeBroadcast { message, .. } = effect {
                let emitted = consensus
                    .step(Event::Persisted(sign(message)), &DeterministicValidator)
                    .unwrap();
                queue.extend(emitted);
            } else {
                retained.push(effect);
            }
        }
        retained
    }

    fn advance_from_precommit(consensus: &mut Consensus, round: u32) {
        let timeout = Timeout {
            height: 1,
            round,
            phase: Phase::Precommit,
            after: config().timeout_for(Phase::Precommit, round),
        };
        let effects = consensus
            .step(Event::Timeout(timeout), &DeterministicValidator)
            .unwrap();
        persist_all(consensus, effects);
    }

    fn timeout_through_round(consensus: &mut Consensus, round: u32) {
        for phase in [Phase::Propose, Phase::Prevote] {
            let timeout = Timeout {
                height: 1,
                round,
                phase,
                after: config().timeout_for(phase, round),
            };
            let effects = consensus
                .step(Event::Timeout(timeout), &DeterministicValidator)
                .unwrap();
            persist_all(consensus, effects);
        }
        advance_from_precommit(consensus, round);
    }
}
