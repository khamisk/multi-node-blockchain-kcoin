use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signer, SigningKey};
use kcoin_consensus::{
    BlockId as ConsensusBlockId, CommitCertificate as ConsensusCommitCertificate, Consensus,
    ConsensusConfig, Effect as ConsensusEffect, Event as ConsensusEvent, MessageValidator,
    Phase as ConsensusPhase, SafetyState, SignableMessage, SignedMessage, SignedProposal,
    SignedVote, Timeout as ConsensusTimeout, ValidationError as ConsensusValidationError,
    ValidatorId as ConsensusValidatorId, ValidatorSet, VoteKind, verify_ed25519_proposal,
    verify_ed25519_vote,
};
use kcoin_protocol::{
    Block, ChainId, CommitCertificate, CommitSignature, CommitVote, Hash32, LedgerState,
    MAX_SUPPLY_ATOMS, SignedTransaction, TransactionOutcome, TransactionReceipt, ValidationError,
    ValidatorId as ProtocolValidatorId, replay_finalized_blocks, reward_for_supply,
};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{error, info, warn};

use crate::{
    config::{NodeConfig, NodeRole},
    network::{
        GossipKind, MAX_SYNC_RESPONSE_BYTES, NetworkEvent, NetworkHandle, OutboundSyncIntent,
        OutboundSyncRequestId, SyncRequest, SyncResponse,
    },
    storage::{
        AccountProjection, BlockRow, FinalizedProjection, PersistedConsensusDecision,
        PersistedConsensusProposal, Store, TransactionProjection,
    },
};

const SYNC_REQUEST_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(12);
const VALIDATOR_SYNC_ANNOUNCEMENT_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct ValidatorView {
    pub id: String,
    pub name: String,
    pub index: u16,
    pub online: bool,
    pub phase: String,
    pub height: String,
    pub round: u32,
    pub block_hash: String,
    pub state_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_progress: Option<f32>,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeSnapshot {
    pub chain_id: String,
    pub protocol_version: u16,
    pub height: String,
    pub finalized_hash: String,
    pub state_root: String,
    pub circulating_supply_atoms: String,
    pub max_supply_atoms: String,
    pub mempool_size: usize,
    pub peer_count: usize,
    pub block_time_ms: u64,
    pub validators: Vec<ValidatorView>,
    pub syncing: bool,
    pub halted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ApiEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct SubmissionReceipt {
    pub transaction_id: Hash32,
}

enum Command {
    Submit {
        transaction: SignedTransaction,
        gossip: bool,
        response: oneshot::Sender<std::result::Result<SubmissionReceipt, ValidationError>>,
    },
    Tick,
    #[cfg(test)]
    SyncWatchdog {
        now: Instant,
    },
    ConsensusMessage {
        source: Option<PeerId>,
        message: ConsensusWireMessage,
    },
    ConsensusTimeout(ConsensusTimeout),
    ImportFinalized {
        record: FinalizedWireRecord,
        source: Option<PeerId>,
    },
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    SyncRequest {
        response_token: u64,
        request: SyncRequest,
    },
    SyncResponse {
        peer: PeerId,
        intent: OutboundSyncIntent,
        response: SyncResponse,
    },
    SyncFailure {
        peer: PeerId,
        intent: OutboundSyncIntent,
        error: String,
    },
    ValidatorStatus(SignedValidatorStatus),
    Shutdown,
}

#[derive(Clone)]
pub struct NodeHandle {
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<RuntimeSnapshot>,
    events: broadcast::Sender<ApiEvent>,
    store: Store,
}

impl NodeHandle {
    pub async fn submit(
        &self,
        transaction: SignedTransaction,
    ) -> std::result::Result<SubmissionReceipt, ValidationError> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(Command::Submit {
                transaction,
                gossip: true,
                response,
            })
            .await
            .map_err(|_| ValidationError::Malformed)?;
        receiver.await.map_err(|_| ValidationError::Malformed)?
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.borrow().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ApiEvent> {
        self.events.subscribe()
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown).await;
    }
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct FinalizedWireRecord {
    block: Block,
    certificate: CommitCertificate,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
enum ConsensusWireMessage {
    Proposal {
        signed: SignedProposal,
        block: Box<Block>,
    },
    Vote(SignedVote),
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct ValidatorStatusMessage {
    chain_id: String,
    validator: ConsensusValidatorId,
    index: u16,
    height: u64,
    round: u32,
    phase: String,
    block_hash: [u8; 32],
    state_root: [u8; 32],
    syncing: bool,
    sync_target: Option<u64>,
    timestamp_ms: u64,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct SignedValidatorStatus {
    status: ValidatorStatusMessage,
    signature: Vec<u8>,
}

#[derive(Clone, Debug)]
struct ValidatorTelemetry {
    height: u64,
    round: u32,
    phase: String,
    block_hash: Hash32,
    state_root: Hash32,
    sync_progress: Option<f32>,
    last_seen_ms: u64,
}

struct ConsensusRecovery {
    consensus: Option<Consensus>,
    proposal_blocks: BTreeMap<ConsensusBlockId, Block>,
    replay_messages: Vec<SignedMessage>,
    rebroadcast_wires: Vec<ConsensusWireMessage>,
}

fn initialize_consensus_recovery(
    config: &NodeConfig,
    store: &Store,
    ledger: &LedgerState,
    validator_ids: &[ProtocolValidatorId],
    consensus_ids: &[ConsensusValidatorId],
    signing_key: Option<&SigningKey>,
) -> Result<ConsensusRecovery> {
    let Some(signing_key) = signing_key else {
        return Ok(ConsensusRecovery {
            consensus: None,
            proposal_blocks: BTreeMap::new(),
            replay_messages: Vec::new(),
            rebroadcast_wires: Vec::new(),
        });
    };
    let local_id = ConsensusValidatorId::from(ProtocolValidatorId::from_signing_key(signing_key));
    let height = ledger.height() + 1;
    let validators = ValidatorSet::new(consensus_ids.to_vec())?;
    let consensus_config = ConsensusConfig::for_chain(config.chain_id.clone());
    let Some((stored_safety_height, safety_bytes)) = store.consensus_safety_state()? else {
        return Ok(ConsensusRecovery {
            consensus: Some(Consensus::new(
                local_id,
                validators,
                height,
                consensus_config,
            )?),
            proposal_blocks: BTreeMap::new(),
            replay_messages: Vec::new(),
            rebroadcast_wires: Vec::new(),
        });
    };

    if stored_safety_height != height {
        anyhow::bail!(
            "active consensus safety row belongs to height {stored_safety_height}, expected {height}"
        );
    }

    let safety_state: SafetyState = serde_json::from_slice(&safety_bytes)
        .context("active consensus safety state is malformed")?;
    if safety_state.height != height {
        anyhow::bail!(
            "active consensus safety state belongs to height {}, expected {}",
            safety_state.height,
            height
        );
    }
    validate_recovered_safety_state(&safety_state)?;

    let mut proposal_blocks = BTreeMap::new();
    let mut proposal_variants = BTreeMap::new();
    let mut proposal_messages = Vec::new();
    for persisted in store.consensus_proposals(height)? {
        let block_id_bytes: [u8; 32] = persisted
            .block_id
            .as_slice()
            .try_into()
            .context("persisted proposal block id must be 32 bytes")?;
        let block_id = ConsensusBlockId::new(block_id_bytes);
        let block = Block::decode(&persisted.block_bytes)
            .context("persisted proposal block bytes are invalid")?;
        let signed = SignedProposal::try_from_slice(&persisted.signed_proposal)
            .context("persisted signed proposal is malformed")?;
        verify_ed25519_proposal(&signed)
            .map_err(|error| anyhow::anyhow!(error))
            .context("persisted proposal signature is invalid")?;
        if persisted.height != height
            || persisted.round != signed.proposal.round
            || signed.proposal.height != height
            || signed.proposal.chain_id != config.chain_id
            || signed.proposal.block_id != block_id
            || ConsensusBlockId::from(block.consensus_hash()) != block_id
            || validators.proposer(height, signed.proposal.round) != signed.proposal.proposer
            || block.header.round > signed.proposal.round
            || expected_protocol_proposer(validator_ids, block.header.height, block.header.round)
                != Some(block.header.proposer)
        {
            anyhow::bail!("persisted proposal context is inconsistent");
        }
        if proposal_blocks
            .get(&block_id)
            .is_some_and(|existing| existing != &block)
        {
            anyhow::bail!("persisted proposal bytes conflict for one canonical block id");
        }
        proposal_variants.insert((block_id, signed.proposal.round), block.clone());
        proposal_blocks.insert(block_id, block);
        proposal_messages.push(SignedMessage::Proposal(signed));
    }
    if safety_state
        .locked_block
        .is_some_and(|block_id| !proposal_blocks.contains_key(&block_id))
    {
        anyhow::bail!("restored lock is missing its persisted proposal block");
    }
    if safety_state
        .valid_block
        .is_some_and(|block_id| !proposal_blocks.contains_key(&block_id))
    {
        anyhow::bail!("restored valid value is missing its persisted proposal block");
    }
    let application_validator = NodeMessageValidator {
        ledger: ledger.clone(),
        blocks: proposal_blocks.clone(),
        validators: validator_ids.to_vec(),
    };
    for message in &proposal_messages {
        if let SignedMessage::Proposal(proposal) = message {
            application_validator
                .validate_proposal(proposal)
                .map_err(|error| anyhow::anyhow!(error))
                .context("persisted proposal is not application-valid")?;
        }
    }
    let mut consensus = Consensus::new_with_safety_state(
        local_id,
        validators.clone(),
        height,
        consensus_config,
        safety_state.clone(),
        &application_validator,
    )?;

    let decisions = store.consensus_decisions(height)?;
    if decisions.is_empty() {
        anyhow::bail!("active consensus safety state has no durable signing decision");
    }
    if decisions
        .last()
        .is_none_or(|decision| decision.safety_state != safety_bytes)
    {
        anyhow::bail!("active consensus safety state does not match the latest decision");
    }
    let mut local_messages = Vec::new();
    for decision in &decisions {
        let signed = validate_persisted_decision(decision, height)?;
        consensus
            .restore_local_decision(signed.signable())
            .context("persisted local decision conflicts with restored consensus")?;
        local_messages.push(signed);
    }
    let mut replay_messages = proposal_messages;
    for message in &local_messages {
        if !replay_messages.contains(message) {
            replay_messages.push(message.clone());
        }
    }
    let rebroadcast_wires = local_messages
        .iter()
        .map(|message| match message {
            SignedMessage::Proposal(signed) => {
                let block = proposal_variants
                    .get(&(signed.proposal.block_id, signed.proposal.round))
                    .cloned()
                    .context("durable local proposal is missing its exact block variant")?;
                Ok(ConsensusWireMessage::Proposal {
                    signed: signed.clone(),
                    block: Box::new(block),
                })
            }
            SignedMessage::Vote(vote) => Ok(ConsensusWireMessage::Vote(vote.clone())),
        })
        .collect::<Result<Vec<_>>>()?;
    info!(
        height,
        resume_round = consensus.round(),
        decisions = local_messages.len(),
        "validated interrupted consensus state for restart"
    );
    Ok(ConsensusRecovery {
        consensus: Some(consensus),
        proposal_blocks,
        replay_messages,
        rebroadcast_wires,
    })
}

fn validate_recovered_safety_state(safety_state: &SafetyState) -> Result<()> {
    if safety_state.locked_round.is_some() != safety_state.locked_block.is_some()
        || safety_state
            .locked_round
            .is_some_and(|locked_round| locked_round > safety_state.round)
    {
        anyhow::bail!("persisted consensus safety state has an invalid lock");
    }
    Ok(())
}

fn validate_persisted_decision(
    persisted: &PersistedConsensusDecision,
    height: u64,
) -> Result<SignedMessage> {
    let signed = SignedMessage::try_from_slice(&persisted.signed_message)
        .context("persisted signed consensus message is malformed")?;
    match &signed {
        SignedMessage::Proposal(proposal) => verify_ed25519_proposal(proposal),
        SignedMessage::Vote(vote) => verify_ed25519_vote(vote),
    }
    .map_err(|error| anyhow::anyhow!(error))
    .context("persisted consensus signature is invalid")?;
    if signed.signature() != persisted.signature
        || signed.signing_bytes()? != persisted.sign_bytes
        || signer_slot(&signed.signable()) != persisted.slot
    {
        anyhow::bail!("persisted consensus signing record is internally inconsistent");
    }
    let decision_safety: SafetyState = serde_json::from_slice(&persisted.safety_state)
        .context("persisted decision safety state is malformed")?;
    if decision_safety.height != height {
        anyhow::bail!("persisted decision belongs to a stale or future height");
    }
    validate_recovered_safety_state(&decision_safety)?;
    Ok(signed)
}

pub async fn start_node(
    config: NodeConfig,
    store: Store,
    network: Option<NetworkHandle>,
) -> Result<NodeHandle> {
    let chain_id = ChainId::new(config.chain_id.clone()).context("invalid configured chain id")?;
    if matches!(config.role, NodeRole::Validator | NodeRole::Observer)
        && !config.demo
        && !config.chain_id.contains("local")
        && !cfg!(test)
    {
        anyhow::bail!(
            "the fixed deterministic validator set is restricted to --demo or a local chain id"
        );
    }

    let validator_ids = deterministic_dev_validator_keys()
        .iter()
        .map(ProtocolValidatorId::from_signing_key)
        .collect::<Vec<_>>();
    let history = store
        .canonical_block_rows()?
        .into_iter()
        .map(|row| {
            Ok((
                Block::decode(&row.block_bytes)?,
                CommitCertificate::decode(&row.certificate_bytes)?,
            ))
        })
        .collect::<std::result::Result<Vec<_>, ValidationError>>()
        .context("decode persisted finalized history")?;
    let ledger = replay_finalized_blocks(chain_id.clone(), &history, &validator_ids)
        .context("verify and replay persisted finalized history")?;
    let consensus_ids = validator_ids
        .iter()
        .copied()
        .map(ConsensusValidatorId::from)
        .collect::<Vec<_>>();
    let local_validator_key = if config.role == NodeRole::Validator {
        let index = usize::from(
            config
                .validator_index
                .context("validator index is required")?,
        );
        Some(
            deterministic_dev_validator_keys()
                .into_iter()
                .nth(index)
                .context("validator index is outside the fixed validator set")?,
        )
    } else {
        None
    };
    let recovery = initialize_consensus_recovery(
        &config,
        &store,
        &ledger,
        &validator_ids,
        &consensus_ids,
        local_validator_key.as_ref(),
    )?;

    // Subscribe before reading the persistent peer snapshot. A connection
    // racing this boundary is then represented either in the snapshot, the
    // queued event, or both (the actor's HashSet makes the update idempotent).
    let network_events = network.as_ref().map(NetworkHandle::subscribe);
    let initial_peers = network
        .as_ref()
        .map(NetworkHandle::connected_peers)
        .unwrap_or_default();

    let empty_telemetry = BTreeMap::new();
    let snapshot = snapshot_for(SnapshotInputs {
        config: &config,
        ledger: &ledger,
        mempool_size: 0,
        peer_count: initial_peers.len(),
        sync_target: None,
        halted: false,
        validators: &validator_ids,
        telemetry: &empty_telemetry,
    });
    let (snapshot_tx, snapshot_rx) = watch::channel(snapshot);
    let (events, _) = broadcast::channel(1_024);
    let (commands, command_rx) = mpsc::channel(1_024);

    let actor = NodeActor {
        config: config.clone(),
        chain_id,
        ledger,
        mempool: VecDeque::new(),
        mempool_ids: HashSet::new(),
        store: store.clone(),
        network: network.clone(),
        validator_ids,
        consensus_ids,
        standalone_keys: if config.role == NodeRole::Standalone {
            deterministic_dev_validator_keys()
        } else {
            Vec::new()
        },
        local_validator_key,
        consensus: recovery.consensus,
        consensus_started: false,
        proposal_blocks: recovery.proposal_blocks,
        recovery_messages: recovery.replay_messages,
        recovery_broadcasts: recovery.rebroadcast_wires,
        commands: command_rx,
        command_tx: commands.clone(),
        snapshots: snapshot_tx,
        events: events.clone(),
        peers: initial_peers,
        validator_telemetry: BTreeMap::new(),
        sync_target: None,
        sync_peer: None,
        sync_request_id: None,
        sync_request_deadline: None,
        halted: false,
        last_block_at: Instant::now(),
        last_status_at: Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now),
        last_block_duration_ms: config.heartbeat_ms,
    };
    tokio::spawn(actor.run());

    let ticker_commands = commands.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;
            if ticker_commands.send(Command::Tick).await.is_err() {
                break;
            }
        }
    });

    if let Some(events) = network_events {
        spawn_network_bridge(config.chain_id.clone(), events, commands.clone());
    }

    info!(
        role = ?config.role,
        height = snapshot_rx.borrow().height,
        "node ledger actor started"
    );
    Ok(NodeHandle {
        commands,
        snapshot: snapshot_rx,
        events,
        store,
    })
}

struct NodeActor {
    config: NodeConfig,
    chain_id: ChainId,
    ledger: LedgerState,
    mempool: VecDeque<SignedTransaction>,
    mempool_ids: HashSet<Hash32>,
    store: Store,
    network: Option<NetworkHandle>,
    validator_ids: Vec<ProtocolValidatorId>,
    consensus_ids: Vec<ConsensusValidatorId>,
    standalone_keys: Vec<SigningKey>,
    local_validator_key: Option<SigningKey>,
    consensus: Option<Consensus>,
    consensus_started: bool,
    proposal_blocks: BTreeMap<ConsensusBlockId, Block>,
    recovery_messages: Vec<SignedMessage>,
    recovery_broadcasts: Vec<ConsensusWireMessage>,
    commands: mpsc::Receiver<Command>,
    command_tx: mpsc::Sender<Command>,
    snapshots: watch::Sender<RuntimeSnapshot>,
    events: broadcast::Sender<ApiEvent>,
    peers: HashSet<PeerId>,
    validator_telemetry: BTreeMap<ConsensusValidatorId, ValidatorTelemetry>,
    sync_target: Option<u64>,
    sync_peer: Option<PeerId>,
    sync_request_id: Option<OutboundSyncRequestId>,
    sync_request_deadline: Option<Instant>,
    halted: bool,
    last_block_at: Instant,
    last_status_at: Instant,
    last_block_duration_ms: u64,
}

impl NodeActor {
    async fn run(mut self) {
        if !self.recovery_messages.is_empty()
            && let Err(error) = self.resume_interrupted_consensus().await
        {
            error!(%error, "failed closed while resuming durable consensus state");
            self.enter_halted_state();
            self.publish_snapshot();
        }
        while let Some(command) = self.commands.recv().await {
            match command {
                Command::Submit {
                    transaction,
                    gossip,
                    response,
                } => {
                    let result = self.accept_transaction(transaction, gossip).await;
                    let _ = response.send(result);
                }
                Command::Tick => {
                    if let Err(error) = self.tick().await {
                        error!(%error, "node tick failed");
                    }
                }
                #[cfg(test)]
                Command::SyncWatchdog { now } => {
                    if let Err(error) = self.retry_timed_out_sync_request(now).await {
                        warn!(%error, "sync request watchdog retry failed");
                    }
                }
                Command::ConsensusMessage { source, message } => {
                    if let Err(error) = self.handle_consensus_message(source, message).await {
                        warn!(%error, "rejected consensus message");
                    }
                }
                Command::ConsensusTimeout(timeout) => {
                    if !self.consensus_started {
                        continue;
                    }
                    if let Err(error) = self
                        .process_consensus_event(ConsensusEvent::Timeout(timeout))
                        .await
                    {
                        warn!(%error, "consensus timeout failed");
                    }
                }
                Command::ImportFinalized { record, source } => {
                    if let Err(error) = self.import_finalized(record, source).await {
                        warn!(%error, "rejected finalized block from peer");
                    }
                }
                Command::PeerConnected(peer) => {
                    self.peers.insert(peer);
                    if self.sync_target.is_some() {
                        if let Err(error) = self.ensure_active_sync_request(peer).await {
                            warn!(%peer, %error, "failed to request blocks from connected peer");
                        }
                    } else if let Some(network) = &self.network {
                        let _ = network.request(peer, SyncRequest::Status).await;
                    }
                    self.publish_snapshot();
                }
                Command::PeerDisconnected(peer) => {
                    self.peers.remove(&peer);
                    if self.sync_peer == Some(peer)
                        && let Err(error) = self.retry_sync_after_failure(peer, None).await
                    {
                        warn!(%error, "failed to rotate sync peer after disconnect");
                    }
                    self.publish_snapshot();
                }
                Command::SyncRequest {
                    response_token,
                    request,
                } => {
                    if let Err(error) = self.respond_to_sync(response_token, request).await {
                        warn!(%error, "failed to serve sync request");
                    }
                }
                Command::SyncResponse {
                    peer,
                    intent,
                    response,
                } => {
                    let failed_peer = intent.peer;
                    let failed_request_id = intent.request_id;
                    if let Err(error) = self.handle_sync_response(peer, intent, response).await {
                        warn!(%peer, ?failed_request_id, %error, "rejected sync response");
                        if let Err(retry_error) = self
                            .retry_sync_after_failure(failed_peer, Some(failed_request_id))
                            .await
                        {
                            warn!(%retry_error, "failed to rotate sync peer");
                        }
                    }
                }
                Command::SyncFailure {
                    peer,
                    intent,
                    error,
                } => {
                    warn!(%peer, ?intent.request_id, %error, "sync transport failed");
                    if matches!(intent.request, SyncRequest::Blocks { .. })
                        && let Err(retry_error) = self
                            .retry_sync_after_failure(intent.peer, Some(intent.request_id))
                            .await
                    {
                        warn!(%retry_error, "failed to rotate sync peer");
                    }
                }
                Command::ValidatorStatus(status) => {
                    if let Err(error) = self.accept_validator_status(status) {
                        warn!(%error, "ignored invalid validator status");
                    }
                }
                Command::Shutdown => break,
            }
        }
    }

    async fn resume_interrupted_consensus(&mut self) -> Result<()> {
        // `restore_local_decision` already rebuilt the engine's signer-slot
        // memory. Re-gossip exact old messages while their original proposal
        // variant is still loaded, then start the next round; feeding an old
        // proposal as a fresh event after reproposal would pair it with the
        // later round's canonical block variant.
        self.recovery_messages.clear();
        let rebroadcast_wires = std::mem::take(&mut self.recovery_broadcasts);
        for wire in rebroadcast_wires {
            self.publish_consensus_wire(wire).await?;
        }
        self.consensus_started = true;
        self.process_consensus_event(ConsensusEvent::Start).await?;
        Ok(())
    }

    async fn accept_transaction(
        &mut self,
        transaction: SignedTransaction,
        gossip: bool,
    ) -> std::result::Result<SubmissionReceipt, ValidationError> {
        if self.halted || self.sync_target.is_some() {
            return Err(ValidationError::Malformed);
        }
        let id = transaction.id();
        if self.mempool_ids.contains(&id) || self.ledger.contains_transaction(&id) {
            return Err(ValidationError::DuplicateTransaction);
        }

        let execution_height = self
            .ledger
            .height()
            .checked_add(1)
            .ok_or(ValidationError::ArithmeticOverflow)?;
        let mut candidate = self.ledger.clone();
        for pending in &self.mempool {
            candidate.apply_transaction(pending, execution_height)?;
        }
        candidate.validate_transaction(&transaction, execution_height)?;

        self.mempool_ids.insert(id);
        self.mempool.push_back(transaction.clone());
        self.publish_snapshot();
        let _ = self.events.send(ApiEvent {
            event_type: "transaction".into(),
            id: id.to_string(),
        });

        if gossip && let Some(network) = self.network.clone() {
            let chain_id = self.chain_id.to_string();
            let bytes = transaction.canonical_bytes();
            tokio::spawn(async move {
                if let Err(error) = network.publish_transaction(&chain_id, bytes).await {
                    warn!(%error, "transaction accepted locally but gossip failed");
                }
            });
        }
        // Wake the producer immediately instead of waiting for the next ticker.
        let _ = self.command_tx.try_send(Command::Tick);
        Ok(SubmissionReceipt { transaction_id: id })
    }

    async fn tick(&mut self) -> Result<()> {
        self.retry_timed_out_sync_request(Instant::now()).await?;
        if self.last_status_at.elapsed() >= Duration::from_secs(1) {
            self.last_status_at = Instant::now();
            self.publish_validator_status().await?;
        }
        if self.halted {
            return Ok(());
        }
        let heartbeat_due =
            self.last_block_at.elapsed() >= Duration::from_millis(self.config.heartbeat_ms);
        if self.mempool.is_empty() && !heartbeat_due {
            return Ok(());
        }
        match self.config.role {
            NodeRole::Standalone => self.produce_standalone().await,
            NodeRole::Validator if self.sync_target.is_none() => {
                self.ensure_consensus_started().await
            }
            NodeRole::Validator | NodeRole::Observer => Ok(()),
        }
    }

    async fn produce_standalone(&mut self) -> Result<()> {
        let transactions = self.mempool.iter().take(1_000).cloned().collect::<Vec<_>>();
        let proposer_index = self.ledger.height() as usize % self.validator_ids.len();
        let timestamp_ms = unix_timestamp_ms();
        let block = self
            .ledger
            .build_block(
                self.validator_ids[proposer_index],
                0,
                timestamp_ms,
                transactions,
            )
            .context("build standalone proposal")?;
        let vote = CommitVote::new(
            self.chain_id.clone(),
            block.header.height,
            block.header.round,
            block.consensus_hash(),
        );
        let signatures = self.standalone_keys[..3]
            .iter()
            .map(|key| CommitSignature::sign(&vote, key))
            .collect();
        let certificate = CommitCertificate::new(vote, signatures);
        certificate
            .verify_for_block(&block, &self.validator_ids)
            .context("verify local standalone certificate")?;
        self.commit_finalized(block, certificate, true).await
    }

    async fn ensure_consensus_started(&mut self) -> Result<()> {
        if self.config.role != NodeRole::Validator
            || self.consensus_started
            || self.sync_target.is_some()
            || self.halted
        {
            return Ok(());
        }
        self.consensus_started = true;
        self.process_consensus_event(ConsensusEvent::Start).await
    }

    async fn handle_consensus_message(
        &mut self,
        _source: Option<PeerId>,
        wire: ConsensusWireMessage,
    ) -> Result<()> {
        if self.config.role != NodeRole::Validator || self.sync_target.is_some() || self.halted {
            return Ok(());
        }
        let (height, signer, phase, message) = match wire {
            ConsensusWireMessage::Proposal { signed, block } => {
                if ConsensusBlockId::from(block.consensus_hash()) != signed.proposal.block_id {
                    anyhow::bail!("proposal block bytes do not match its signed block id");
                }
                let height = signed.proposal.height;
                let signer = signed.proposal.proposer;
                let expected = self.ledger.height() + 1;
                if height < expected {
                    return Ok(());
                }
                if height > expected {
                    // A proposal is not proof that the preceding height was
                    // finalized. Catch-up begins only after a verified commit
                    // certificate arrives through finalized gossip.
                    return Ok(());
                }
                let block = *block;
                if self
                    .proposal_blocks
                    .get(&signed.proposal.block_id)
                    .is_some_and(|existing| existing != &block)
                {
                    anyhow::bail!("proposal bytes conflict for one canonical block id");
                }
                let mut candidate_blocks = self.proposal_blocks.clone();
                candidate_blocks.insert(signed.proposal.block_id, block.clone());
                let message = SignedMessage::Proposal(signed.clone());
                let validator = NodeMessageValidator {
                    ledger: self.ledger.clone(),
                    blocks: candidate_blocks,
                    validators: self.validator_ids.clone(),
                };
                let cacheable = self
                    .consensus
                    .as_ref()
                    .context("validator consensus driver is missing")?
                    .preflight_message(&message, &validator)
                    .context("proposal failed structural preflight")?;
                if !cacheable {
                    return Ok(());
                }
                let persisted = PersistedConsensusProposal {
                    height,
                    round: signed.proposal.round,
                    block_id: signed.proposal.block_id.0.to_vec(),
                    block_bytes: block.canonical_bytes(),
                    signed_proposal: borsh::to_vec(&signed)?,
                };
                self.store.persist_consensus_proposal(&persisted)?;
                self.proposal_blocks.insert(signed.proposal.block_id, block);
                (height, signer, "proposal", message)
            }
            ConsensusWireMessage::Vote(signed) => {
                let phase = match signed.vote.kind {
                    VoteKind::Prevote => "prevote",
                    VoteKind::Precommit => "precommit",
                };
                (
                    signed.vote.height,
                    signed.vote.validator,
                    phase,
                    SignedMessage::Vote(signed),
                )
            }
        };
        let expected = self.ledger.height() + 1;
        if height < expected {
            return Ok(());
        }
        if height > expected {
            // Votes and proposals are uncommitted height hints. They must not
            // move a validator into non-voting Syncing mode.
            return Ok(());
        }
        self.ensure_consensus_started().await?;
        self.process_consensus_event(ConsensusEvent::Message(message))
            .await?;
        self.note_validator_activity(signer, height, phase);
        Ok(())
    }

    async fn process_consensus_event(&mut self, initial: ConsensusEvent) -> Result<()> {
        if self.config.role != NodeRole::Validator || self.sync_target.is_some() || self.halted {
            return Ok(());
        }
        let mut pending_events = VecDeque::from([initial]);
        while let Some(event) = pending_events.pop_front() {
            let validator = NodeMessageValidator {
                ledger: self.ledger.clone(),
                blocks: self.proposal_blocks.clone(),
                validators: self.validator_ids.clone(),
            };
            let effects = self
                .consensus
                .as_mut()
                .context("validator consensus driver is missing")?
                .step(event, &validator)
                .context("consensus state transition failed")?;
            for effect in effects {
                match effect {
                    ConsensusEffect::RequestProposal {
                        height,
                        round,
                        valid_block,
                        ..
                    } => {
                        let locked_block = self
                            .consensus
                            .as_ref()
                            .and_then(Consensus::locked)
                            .map(|(_, block_id)| block_id);
                        let block =
                            self.build_consensus_proposal(round, valid_block.or(locked_block))?;
                        let block_id = ConsensusBlockId::from(block.consensus_hash());
                        self.proposal_blocks.insert(block_id, block);
                        pending_events.push_back(ConsensusEvent::ProposalReady {
                            height,
                            round,
                            block_id,
                        });
                    }
                    ConsensusEffect::PersistBeforeBroadcast {
                        message,
                        safety_state,
                    } => {
                        let signed = self.sign_and_persist(message, safety_state)?;
                        pending_events.push_back(ConsensusEvent::Persisted(signed));
                    }
                    ConsensusEffect::Broadcast(message) => {
                        self.broadcast_consensus(message).await?;
                    }
                    ConsensusEffect::ScheduleTimeout(timeout) => {
                        let commands = self.command_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(timeout.after).await;
                            let _ = commands.send(Command::ConsensusTimeout(timeout)).await;
                        });
                    }
                    ConsensusEffect::Finalize(finalization) => {
                        let block = self
                            .proposal_blocks
                            .get(&finalization.block_id)
                            .cloned()
                            .context("finalized proposal bytes are unavailable")?;
                        let certificate = finalization
                            .certificate
                            .to_protocol()
                            .context("convert consensus precommits to protocol certificate")?;
                        certificate
                            .verify_for_block(&block, &self.validator_ids)
                            .context("verify protocol-compatible consensus certificate")?;
                        self.commit_finalized(block, certificate, true).await?;
                        self.reset_consensus()?;
                    }
                    ConsensusEffect::Evidence(evidence) => {
                        self.store.append_consensus_event(
                            "evidence",
                            format!("{evidence:?}").as_bytes(),
                        )?;
                    }
                }
            }
        }
        self.publish_snapshot();
        Ok(())
    }

    fn build_consensus_proposal(
        &self,
        round: u32,
        valid_block: Option<ConsensusBlockId>,
    ) -> Result<Block> {
        if let Some(block_id) = valid_block
            && let Some(block) = self.proposal_blocks.get(&block_id)
        {
            // Tendermint-style valid-value reproposal carries the original
            // canonical bytes unchanged. The signed proposal and eventual
            // certificate record this later round separately.
            return Ok(block.clone());
        }
        let proposer = self
            .local_validator_key
            .as_ref()
            .map(ProtocolValidatorId::from_signing_key)
            .context("validator signing key is missing")?;
        let execution_height = self.ledger.height() + 1;
        let mut candidate = self.ledger.clone();
        let mut transactions = Vec::new();
        for transaction in self.mempool.iter().take(1_000) {
            if candidate
                .apply_transaction(transaction, execution_height)
                .is_ok()
            {
                transactions.push(transaction.clone());
            }
        }
        self.ledger
            .build_block(proposer, round, unix_timestamp_ms(), transactions)
            .context("build consensus proposal")
    }

    fn sign_and_persist(
        &self,
        message: SignableMessage,
        safety_state: SafetyState,
    ) -> Result<SignedMessage> {
        let signing_key = self
            .local_validator_key
            .as_ref()
            .context("validator signing key is missing")?;
        let local_id =
            ConsensusValidatorId::from(ProtocolValidatorId::from_signing_key(signing_key));
        let signer = match &message {
            SignableMessage::Proposal(proposal) => proposal.proposer,
            SignableMessage::Vote(vote) => vote.validator,
        };
        if signer != local_id {
            anyhow::bail!("consensus requested a signature for another validator");
        }
        let sign_bytes = message.signing_bytes()?;
        let generated = signing_key.sign(&sign_bytes).to_bytes().to_vec();
        let safety_bytes = serde_json::to_vec(&safety_state)?;
        let slot = signer_slot(&message);
        let provisional = signed_message(message.clone(), generated.clone());
        let encoded_message = borsh::to_vec(&provisional)?;
        if let SignedMessage::Proposal(signed) = &provisional {
            let block = self
                .proposal_blocks
                .get(&signed.proposal.block_id)
                .context("local proposal block bytes are unavailable")?;
            self.store
                .persist_consensus_proposal(&PersistedConsensusProposal {
                    height: signed.proposal.height,
                    round: signed.proposal.round,
                    block_id: signed.proposal.block_id.0.to_vec(),
                    block_bytes: block.canonical_bytes(),
                    signed_proposal: borsh::to_vec(signed)?,
                })?;
        }
        let signature = self.store.persist_consensus_decision(
            &slot,
            &sign_bytes,
            &generated,
            &safety_bytes,
            &encoded_message,
        )?;
        let signed = signed_message(message, signature);
        self.store
            .append_consensus_event("signed", &borsh::to_vec(&signed)?)?;
        Ok(signed)
    }

    async fn broadcast_consensus(&self, message: SignedMessage) -> Result<()> {
        let wire = match message {
            SignedMessage::Proposal(signed) => {
                let block = self
                    .proposal_blocks
                    .get(&signed.proposal.block_id)
                    .cloned()
                    .context("proposal bytes are unavailable for broadcast")?;
                ConsensusWireMessage::Proposal {
                    signed,
                    block: Box::new(block),
                }
            }
            SignedMessage::Vote(vote) => ConsensusWireMessage::Vote(vote),
        };
        self.publish_consensus_wire(wire).await
    }

    async fn publish_consensus_wire(&self, wire: ConsensusWireMessage) -> Result<()> {
        let Some(network) = &self.network else {
            return Ok(());
        };
        if let Err(error) = network
            .publish_consensus(&self.config.chain_id, borsh::to_vec(&wire)?)
            .await
        {
            warn!(%error, "durable consensus message could not be gossiped");
        }
        Ok(())
    }

    fn reset_consensus(&mut self) -> Result<()> {
        self.proposal_blocks.clear();
        self.recovery_messages.clear();
        self.recovery_broadcasts.clear();
        self.consensus_started = false;
        if let Some(key) = &self.local_validator_key {
            self.consensus = Some(Consensus::new(
                ConsensusValidatorId::from(ProtocolValidatorId::from_signing_key(key)),
                ValidatorSet::new(self.consensus_ids.clone())?,
                self.ledger.height() + 1,
                ConsensusConfig::for_chain(self.config.chain_id.clone()),
            )?);
        }
        Ok(())
    }

    async fn import_finalized(
        &mut self,
        record: FinalizedWireRecord,
        source: Option<PeerId>,
    ) -> Result<()> {
        if self.halted {
            anyhow::bail!("halted node refuses finalized-state mutation");
        }
        if record.block.header.chain_id != self.chain_id
            || record.certificate.chain_id != self.chain_id
        {
            anyhow::bail!("finalized block proof belongs to another chain");
        }
        record
            .block
            .validate_commitments()
            .context("validate finalized block commitments")?;
        record
            .certificate
            .verify_for_block(&record.block, &self.validator_ids)
            .context("verify peer commit certificate")?;
        if record.block.header.height <= self.ledger.height() {
            let existing = self
                .store
                .block_by_height(record.block.header.height)?
                .context("local finalized height is missing from storage")?;
            let canonical_existing = Block::decode(&existing.block_bytes)
                .context("decode existing canonical finalized block")?;
            if record.block.hash() != canonical_existing.hash() {
                self.enter_halted_state();
                if let Some(key) = &self.local_validator_key {
                    self.note_validator_activity(
                        ConsensusValidatorId::from(ProtocolValidatorId::from_signing_key(key)),
                        self.ledger.height(),
                        "halted",
                    );
                }
                self.publish_snapshot();
                anyhow::bail!(
                    "conflicting certified block at height {}",
                    record.block.header.height
                );
            }
            return Ok(());
        }
        if record.block.header.height != self.ledger.height() + 1 {
            if let Some(peer) = source {
                self.begin_sync(peer, record.block.header.height).await?;
                return Ok(());
            }
            anyhow::bail!("block gap requires range synchronization");
        }
        self.commit_finalized(record.block, record.certificate, false)
            .await?;
        self.reset_consensus()?;
        self.finish_sync_if_current();
        Ok(())
    }

    async fn commit_finalized(
        &mut self,
        block: Block,
        certificate: CommitCertificate,
        gossip: bool,
    ) -> Result<()> {
        let before = Instant::now();
        let mut candidate = self.ledger.clone();
        let receipts = candidate
            .apply_finalized_block(&block, &certificate, &self.validator_ids)
            .context("execute finalized block")?;
        let projection = self.project_block(&candidate, &block, &certificate, &receipts)?;
        self.store
            .persist_finalized(&projection)
            .context("atomically persist finalized block")?;
        self.ledger = candidate;

        self.revalidate_mempool();

        self.last_block_duration_ms = before.elapsed().as_millis().max(1) as u64;
        self.last_block_at = Instant::now();
        for signature in &certificate.signatures {
            self.note_validator_activity(
                ConsensusValidatorId::from(signature.validator),
                block.header.height,
                "finalized",
            );
        }
        self.publish_snapshot();

        let block_id = block.hash().to_string();
        let _ = self.events.send(ApiEvent {
            event_type: "finalized_block".into(),
            id: block_id.clone(),
        });
        for transaction in &block.transactions {
            let _ = self.events.send(ApiEvent {
                event_type: "transaction".into(),
                id: transaction.id().to_string(),
            });
        }

        if gossip && let Some(network) = self.network.clone() {
            let chain_id = self.chain_id.to_string();
            let bytes = borsh::to_vec(&FinalizedWireRecord { block, certificate })?;
            tokio::spawn(async move {
                if let Err(error) = network.publish_finalized(&chain_id, bytes).await {
                    warn!(%error, "finalized block persisted but gossip failed");
                }
            });
        }
        info!(height = self.ledger.height(), block_hash = %block_id, "block durably finalized");
        Ok(())
    }

    fn revalidate_mempool(&mut self) {
        let execution_height = self.ledger.height().saturating_add(1);
        let mut candidate = self.ledger.clone();
        let mut kept = VecDeque::new();
        let mut ids = HashSet::new();
        while let Some(transaction) = self.mempool.pop_front() {
            if candidate
                .apply_transaction(&transaction, execution_height)
                .is_ok()
            {
                ids.insert(transaction.id());
                kept.push_back(transaction);
            }
        }
        self.mempool = kept;
        self.mempool_ids = ids;
    }

    async fn begin_sync(&mut self, peer: PeerId, target: u64) -> Result<()> {
        if self.halted {
            anyhow::bail!("halted node refuses to enter synchronization");
        }
        if target <= self.ledger.height() {
            return Ok(());
        }
        self.sync_target = Some(self.sync_target.map_or(target, |known| known.max(target)));
        self.consensus_started = false;
        self.publish_snapshot();
        // Emit the non-voting recovery state before requesting blocks so an
        // observer can display the real offline -> syncing -> current path,
        // even when a short local catch-up finishes within one status tick.
        self.publish_validator_status().await?;
        if self.config.role == NodeRole::Validator {
            // Local recovery can verify a short range in a few milliseconds.
            // Keep the validator non-voting for one brief announcement window
            // so operators receive the syncing state before catch-up completes.
            tokio::time::sleep(VALIDATOR_SYNC_ANNOUNCEMENT_WINDOW).await;
        }
        if self.network.is_some() {
            self.ensure_active_sync_request(peer).await?;
        }
        Ok(())
    }

    async fn ensure_active_sync_request(&mut self, preferred_peer: PeerId) -> Result<()> {
        if self.sync_request_id.is_some() {
            return Ok(());
        }
        self.request_sync_blocks(preferred_peer).await
    }

    async fn request_sync_blocks(&mut self, peer: PeerId) -> Result<()> {
        let network = self.network.clone().context("network is unavailable")?;
        let request_id = network
            .request(
                peer,
                SyncRequest::Blocks {
                    from_height: self
                        .ledger
                        .height()
                        .checked_add(1)
                        .context("local height cannot advance")?,
                    limit: 128,
                },
            )
            .await?;
        self.sync_peer = Some(peer);
        self.sync_request_id = Some(request_id);
        self.sync_request_deadline = Some(Instant::now() + SYNC_REQUEST_WATCHDOG_TIMEOUT);
        Ok(())
    }

    fn finish_sync_if_current(&mut self) {
        if self
            .sync_target
            .is_some_and(|target| self.ledger.height() >= target)
        {
            self.sync_target = None;
            self.sync_peer = None;
            self.sync_request_id = None;
            self.sync_request_deadline = None;
            info!(
                height = self.ledger.height(),
                "node caught up to verified finality"
            );
        }
        self.publish_snapshot();
    }

    async fn respond_to_sync(&self, response_token: u64, request: SyncRequest) -> Result<()> {
        let network = self.network.as_ref().context("network is unavailable")?;
        let response = match request {
            SyncRequest::Status => {
                let finalized_tip = if self.ledger.height() == 0 {
                    None
                } else {
                    let row = self
                        .store
                        .block_by_height(self.ledger.height())?
                        .context("finalized tip is missing from storage")?;
                    Some(borsh::to_vec(&FinalizedWireRecord {
                        block: Block::decode(&row.block_bytes)?,
                        certificate: CommitCertificate::decode(&row.certificate_bytes)?,
                    })?)
                };
                SyncResponse::Status {
                    height: self.ledger.height(),
                    block_hash: self.ledger.tip_hash().to_string(),
                    state_root: self.ledger.state_root().to_string(),
                    syncing: self.sync_target.is_some(),
                    finalized_tip,
                }
            }
            SyncRequest::Blocks { from_height, limit } => {
                let mut records = Vec::new();
                let mut payload_bytes = 0_usize;
                for row in self.store.finalized_range(from_height, limit)? {
                    let record = FinalizedWireRecord {
                        block: Block::decode(&row.block_bytes)?,
                        certificate: CommitCertificate::decode(&row.certificate_bytes)?,
                    };
                    let encoded = borsh::to_vec(&record)?;
                    let response_budget = MAX_SYNC_RESPONSE_BYTES.saturating_sub(64 * 1024);
                    if payload_bytes.saturating_add(encoded.len()) > response_budget {
                        break;
                    }
                    payload_bytes += encoded.len();
                    records.push(encoded);
                }
                SyncResponse::Blocks { records }
            }
        };
        network.respond(response_token, response).await
    }

    async fn handle_sync_response(
        &mut self,
        peer: PeerId,
        intent: OutboundSyncIntent,
        response: SyncResponse,
    ) -> Result<()> {
        validate_sync_response_intent(
            peer,
            &intent,
            &response,
            self.sync_request_id,
            self.sync_peer,
            self.sync_target.is_some(),
            self.ledger.height(),
        )?;

        match (intent.request, response) {
            (
                SyncRequest::Status,
                SyncResponse::Status {
                    height,
                    block_hash,
                    state_root,
                    finalized_tip,
                    ..
                },
            ) if height > self.ledger.height() => {
                let bytes =
                    finalized_tip.context("higher peer status omitted its finality proof")?;
                let record = FinalizedWireRecord::try_from_slice(&bytes)
                    .context("decode status finality proof")?;
                if record.block.header.chain_id != self.chain_id
                    || record.certificate.chain_id != self.chain_id
                {
                    anyhow::bail!("status finality proof belongs to another chain");
                }
                record.block.validate_commitments()?;
                record
                    .certificate
                    .verify_for_block(&record.block, &self.validator_ids)?;
                if record.block.header.height != height
                    || record.block.hash().to_string() != block_hash
                    || record.block.header.state_root.to_string() != state_root
                {
                    anyhow::bail!("status fields do not match its certified finalized tip");
                }
                self.begin_sync(peer, height).await?;
            }
            (SyncRequest::Status, SyncResponse::Status { .. }) => {}
            (
                SyncRequest::Blocks {
                    from_height,
                    limit: _,
                },
                SyncResponse::Blocks { records },
            ) => {
                if records.is_empty() {
                    if self
                        .sync_target
                        .is_some_and(|target| target > self.ledger.height())
                    {
                        anyhow::bail!("sync peer returned an empty range before the target height");
                    }
                    if let Some(network) = &self.network {
                        network.request(peer, SyncRequest::Status).await?;
                    }
                    self.sync_request_id = None;
                    self.sync_request_deadline = None;
                    return Ok(());
                }
                let decoded = records
                    .into_iter()
                    .map(|bytes| {
                        FinalizedWireRecord::try_from_slice(&bytes)
                            .context("decode synchronized finalized record")
                    })
                    .collect::<Result<Vec<_>>>()?;
                let height_before = self.ledger.height();
                validate_sync_batch_sequence(&decoded, from_height.saturating_sub(1))?;
                for record in decoded {
                    self.import_finalized(record, None).await?;
                }
                if self.ledger.height() <= height_before {
                    anyhow::bail!("sync range made no finalized-height progress");
                }
                // Report verified progress while `sync_target` is still set;
                // the next periodic status reports the current voting state.
                self.publish_validator_status().await?;
                if let Some(target) = self.sync_target {
                    if self.ledger.height() < target {
                        if self.network.is_some() {
                            self.request_sync_blocks(peer).await?;
                        }
                    } else {
                        self.finish_sync_if_current();
                    }
                }
            }
            (_, SyncResponse::Error { code, message }) => {
                anyhow::bail!("peer sync error {code}: {message}");
            }
            _ => unreachable!("response kind was validated against its request intent"),
        }
        Ok(())
    }

    async fn retry_sync_after_failure(
        &mut self,
        failed_peer: PeerId,
        failed_request_id: Option<OutboundSyncRequestId>,
    ) -> Result<()> {
        if self.sync_target.is_none() || self.sync_peer != Some(failed_peer) {
            return Ok(());
        }
        if failed_request_id.is_some() && failed_request_id != self.sync_request_id {
            return Ok(());
        }
        let next_peer = choose_sync_retry_peer(&self.peers, failed_peer);
        self.sync_peer = next_peer;
        self.sync_request_id = None;
        self.sync_request_deadline = None;
        self.publish_snapshot();
        let Some(next_peer) = next_peer else {
            warn!(%failed_peer, "verified sync is waiting for another connected peer");
            return Ok(());
        };
        if next_peer == failed_peer {
            // A one-peer observer must not become permanently stuck merely
            // because its only peer returned a stale or mismatched response.
            // Bound a malicious peer's retry rate while keeping recovery live.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.request_sync_blocks(next_peer).await?;
        if next_peer == failed_peer {
            info!(%failed_peer, "retried verified block synchronization with the only connected peer");
        } else {
            info!(%failed_peer, %next_peer, "rotated verified block synchronization peer");
        }
        Ok(())
    }

    async fn retry_timed_out_sync_request(&mut self, now: Instant) -> Result<()> {
        if self.halted {
            return Ok(());
        }
        let (Some(request_id), Some(peer), Some(deadline)) = (
            self.sync_request_id,
            self.sync_peer,
            self.sync_request_deadline,
        ) else {
            return Ok(());
        };
        if now < deadline {
            return Ok(());
        }

        warn!(%peer, ?request_id, "active block synchronization request timed out");
        self.retry_sync_after_failure(peer, Some(request_id)).await
    }

    fn enter_halted_state(&mut self) {
        self.halted = true;
        self.consensus_started = false;
        self.sync_target = None;
        self.sync_peer = None;
        self.sync_request_id = None;
        self.sync_request_deadline = None;
    }

    async fn publish_validator_status(&mut self) -> Result<()> {
        let Some(key) = &self.local_validator_key else {
            return Ok(());
        };
        let local = ConsensusValidatorId::from(ProtocolValidatorId::from_signing_key(key));
        let (round, phase) = self
            .consensus
            .as_ref()
            .map(|consensus| (consensus.round(), phase_name(consensus.phase()).to_owned()))
            .unwrap_or((0, "idle".into()));
        let status = ValidatorStatusMessage {
            chain_id: self.config.chain_id.clone(),
            validator: local,
            index: self.config.validator_index.unwrap_or_default(),
            height: self.ledger.height(),
            round,
            phase: if self.sync_target.is_some() {
                "syncing".into()
            } else if self.halted {
                "halted".into()
            } else if self.consensus_started {
                phase
            } else {
                "finalized".into()
            },
            block_hash: *self.ledger.tip_hash().as_bytes(),
            state_root: *self.ledger.state_root().as_bytes(),
            syncing: self.sync_target.is_some(),
            sync_target: self.sync_target,
            timestamp_ms: unix_timestamp_ms(),
        };
        let bytes = validator_status_signing_bytes(&status)?;
        let signed = SignedValidatorStatus {
            status,
            signature: key.sign(&bytes).to_bytes().to_vec(),
        };
        self.accept_validator_status(signed.clone())?;
        if let Some(network) = &self.network
            && let Err(error) = network
                .publish_status(&self.config.chain_id, borsh::to_vec(&signed)?)
                .await
        {
            warn!(%error, "validator status could not be gossiped");
        }
        Ok(())
    }

    fn accept_validator_status(&mut self, signed: SignedValidatorStatus) -> Result<()> {
        let status = &signed.status;
        if status.chain_id != self.config.chain_id {
            anyhow::bail!("validator status belongs to another chain");
        }
        let now = unix_timestamp_ms();
        if now.abs_diff(status.timestamp_ms) > 10_000 {
            anyhow::bail!("validator status timestamp is stale or too far in the future");
        }
        let expected = self
            .consensus_ids
            .get(usize::from(status.index))
            .copied()
            .context("status uses an unknown validator index")?;
        if expected != status.validator || !self.consensus_ids.contains(&status.validator) {
            anyhow::bail!("status validator identity does not match the fixed set");
        }
        let signature: [u8; 64] = signed
            .signature
            .as_slice()
            .try_into()
            .context("status signature must be 64 bytes")?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&status.validator.0)
            .context("status validator key is malformed")?;
        key.verify_strict(
            &validator_status_signing_bytes(status)?,
            &ed25519_dalek::Signature::from_bytes(&signature),
        )
        .context("status signature is invalid")?;
        self.validator_telemetry.insert(
            status.validator,
            ValidatorTelemetry {
                height: status.height,
                round: status.round,
                phase: status.phase.clone(),
                block_hash: Hash32::from_bytes(status.block_hash),
                state_root: Hash32::from_bytes(status.state_root),
                sync_progress: (status.syncing && status.sync_target.is_some()).then(|| {
                    let target = status.sync_target.unwrap_or(status.height).max(1);
                    ((status.height as f64 / target as f64) * 100.0).clamp(0.0, 100.0) as f32
                }),
                last_seen_ms: now,
            },
        );
        self.publish_snapshot();
        Ok(())
    }

    fn note_validator_activity(
        &mut self,
        validator: ConsensusValidatorId,
        height: u64,
        phase: &str,
    ) {
        self.validator_telemetry.insert(
            validator,
            ValidatorTelemetry {
                height,
                round: self.consensus.as_ref().map_or(0, Consensus::round),
                phase: phase.to_owned(),
                block_hash: self.ledger.tip_hash(),
                state_root: self.ledger.state_root(),
                sync_progress: None,
                last_seen_ms: unix_timestamp_ms(),
            },
        );
    }

    fn project_block(
        &self,
        candidate: &LedgerState,
        block: &Block,
        certificate: &CommitCertificate,
        receipts: &[TransactionReceipt],
    ) -> Result<FinalizedProjection> {
        build_projection(&self.store, candidate, block, certificate, receipts)
    }

    fn publish_snapshot(&self) {
        self.snapshots.send_replace(snapshot_for(SnapshotInputs {
            config: &self.config,
            ledger: &self.ledger,
            mempool_size: self.mempool.len(),
            peer_count: self.peers.len(),
            sync_target: self.sync_target,
            halted: self.halted,
            validators: &self.validator_ids,
            telemetry: &self.validator_telemetry,
        }));
        let _ = self.events.send(ApiEvent {
            event_type: "validator_status".into(),
            id: self.ledger.height().to_string(),
        });
    }
}

#[derive(Clone)]
struct NodeMessageValidator {
    ledger: LedgerState,
    blocks: BTreeMap<ConsensusBlockId, Block>,
    validators: Vec<ProtocolValidatorId>,
}

impl MessageValidator for NodeMessageValidator {
    fn validate_proposal(
        &self,
        proposal: &SignedProposal,
    ) -> std::result::Result<(), ConsensusValidationError> {
        verify_ed25519_proposal(proposal)?;
        let block = self
            .blocks
            .get(&proposal.proposal.block_id)
            .ok_or(ConsensusValidationError::InvalidProposal)?;
        if ConsensusBlockId::from(block.consensus_hash()) != proposal.proposal.block_id
            || block.header.height != proposal.proposal.height
            || block.header.round > proposal.proposal.round
            || expected_protocol_proposer(&self.validators, block.header.height, block.header.round)
                != Some(block.header.proposer)
        {
            return Err(ConsensusValidationError::InvalidProposal);
        }
        let mut ledger = self.ledger.clone();
        ledger
            .apply_block(block)
            .map_err(|error| ConsensusValidationError::Other(error.to_string()))?;
        Ok(())
    }

    fn validate_vote(
        &self,
        vote: &SignedVote,
    ) -> std::result::Result<(), ConsensusValidationError> {
        verify_ed25519_vote(vote)
    }

    fn validate_certificate(
        &self,
        certificate: &ConsensusCommitCertificate,
    ) -> std::result::Result<(), ConsensusValidationError> {
        let block = self
            .blocks
            .get(&certificate.block_id)
            .ok_or(ConsensusValidationError::InvalidCertificate)?;
        let protocol = certificate
            .to_protocol()
            .map_err(|error| ConsensusValidationError::Other(error.to_string()))?;
        protocol
            .verify_for_block(block, &self.validators)
            .map_err(|error| ConsensusValidationError::Other(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ReplayReport {
    pub height: u64,
    pub block_hash: String,
    pub state_root: String,
    pub circulating_supply_atoms: u64,
    pub account_count: usize,
    pub transaction_count: usize,
}

fn replay_canonical_rows(
    rows: &[BlockRow],
    chain_id: ChainId,
    verify_derived_columns: bool,
) -> Result<LedgerState> {
    let validators = deterministic_dev_validator_keys()
        .iter()
        .map(ProtocolValidatorId::from_signing_key)
        .collect::<Vec<_>>();
    let mut ledger = LedgerState::new(chain_id);
    for row in rows {
        let block = Block::decode(&row.block_bytes).context("decode canonical block")?;
        let certificate = CommitCertificate::decode(&row.certificate_bytes)
            .context("decode commit certificate")?;
        ledger
            .apply_finalized_block(&block, &certificate, &validators)
            .with_context(|| format!("verify finalized block at height {}", row.height))?;
        if verify_derived_columns
            && (row.height != block.header.height
                || row.block_hash != block.hash().to_string()
                || row.parent_hash != block.header.parent_hash.to_string()
                || row.state_root != block.header.state_root.to_string()
                || row.proposer != hex::encode(block.header.proposer.as_bytes())
                || row.round != block.header.round
                || row.timestamp_ms != block.header.timestamp_ms
                || row.transaction_count != block.transactions.len() as u64
                || row.block_bytes != block.canonical_bytes()
                || row.certificate_bytes != certificate.canonical_bytes())
        {
            anyhow::bail!(
                "stored block projection disagrees with canonical bytes at height {}",
                row.height
            );
        }
    }
    Ok(ledger)
}

/// Verify every persisted block and certificate, reconstructing state without
/// trusting any explorer projection.
pub fn verify_store(store: &Store, chain_id: ChainId) -> Result<ReplayReport> {
    let rows = store.canonical_block_rows()?;
    let ledger = replay_canonical_rows(&rows, chain_id, true)?;
    Ok(ReplayReport {
        height: ledger.height(),
        block_hash: ledger.tip_hash().to_string(),
        state_root: ledger.state_root().to_string(),
        circulating_supply_atoms: ledger.total_supply_atoms(),
        account_count: ledger.accounts().len(),
        transaction_count: ledger.applied_transaction_count(),
    })
}

/// Rebuild every query projection from already-verified canonical history.
/// Canonical block and signer-safety tables are never deleted.
pub fn reindex_store(store: &Store, chain_id: ChainId) -> Result<ReplayReport> {
    let rows = store.canonical_block_rows()?;
    // This read-only pass deliberately trusts only canonical blobs, not the
    // duplicated explorer columns that `reindex` is meant to repair.
    replay_canonical_rows(&rows, chain_id.clone(), false)?;
    let validators = deterministic_dev_validator_keys()
        .iter()
        .map(ProtocolValidatorId::from_signing_key)
        .collect::<Vec<_>>();
    store.clear_projections()?;
    let mut ledger = LedgerState::new(chain_id);
    for row in &rows {
        let block = Block::decode(&row.block_bytes)?;
        let certificate = CommitCertificate::decode(&row.certificate_bytes)?;
        let receipts = ledger.apply_finalized_block(&block, &certificate, &validators)?;
        let projection = build_projection(store, &ledger, &block, &certificate, &receipts)?;
        store.refresh_block_projection(&projection)?;
        store.persist_finalized(&projection)?;
    }
    verify_store(store, ledger.chain_id().clone())
}

fn build_projection(
    store: &Store,
    candidate: &LedgerState,
    block: &Block,
    certificate: &CommitCertificate,
    receipts: &[TransactionReceipt],
) -> Result<FinalizedProjection> {
    let transactions = receipts
        .iter()
        .enumerate()
        .map(|(index, receipt)| {
            let (kind, recipient, amount_atoms) = match &receipt.outcome {
                TransactionOutcome::Transfer {
                    recipient,
                    amount_atoms,
                } => ("transfer", Some(recipient.to_string()), *amount_atoms),
                TransactionOutcome::RewardClaimed { reward_atoms, .. } => (
                    "claim_reward",
                    Some(receipt.sender.to_string()),
                    *reward_atoms,
                ),
                TransactionOutcome::DisplayNameUpdated { .. } => ("set_display_name", None, 0),
            };
            TransactionProjection {
                id: receipt.transaction_id.to_string(),
                index: index as u32,
                kind: kind.into(),
                sender: receipt.sender.to_string(),
                recipient,
                amount_atoms,
                nonce: receipt.nonce,
            }
        })
        .collect::<Vec<_>>();

    let mut activity = BTreeMap::<String, u64>::new();
    for transaction in &transactions {
        *activity.entry(transaction.sender.clone()).or_default() += 1;
        if let Some(recipient) = &transaction.recipient
            && recipient != &transaction.sender
        {
            *activity.entry(recipient.clone()).or_default() += 1;
        }
    }
    let changed_accounts = candidate
        .accounts()
        .iter()
        .map(|(address, account)| {
            let address_string = address.to_string();
            let existing_count = store
                .account(&address_string)
                .ok()
                .flatten()
                .map_or(0, |projection| projection.transaction_count);
            AccountProjection {
                address: address_string.clone(),
                balance_atoms: account.balance_atoms,
                nonce: account.next_nonce,
                display_name: account.display_name.clone(),
                transaction_count: existing_count
                    + activity.get(&address_string).copied().unwrap_or_default(),
            }
        })
        .collect();
    let challenge = candidate.current_challenge();
    let challenge_id = challenge.id.to_string();
    let challenge_advanced = receipts
        .iter()
        .any(|receipt| matches!(receipt.outcome, TransactionOutcome::RewardClaimed { .. }));
    let previous_challenge = store
        .metadata("challenge")?
        .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok());
    let previous_issued_height = previous_challenge.as_ref().and_then(|previous| {
        (previous["challenge_id"].as_str() == Some(challenge_id.as_str()))
            .then(|| previous["issued_at_height"].as_str().map(str::to_owned))
            .flatten()
    });
    let issued_at_height = if challenge_advanced {
        candidate.height().to_string()
    } else if let Some(height) = previous_issued_height {
        height
    } else {
        canonical_challenge_issued_height(store, challenge.id)?
            .unwrap_or(candidate.height())
            .to_string()
    };
    let challenge_json = serde_json::json!({
        "challenge_id": challenge_id,
        "expression": format!("{} {} {}", challenge.left, operation_symbol(challenge.operation), challenge.right),
        "issued_at_height": issued_at_height,
        "reward_atoms": reward_for_supply(candidate.total_supply_atoms()).unwrap_or(0).to_string(),
    })
    .to_string();

    Ok(FinalizedProjection {
        height: block.header.height,
        block_hash: block.hash().to_string(),
        parent_hash: block.header.parent_hash.to_string(),
        state_root: block.header.state_root.to_string(),
        proposer: hex::encode(block.header.proposer.as_bytes()),
        round: block.header.round,
        timestamp_ms: block.header.timestamp_ms,
        block_bytes: block.canonical_bytes(),
        certificate_bytes: certificate.canonical_bytes(),
        transactions,
        changed_accounts,
        issued_supply_atoms: candidate.total_supply_atoms(),
        challenge_json,
    })
}

fn canonical_challenge_issued_height(store: &Store, challenge_id: u64) -> Result<Option<u64>> {
    if challenge_id == 0 {
        return Ok(Some(0));
    }
    for row in store.canonical_block_rows()?.iter().rev() {
        let block =
            Block::decode(&row.block_bytes).context("decode canonical challenge history")?;
        for transaction in block.transactions.iter().rev() {
            if let kcoin_protocol::TransactionAction::ClaimReward {
                challenge_id: claimed,
                ..
            } = &transaction.unsigned.action
                && claimed.checked_add(1) == Some(challenge_id)
            {
                return Ok(Some(block.header.height));
            }
        }
    }
    Ok(None)
}

fn spawn_network_bridge(
    chain_id: String,
    mut events: broadcast::Receiver<NetworkEvent>,
    commands: mpsc::Sender<Command>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(NetworkEvent::Gossip { peer, envelope }) if envelope.chain_id == chain_id => {
                    match envelope.kind {
                        GossipKind::Transaction => {
                            if let Ok(transaction) = SignedTransaction::decode(&envelope.payload) {
                                let (response, _) = oneshot::channel();
                                let _ = commands
                                    .send(Command::Submit {
                                        transaction,
                                        gossip: false,
                                        response,
                                    })
                                    .await;
                            }
                        }
                        GossipKind::Finalized => {
                            if let Ok(record) =
                                FinalizedWireRecord::try_from_slice(&envelope.payload)
                            {
                                let _ = commands
                                    .send(Command::ImportFinalized {
                                        record,
                                        source: peer,
                                    })
                                    .await;
                            }
                        }
                        GossipKind::Consensus => {
                            if let Ok(message) =
                                ConsensusWireMessage::try_from_slice(&envelope.payload)
                            {
                                let _ = commands
                                    .send(Command::ConsensusMessage {
                                        source: peer,
                                        message,
                                    })
                                    .await;
                            }
                        }
                        GossipKind::Status => {
                            if let Ok(status) =
                                SignedValidatorStatus::try_from_slice(&envelope.payload)
                            {
                                let _ = commands.send(Command::ValidatorStatus(status)).await;
                            }
                        }
                    }
                }
                Ok(NetworkEvent::PeerConnected(peer)) => {
                    let _ = commands.send(Command::PeerConnected(peer)).await;
                }
                Ok(NetworkEvent::PeerDisconnected(peer)) => {
                    let _ = commands.send(Command::PeerDisconnected(peer)).await;
                }
                Ok(NetworkEvent::SyncRequest {
                    response_token,
                    request,
                    ..
                }) => {
                    let _ = commands
                        .send(Command::SyncRequest {
                            response_token,
                            request,
                        })
                        .await;
                }
                Ok(NetworkEvent::SyncResponse {
                    peer,
                    intent,
                    response,
                }) => {
                    let _ = commands
                        .send(Command::SyncResponse {
                            peer,
                            intent,
                            response,
                        })
                        .await;
                }
                Ok(NetworkEvent::SyncFailure {
                    peer,
                    intent,
                    error,
                }) => {
                    let _ = commands
                        .send(Command::SyncFailure {
                            peer,
                            intent,
                            error,
                        })
                        .await;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "network bridge lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

struct SnapshotInputs<'a> {
    config: &'a NodeConfig,
    ledger: &'a LedgerState,
    mempool_size: usize,
    peer_count: usize,
    sync_target: Option<u64>,
    halted: bool,
    validators: &'a [ProtocolValidatorId],
    telemetry: &'a BTreeMap<ConsensusValidatorId, ValidatorTelemetry>,
}

fn snapshot_for(inputs: SnapshotInputs<'_>) -> RuntimeSnapshot {
    let SnapshotInputs {
        config,
        ledger,
        mempool_size,
        peer_count,
        sync_target,
        halted,
        validators,
        telemetry,
    } = inputs;
    let now = unix_timestamp_ms();
    let syncing = sync_target.is_some();
    let validator_views = validators
        .iter()
        .enumerate()
        .map(|(index, validator)| {
            let local = config.validator_index == Some(index as u16);
            let status = telemetry.get(&ConsensusValidatorId::from(*validator));
            let online = config.role == NodeRole::Standalone
                || local
                || status.is_some_and(|status| now.saturating_sub(status.last_seen_ms) <= 3_500);
            let sync_progress = if syncing && local {
                let target = sync_target.unwrap_or(ledger.height()).max(1);
                Some(((ledger.height() as f64 / target as f64) * 100.0).clamp(0.0, 100.0) as f32)
            } else {
                status.and_then(|status| status.sync_progress)
            };
            ValidatorView {
                id: hex::encode(validator.as_bytes()),
                name: format!("Validator {}", index + 1),
                index: index as u16,
                online,
                phase: if halted && local {
                    "halted".into()
                } else if syncing && local {
                    "syncing".into()
                } else if !online {
                    "offline".into()
                } else {
                    status.map_or_else(|| "finalized".into(), |status| status.phase.clone())
                },
                height: status
                    .map_or(ledger.height(), |status| status.height)
                    .to_string(),
                round: status.map_or(0, |status| status.round),
                block_hash: status
                    .map_or(ledger.tip_hash(), |status| status.block_hash)
                    .to_string(),
                state_root: status
                    .map_or(ledger.state_root(), |status| status.state_root)
                    .to_string(),
                sync_progress,
                last_seen_ms: status
                    .map_or(if local { now } else { 0 }, |status| status.last_seen_ms),
            }
        })
        .collect();
    RuntimeSnapshot {
        chain_id: config.chain_id.clone(),
        protocol_version: kcoin_protocol::PROTOCOL_VERSION,
        height: ledger.height().to_string(),
        finalized_hash: ledger.tip_hash().to_string(),
        state_root: ledger.state_root().to_string(),
        circulating_supply_atoms: ledger.total_supply_atoms().to_string(),
        max_supply_atoms: MAX_SUPPLY_ATOMS.to_string(),
        mempool_size,
        peer_count,
        block_time_ms: config.heartbeat_ms,
        validators: validator_views,
        syncing,
        halted,
    }
}

fn deterministic_dev_validator_keys() -> Vec<SigningKey> {
    (1_u8..=4)
        .map(|index| {
            let mut hasher = blake3::Hasher::new_derive_key("kcoin.dev/v1/local-validator-key");
            hasher.update(&[index]);
            SigningKey::from_bytes(hasher.finalize().as_bytes())
        })
        .collect()
}

fn signer_slot(message: &SignableMessage) -> String {
    match message {
        SignableMessage::Proposal(proposal) => {
            format!("{}/{}/proposal", proposal.height, proposal.round)
        }
        SignableMessage::Vote(vote) => format!(
            "{}/{}/{}",
            vote.height,
            vote.round,
            match vote.kind {
                VoteKind::Prevote => "prevote",
                VoteKind::Precommit => "precommit",
            }
        ),
    }
}

fn choose_sync_retry_peer(peers: &HashSet<PeerId>, failed_peer: PeerId) -> Option<PeerId> {
    peers
        .iter()
        .filter(|peer| **peer != failed_peer)
        .min_by_key(|peer| peer.to_bytes())
        .copied()
        .or_else(|| peers.contains(&failed_peer).then_some(failed_peer))
}

fn validate_sync_response_intent(
    responding_peer: PeerId,
    intent: &OutboundSyncIntent,
    response: &SyncResponse,
    active_request_id: Option<OutboundSyncRequestId>,
    sync_peer: Option<PeerId>,
    syncing: bool,
    local_height: u64,
) -> Result<()> {
    if responding_peer != intent.peer {
        anyhow::bail!(
            "sync response peer {responding_peer} did not match requested peer {}",
            intent.peer
        );
    }

    match &intent.request {
        SyncRequest::Status => {
            if !matches!(
                response,
                SyncResponse::Status { .. } | SyncResponse::Error { .. }
            ) {
                anyhow::bail!("status request received a blocks response");
            }
        }
        SyncRequest::Blocks { from_height, limit } => {
            if !syncing {
                anyhow::bail!("blocks response arrived while the node was not syncing");
            }
            if active_request_id != Some(intent.request_id) {
                anyhow::bail!("blocks response did not match the active outbound request");
            }
            if sync_peer != Some(responding_peer) {
                anyhow::bail!("blocks response came from a peer that is no longer selected");
            }
            let expected_height = local_height
                .checked_add(1)
                .context("local height cannot advance")?;
            if *from_height != expected_height {
                anyhow::bail!(
                    "blocks response intent started at {from_height}, current range starts at {expected_height}"
                );
            }
            if *limit == 0 {
                anyhow::bail!("blocks response intent had a zero-sized range");
            }
            match response {
                SyncResponse::Blocks { records } if records.len() <= usize::from(*limit) => {}
                SyncResponse::Blocks { records } => anyhow::bail!(
                    "blocks response contained {} records for a limit of {limit}",
                    records.len()
                ),
                SyncResponse::Error { .. } => {}
                SyncResponse::Status { .. } => {
                    anyhow::bail!("blocks request received a status response")
                }
            }
        }
    }
    Ok(())
}

fn validate_sync_batch_sequence(records: &[FinalizedWireRecord], local_height: u64) -> Result<()> {
    let first_expected = local_height
        .checked_add(1)
        .context("local height cannot advance")?;
    for (offset, record) in records.iter().enumerate() {
        let expected = first_expected
            .checked_add(offset as u64)
            .context("sync range height overflow")?;
        if record.block.header.height != expected {
            anyhow::bail!(
                "sync range expected height {expected}, received {}",
                record.block.header.height
            );
        }
    }
    Ok(())
}

fn signed_message(message: SignableMessage, signature: Vec<u8>) -> SignedMessage {
    match message {
        SignableMessage::Proposal(proposal) => SignedMessage::Proposal(SignedProposal {
            proposal,
            signature,
        }),
        SignableMessage::Vote(vote) => SignedMessage::Vote(SignedVote { vote, signature }),
    }
}

fn expected_protocol_proposer(
    validators: &[ProtocolValidatorId],
    height: u64,
    round: u32,
) -> Option<ProtocolValidatorId> {
    if validators.is_empty() || height == 0 {
        return None;
    }
    let height_offset = height.saturating_sub(1) % validators.len() as u64;
    let round_offset = u64::from(round) % validators.len() as u64;
    let index = ((height_offset + round_offset) % validators.len() as u64) as usize;
    validators.get(index).copied()
}

fn phase_name(phase: ConsensusPhase) -> &'static str {
    match phase {
        ConsensusPhase::Propose => "proposal",
        ConsensusPhase::Prevote => "prevote",
        ConsensusPhase::Precommit => "precommit",
        ConsensusPhase::Finalized => "finalized",
    }
}

fn validator_status_signing_bytes(status: &ValidatorStatusMessage) -> Result<Vec<u8>> {
    let canonical = borsh::to_vec(status)?;
    let mut bytes = Vec::with_capacity(16 + canonical.len());
    bytes.extend_from_slice(b"KCOIN_STATUS_V1\0");
    bytes.extend_from_slice(&canonical);
    Ok(bytes)
}

fn operation_symbol(operation: kcoin_protocol::ChallengeOperation) -> &'static str {
    match operation {
        kcoin_protocol::ChallengeOperation::Add => "+",
        kcoin_protocol::ChallengeOperation::Subtract => "−",
        kcoin_protocol::ChallengeOperation::Multiply => "×",
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn timestamp_iso(timestamp_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp_ms as i64)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeRole;
    use crate::network::spawn_network;
    use kcoin_consensus::{Proposal as ConsensusProposal, Vote as ConsensusVote, VoteValue};
    use kcoin_protocol::{TransactionAction, UnsignedTransaction};

    fn config() -> NodeConfig {
        NodeConfig {
            chain_id: "kcoin-test-1".into(),
            role: NodeRole::Standalone,
            validator_index: None,
            api_addr: "127.0.0.1:0".parse().unwrap(),
            p2p_port: 0,
            db_path: ":memory:".into(),
            peers: Vec::new(),
            heartbeat_ms: 60_000,
            demo: false,
        }
    }

    #[tokio::test]
    async fn accepted_claim_is_finalized_and_persisted() {
        let store = Store::in_memory().unwrap();
        let handle = start_node(config(), store.clone(), None).await.unwrap();
        let key = SigningKey::from_bytes(&[42; 32]);
        let challenge = kcoin_protocol::Challenge::for_id(0);
        let transaction = SignedTransaction::sign(
            UnsignedTransaction::new(
                ChainId::new("kcoin-test-1").unwrap(),
                key.verifying_key().to_bytes(),
                0,
                100,
                TransactionAction::ClaimReward {
                    challenge_id: challenge.id,
                    answer: challenge.answer(),
                },
            ),
            &key,
        )
        .unwrap();
        let transaction_id = transaction.id().to_string();
        let claimant =
            kcoin_protocol::Address::from_public_key(&key.verifying_key().to_bytes()).to_string();
        handle.submit(transaction).await.unwrap();

        for _ in 0..50 {
            if store.tip().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(store.tip().unwrap().unwrap().height, 1);
        assert_eq!(handle.snapshot().circulating_supply_atoms, "100000000");
        let projected = store.transaction(&transaction_id).unwrap().unwrap();
        assert_eq!(projected.kind, "claim_reward");
        assert_eq!(projected.sender, claimant);
        assert_eq!(projected.recipient, Some(claimant.clone()));
        assert_eq!(store.account_first_seen_height(&claimant).unwrap(), Some(1));
        assert_eq!(
            store.account(&claimant).unwrap().unwrap().transaction_count,
            1
        );

        let issued_after_claim: serde_json::Value =
            serde_json::from_str(&store.metadata("challenge").unwrap().unwrap()).unwrap();
        assert_eq!(issued_after_claim["challenge_id"], "1");
        assert_eq!(issued_after_claim["issued_at_height"], "1");
        store.corrupt_metadata_for_test("challenge").unwrap();

        let recipient_key = SigningKey::from_bytes(&[43; 32]);
        let recipient =
            kcoin_protocol::Address::from_public_key(&recipient_key.verifying_key().to_bytes());
        let transfer = SignedTransaction::sign(
            UnsignedTransaction::new(
                ChainId::new("kcoin-test-1").unwrap(),
                key.verifying_key().to_bytes(),
                1,
                100,
                TransactionAction::Transfer {
                    recipient,
                    amount_atoms: 1,
                },
            ),
            &key,
        )
        .unwrap();
        handle.submit(transfer).await.unwrap();
        for _ in 0..50 {
            if store.tip().unwrap().is_some_and(|tip| tip.height >= 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let issued_after_transfer: serde_json::Value =
            serde_json::from_str(&store.metadata("challenge").unwrap().unwrap()).unwrap();
        assert_eq!(issued_after_transfer["challenge_id"], "1");
        assert_eq!(issued_after_transfer["issued_at_height"], "1");
        handle.shutdown().await;

        let chain_id = ChainId::new("kcoin-test-1").unwrap();
        store.corrupt_block_projection_for_test(2).unwrap();
        assert!(verify_store(&store, chain_id.clone()).is_err());

        let report = reindex_store(&store, chain_id.clone()).unwrap();
        assert_eq!(report.height, 2);
        assert_eq!(report.transaction_count, 2);
        verify_store(&store, chain_id).unwrap();

        let repaired = store.block_by_height(2).unwrap().unwrap();
        let canonical = Block::decode(&repaired.block_bytes).unwrap();
        assert_eq!(repaired.block_hash, canonical.hash().to_string());
        assert_eq!(
            repaired.parent_hash,
            canonical.header.parent_hash.to_string()
        );
        assert_eq!(repaired.state_root, canonical.header.state_root.to_string());
        assert_eq!(
            repaired.proposer,
            hex::encode(canonical.header.proposer.as_bytes())
        );
        assert_eq!(repaired.round, canonical.header.round);
        assert_eq!(repaired.timestamp_ms, canonical.header.timestamp_ms);
        assert_eq!(
            repaired.transaction_count,
            canonical.transactions.len() as u64
        );
    }

    #[tokio::test]
    async fn conflicting_valid_certificate_halts_the_node() {
        let store = Store::in_memory().unwrap();
        let handle = start_node(config(), store.clone(), None).await.unwrap();
        let key = SigningKey::from_bytes(&[42; 32]);
        let challenge = kcoin_protocol::Challenge::for_id(0);
        let transaction = SignedTransaction::sign(
            UnsignedTransaction::new(
                ChainId::new("kcoin-test-1").unwrap(),
                key.verifying_key().to_bytes(),
                0,
                100,
                TransactionAction::ClaimReward {
                    challenge_id: challenge.id,
                    answer: challenge.answer(),
                },
            ),
            &key,
        )
        .unwrap();
        handle.submit(transaction).await.unwrap();
        for _ in 0..50 {
            if store.tip().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let canonical_row = store.block_by_height(1).unwrap().unwrap();
        let canonical_record = FinalizedWireRecord {
            block: Block::decode(&canonical_row.block_bytes).unwrap(),
            certificate: CommitCertificate::decode(&canonical_row.certificate_bytes).unwrap(),
        };
        store.corrupt_block_projection_for_test(1).unwrap();
        handle
            .commands
            .send(Command::ImportFinalized {
                record: canonical_record.clone(),
                source: None,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!handle.snapshot().halted);

        let validator_keys = deterministic_dev_validator_keys();
        let validator_ids = validator_keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let genesis = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        let conflicting = genesis
            .build_block(validator_ids[0], 0, 1, Vec::new())
            .unwrap();
        let vote = CommitVote::new(
            ChainId::new("kcoin-test-1").unwrap(),
            1,
            0,
            conflicting.consensus_hash(),
        );
        let certificate = CommitCertificate::new(
            vote.clone(),
            validator_keys[..3]
                .iter()
                .map(|key| CommitSignature::sign(&vote, key))
                .collect(),
        );
        handle
            .commands
            .send(Command::ImportFinalized {
                record: FinalizedWireRecord {
                    block: conflicting,
                    certificate,
                },
                source: None,
            })
            .await
            .unwrap();
        for _ in 0..50 {
            if handle.snapshot().halted {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(handle.snapshot().halted);

        let halted_tip = store.tip().unwrap().unwrap();
        let halted_state_root = handle.snapshot().state_root;
        let mut canonical_ledger = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        canonical_ledger
            .apply_block(&canonical_record.block)
            .unwrap();
        let next_block = canonical_ledger
            .build_block(
                validator_ids[1],
                0,
                canonical_record.block.header.timestamp_ms + 1,
                Vec::new(),
            )
            .unwrap();
        let next_vote = CommitVote::new(
            ChainId::new("kcoin-test-1").unwrap(),
            2,
            0,
            next_block.consensus_hash(),
        );
        let next_certificate = CommitCertificate::new(
            next_vote.clone(),
            validator_keys[..3]
                .iter()
                .map(|key| CommitSignature::sign(&next_vote, key))
                .collect(),
        );
        handle
            .commands
            .send(Command::ImportFinalized {
                record: FinalizedWireRecord {
                    block: next_block,
                    certificate: next_certificate,
                },
                source: None,
            })
            .await
            .unwrap();
        handle
            .commands
            .send(Command::SyncWatchdog {
                now: Instant::now() + SYNC_REQUEST_WATCHDOG_TIMEOUT + Duration::from_secs(1),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let after_halt = store.tip().unwrap().unwrap();
        assert_eq!(after_halt.height, halted_tip.height);
        assert_eq!(after_halt.block_hash, halted_tip.block_hash);
        assert_eq!(handle.snapshot().state_root, halted_state_root);
        assert!(handle.snapshot().halted);
        handle.shutdown().await;
    }

    #[test]
    fn locked_value_reproposal_keeps_canonical_bytes_and_accepts_later_certificate() {
        assert_eq!(phase_name(ConsensusPhase::Propose), "proposal");
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let ledger = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        let round_zero = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();
        let value_id = round_zero.consensus_hash();
        let round_one = round_zero.clone();
        assert_eq!(round_one.consensus_hash(), value_id);
        assert_eq!(round_one.hash(), round_zero.hash());
        assert_eq!(round_one.canonical_bytes(), round_zero.canonical_bytes());

        let vote = CommitVote::new(
            ChainId::new("kcoin-test-1").unwrap(),
            round_one.header.height,
            1,
            value_id,
        );
        let certificate = CommitCertificate::new(
            vote.clone(),
            keys[..3]
                .iter()
                .map(|key| CommitSignature::sign(&vote, key))
                .collect(),
        );
        assert_eq!(
            certificate.verify_for_block(&round_one, &validators),
            Ok(())
        );
    }

    #[tokio::test]
    async fn two_valid_round_certificates_converge_on_one_canonical_block() {
        let store = Store::in_memory().unwrap();
        let handle = start_node(config(), store.clone(), None).await.unwrap();
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let ledger = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        let block = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();

        let certificate = |round, signers: &[SigningKey]| {
            let vote = CommitVote::new(
                ChainId::new("kcoin-test-1").unwrap(),
                block.header.height,
                round,
                block.hash(),
            );
            CommitCertificate::new(
                vote.clone(),
                signers
                    .iter()
                    .map(|key| CommitSignature::sign(&vote, key))
                    .collect(),
            )
        };
        let round_zero = certificate(0, &keys[..3]);
        let round_one = certificate(1, &keys[1..]);
        assert_eq!(round_zero.verify_for_block(&block, &validators), Ok(()));
        assert_eq!(round_one.verify_for_block(&block, &validators), Ok(()));

        for certificate in [round_zero, round_one] {
            handle
                .commands
                .send(Command::ImportFinalized {
                    record: FinalizedWireRecord {
                        block: block.clone(),
                        certificate,
                    },
                    source: None,
                })
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(!handle.snapshot().halted);
        assert_eq!(
            store.tip().unwrap().unwrap().block_hash,
            block.hash().to_string()
        );
        assert_eq!(store.list_blocks(None, 10).unwrap().len(), 1);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn untrusted_consensus_hints_cannot_poison_slots_or_force_syncing() {
        let store = Store::in_memory().unwrap();
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let consensus_ids = validators
            .iter()
            .copied()
            .map(ConsensusValidatorId::from)
            .collect::<Vec<_>>();
        let ledger = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        let block = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();
        let block_id = ConsensusBlockId::from(block.hash());
        let handle = start_node(validator_config(0, 0), store.clone(), None)
            .await
            .unwrap();

        let signed_proposal = |height: u64, round: u32, proposer_index: usize| {
            let proposal = ConsensusProposal {
                chain_id: "kcoin-test-1".into(),
                height,
                round,
                proposer: consensus_ids[proposer_index],
                block_id,
                valid_round: None,
                valid_round_proof: Vec::new(),
            };
            let signature = keys[proposer_index]
                .sign(
                    &SignableMessage::Proposal(proposal.clone())
                        .signing_bytes()
                        .unwrap(),
                )
                .to_bytes()
                .to_vec();
            SignedProposal {
                proposal,
                signature,
            }
        };

        // Validator 1 is authorized, but it is not the round-zero proposer.
        handle
            .commands
            .send(Command::ConsensusMessage {
                source: None,
                message: ConsensusWireMessage::Proposal {
                    signed: signed_proposal(1, 0, 1),
                    block: Box::new(block.clone()),
                },
            })
            .await
            .unwrap();
        // Round 100 has the right rotating proposer but is outside the
        // bounded current-round cache window.
        handle
            .commands
            .send(Command::ConsensusMessage {
                source: None,
                message: ConsensusWireMessage::Proposal {
                    signed: signed_proposal(1, 100, 0),
                    block: Box::new(block),
                },
            })
            .await
            .unwrap();
        // A correctly signed but uncommitted future-height proposal is not a
        // finalized-height proof and must not force the validator to sync.
        handle
            .commands
            .send(Command::ConsensusMessage {
                source: None,
                message: ConsensusWireMessage::Proposal {
                    signed: signed_proposal(u64::MAX, 0, 0),
                    block: Box::new(ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap()),
                },
            })
            .await
            .unwrap();
        // Request-response status is transport-authenticated telemetry, not a
        // quorum certificate, so an arbitrary peer cannot choose sync_target.
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        handle
            .commands
            .send(Command::SyncResponse {
                peer,
                intent: OutboundSyncIntent {
                    request_id: OutboundSyncRequestId::for_test(1),
                    peer,
                    request: SyncRequest::Status,
                },
                response: SyncResponse::Status {
                    height: u64::MAX,
                    block_hash: "ff".repeat(32),
                    state_root: "ee".repeat(32),
                    syncing: false,
                    finalized_tip: None,
                },
            })
            .await
            .unwrap();

        // Even a genuine three-validator certificate cannot choose this
        // node's synchronization target when it belongs to another chain.
        let wrong_chain = ChainId::new("kcoin-other-1").unwrap();
        let wrong_ledger = LedgerState::new(wrong_chain.clone());
        let wrong_block = wrong_ledger
            .build_block(validators[0], 0, 1, Vec::new())
            .unwrap();
        let wrong_vote = CommitVote::new(
            wrong_chain.clone(),
            wrong_block.header.height,
            wrong_block.header.round,
            wrong_block.hash(),
        );
        let wrong_certificate = CommitCertificate::new(
            wrong_vote.clone(),
            keys[..3]
                .iter()
                .map(|key| CommitSignature::sign(&wrong_vote, key))
                .collect(),
        );
        let wrong_record = FinalizedWireRecord {
            block: wrong_block.clone(),
            certificate: wrong_certificate,
        };
        handle
            .commands
            .send(Command::SyncResponse {
                peer,
                intent: OutboundSyncIntent {
                    request_id: OutboundSyncRequestId::for_test(2),
                    peer,
                    request: SyncRequest::Status,
                },
                response: SyncResponse::Status {
                    height: wrong_block.header.height,
                    block_hash: wrong_block.hash().to_string(),
                    state_root: wrong_block.header.state_root.to_string(),
                    syncing: false,
                    finalized_tip: Some(borsh::to_vec(&wrong_record).unwrap()),
                },
            })
            .await
            .unwrap();

        // Status telemetry is signed over its chain id, preventing a peer
        // from rewrapping a validator's status from another local network.
        let wrong_status = ValidatorStatusMessage {
            chain_id: wrong_chain.to_string(),
            validator: consensus_ids[1],
            index: 1,
            height: 999,
            round: 0,
            phase: "finalized".into(),
            block_hash: [1; 32],
            state_root: [2; 32],
            syncing: false,
            sync_target: None,
            timestamp_ms: unix_timestamp_ms(),
        };
        let wrong_status_signature = keys[1]
            .sign(&validator_status_signing_bytes(&wrong_status).unwrap())
            .to_bytes()
            .to_vec();
        handle
            .commands
            .send(Command::ValidatorStatus(SignedValidatorStatus {
                status: wrong_status,
                signature: wrong_status_signature,
            }))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(store.consensus_proposals(1).unwrap().is_empty());
        assert!(!handle.snapshot().syncing);
        assert!(!handle.snapshot().halted);
        assert!(!handle.snapshot().validators[1].online);
        handle.shutdown().await;
    }

    fn persist_test_decision(
        store: &Store,
        key: &SigningKey,
        message: SignableMessage,
        safety_state: SafetyState,
    ) -> SignedMessage {
        let sign_bytes = message.signing_bytes().unwrap();
        let signature = key.sign(&sign_bytes).to_bytes().to_vec();
        let signed = signed_message(message.clone(), signature.clone());
        store
            .persist_consensus_decision(
                &signer_slot(&message),
                &sign_bytes,
                &signature,
                &serde_json::to_vec(&safety_state).unwrap(),
                &borsh::to_vec(&signed).unwrap(),
            )
            .unwrap();
        signed
    }

    #[tokio::test]
    async fn restart_restores_lock_reuses_decisions_and_reproposes_in_next_round() {
        let store = Store::in_memory().unwrap();
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let consensus_ids = validators
            .iter()
            .copied()
            .map(ConsensusValidatorId::from)
            .collect::<Vec<_>>();
        let ledger = LedgerState::new(ChainId::new("kcoin-test-1").unwrap());
        let block = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();
        let block_id = ConsensusBlockId::from(block.consensus_hash());
        let proposal = ConsensusProposal {
            chain_id: "kcoin-test-1".into(),
            height: 1,
            round: 0,
            proposer: consensus_ids[0],
            block_id,
            valid_round: None,
            valid_round_proof: Vec::new(),
        };
        let proposal_signature = keys[0]
            .sign(
                &SignableMessage::Proposal(proposal.clone())
                    .signing_bytes()
                    .unwrap(),
            )
            .to_bytes()
            .to_vec();
        let signed_proposal = SignedProposal {
            proposal,
            signature: proposal_signature,
        };
        store
            .persist_consensus_proposal(&PersistedConsensusProposal {
                height: 1,
                round: 0,
                block_id: block_id.0.to_vec(),
                block_bytes: block.canonical_bytes(),
                signed_proposal: borsh::to_vec(&signed_proposal).unwrap(),
            })
            .unwrap();

        let prevote = SignableMessage::Vote(ConsensusVote {
            chain_id: "kcoin-test-1".into(),
            height: 1,
            round: 0,
            kind: VoteKind::Prevote,
            validator: consensus_ids[1],
            value: VoteValue::Block(block_id),
        });
        persist_test_decision(
            &store,
            &keys[1],
            prevote,
            SafetyState {
                height: 1,
                round: 0,
                locked_round: None,
                locked_block: None,
                valid_round: None,
                valid_block: None,
                valid_round_proof: Vec::new(),
            },
        );
        let valid_round_proof = (0..3)
            .map(|index| {
                let vote = ConsensusVote {
                    chain_id: "kcoin-test-1".into(),
                    height: 1,
                    round: 0,
                    kind: VoteKind::Prevote,
                    validator: consensus_ids[index],
                    value: VoteValue::Block(block_id),
                };
                let signature = keys[index]
                    .sign(&SignableMessage::Vote(vote.clone()).signing_bytes().unwrap())
                    .to_bytes()
                    .to_vec();
                SignedVote { vote, signature }
            })
            .collect::<Vec<_>>();
        let precommit = SignableMessage::Vote(ConsensusVote {
            chain_id: "kcoin-test-1".into(),
            height: 1,
            round: 0,
            kind: VoteKind::Precommit,
            validator: consensus_ids[1],
            value: VoteValue::Block(block_id),
        });
        let durable_precommit = persist_test_decision(
            &store,
            &keys[1],
            precommit,
            SafetyState {
                height: 1,
                round: 0,
                locked_round: Some(0),
                locked_block: Some(block_id),
                valid_round: Some(0),
                valid_block: Some(block_id),
                valid_round_proof: valid_round_proof.clone(),
            },
        );

        let handle = start_node(validator_config(1, 0), store.clone(), None)
            .await
            .unwrap();
        for _ in 0..100 {
            if store
                .consensus_proposals(1)
                .unwrap()
                .iter()
                .any(|proposal| proposal.round == 1)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let proposals = store.consensus_proposals(1).unwrap();
        let recovered = proposals
            .iter()
            .find(|proposal| proposal.round == 1)
            .expect("restored validator reproposed its locked value");
        let recovered_block = Block::decode(&recovered.block_bytes).unwrap();
        assert_eq!(recovered_block.consensus_hash(), block.consensus_hash());
        assert_eq!(recovered_block.canonical_bytes(), block.canonical_bytes());
        assert_eq!(recovered_block.header.proposer, validators[0]);
        assert_eq!(recovered_block.header.round, 0);
        let recovered_proposal =
            SignedProposal::try_from_slice(&recovered.signed_proposal).unwrap();
        assert_eq!(recovered_proposal.proposal.valid_round, Some(0));
        assert_eq!(
            recovered_proposal.proposal.valid_round_proof,
            valid_round_proof
        );
        let decisions = store.consensus_decisions(1).unwrap();
        let restored_precommit = decisions
            .iter()
            .find(|decision| decision.slot == "1/0/precommit")
            .unwrap();
        assert_eq!(
            restored_precommit.signed_message,
            borsh::to_vec(&durable_precommit).unwrap()
        );
        assert!(!handle.snapshot().halted);
        handle.shutdown().await;
    }

    fn free_udp_port() -> u16 {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.local_addr().unwrap().port()
    }

    #[test]
    fn stale_valid_sync_range_is_rejected_before_retrying_another_peer() {
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let chain_id = ChainId::new("kcoin-test-1").unwrap();
        let ledger = LedgerState::new(chain_id.clone());
        let block = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();
        let vote = CommitVote::new(chain_id, 1, 0, block.hash());
        let certificate = CommitCertificate::new(
            vote.clone(),
            keys[..3]
                .iter()
                .map(|key| CommitSignature::sign(&vote, key))
                .collect(),
        );
        let range = [FinalizedWireRecord { block, certificate }];
        assert!(validate_sync_batch_sequence(&range, 1).is_err());
        assert!(validate_sync_batch_sequence(&range, 0).is_ok());

        let peer = || {
            libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
        };
        let failed = peer();
        let alternate = peer();
        let peers = HashSet::from([failed, alternate]);
        assert_eq!(choose_sync_retry_peer(&peers, failed), Some(alternate));
        assert_eq!(
            choose_sync_retry_peer(&HashSet::from([failed]), failed),
            Some(failed),
            "the only connected peer is retried with a bounded delay"
        );
        assert_eq!(choose_sync_retry_peer(&HashSet::new(), failed), None);
    }

    #[test]
    fn blocks_request_answered_with_status_is_rejected_for_peer_rotation() {
        let peer = || {
            libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
        };
        let failed = peer();
        let alternate = peer();
        let request_id = OutboundSyncRequestId::for_test(41);
        let intent = OutboundSyncIntent {
            request_id,
            peer: failed,
            request: SyncRequest::Blocks {
                from_height: 8,
                limit: 128,
            },
        };
        let response = SyncResponse::Status {
            height: 20,
            block_hash: "11".repeat(32),
            state_root: "22".repeat(32),
            syncing: false,
            finalized_tip: None,
        };

        let error = validate_sync_response_intent(
            failed,
            &intent,
            &response,
            Some(request_id),
            Some(failed),
            true,
            7,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("blocks request received a status")
        );
        assert_eq!(
            choose_sync_retry_peer(&HashSet::from([failed, alternate]), failed),
            Some(alternate),
            "a mismatched response leaves the failed peer eligible for rotation"
        );
    }

    #[test]
    fn stale_request_id_peer_and_range_are_rejected() {
        let requested_peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let other_peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let request_id = OutboundSyncRequestId::for_test(10);
        let intent = OutboundSyncIntent {
            request_id,
            peer: requested_peer,
            request: SyncRequest::Blocks {
                from_height: 5,
                limit: 2,
            },
        };
        let response = SyncResponse::Blocks {
            records: Vec::new(),
        };

        assert!(
            validate_sync_response_intent(
                other_peer,
                &intent,
                &response,
                Some(request_id),
                Some(requested_peer),
                true,
                4,
            )
            .is_err()
        );
        assert!(
            validate_sync_response_intent(
                requested_peer,
                &intent,
                &response,
                Some(OutboundSyncRequestId::for_test(11)),
                Some(requested_peer),
                true,
                4,
            )
            .is_err()
        );
        assert!(
            validate_sync_response_intent(
                requested_peer,
                &intent,
                &response,
                Some(request_id),
                Some(requested_peer),
                true,
                5,
            )
            .is_err()
        );
    }

    fn validator_config(index: u16, p2p_port: u16) -> NodeConfig {
        NodeConfig {
            chain_id: "kcoin-test-1".into(),
            role: NodeRole::Validator,
            validator_index: Some(index),
            api_addr: "127.0.0.1:0".parse().unwrap(),
            p2p_port,
            db_path: ":memory:".into(),
            peers: Vec::new(),
            heartbeat_ms: 60_000,
            demo: true,
        }
    }

    fn certified_empty_history() -> (FinalizedWireRecord, FinalizedWireRecord) {
        let keys = deterministic_dev_validator_keys();
        let validators = keys
            .iter()
            .map(ProtocolValidatorId::from_signing_key)
            .collect::<Vec<_>>();
        let chain_id = ChainId::new("kcoin-test-1").unwrap();
        let mut ledger = LedgerState::new(chain_id.clone());
        let block1 = ledger.build_block(validators[0], 0, 1, Vec::new()).unwrap();
        let vote1 = CommitVote::new(chain_id.clone(), 1, 0, block1.hash());
        let record1 = FinalizedWireRecord {
            block: block1.clone(),
            certificate: CommitCertificate::new(
                vote1.clone(),
                keys[..3]
                    .iter()
                    .map(|key| CommitSignature::sign(&vote1, key))
                    .collect(),
            ),
        };
        ledger.apply_block(&block1).unwrap();
        let block2 = ledger.build_block(validators[1], 0, 2, Vec::new()).unwrap();
        let vote2 = CommitVote::new(chain_id, 2, 0, block2.hash());
        let record2 = FinalizedWireRecord {
            block: block2,
            certificate: CommitCertificate::new(
                vote2.clone(),
                keys[..3]
                    .iter()
                    .map(|key| CommitSignature::sign(&vote2, key))
                    .collect(),
            ),
        };
        (record1, record2)
    }

    fn status_response(record: &FinalizedWireRecord) -> SyncResponse {
        SyncResponse::Status {
            height: record.block.header.height,
            block_hash: record.block.hash().to_string(),
            state_root: record.block.header.state_root.to_string(),
            syncing: false,
            finalized_tip: Some(borsh::to_vec(record).unwrap()),
        }
    }

    async fn next_sync_request(
        events: &mut broadcast::Receiver<NetworkEvent>,
    ) -> (u64, SyncRequest) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let NetworkEvent::SyncRequest {
                    response_token,
                    request,
                    ..
                } = events.recv().await.unwrap()
                {
                    break (response_token, request);
                }
            }
        })
        .await
        .expect("provider receives sync request")
    }

    async fn next_blocks_request(
        events: &mut broadcast::Receiver<NetworkEvent>,
    ) -> (u64, u64, u16) {
        loop {
            let (response_token, request) = next_sync_request(events).await;
            if let SyncRequest::Blocks { from_height, limit } = request {
                return (response_token, from_height, limit);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn active_sync_request_survives_gap_hints_and_watchdog_recovers_lost_completion() {
        let provider_port = free_udp_port();
        let provider = spawn_network(
            "kcoin-test-1".into(),
            provider_port,
            Vec::new(),
            HashSet::new(),
        )
        .await
        .unwrap();
        let provider_peer = provider.local_peer_id();
        let mut provider_events = provider.subscribe();
        let observer_port = free_udp_port();
        let observer_network = spawn_network(
            "kcoin-test-1".into(),
            observer_port,
            vec![
                format!("/ip4/127.0.0.1/udp/{provider_port}/quic-v1")
                    .parse()
                    .unwrap(),
            ],
            HashSet::new(),
        )
        .await
        .unwrap();
        let mut observer_config = validator_config(0, observer_port);
        observer_config.role = NodeRole::Observer;
        observer_config.validator_index = None;
        let observer = start_node(
            observer_config,
            Store::in_memory().unwrap(),
            Some(observer_network),
        )
        .await
        .unwrap();
        let (record1, record2) = certified_empty_history();

        let (status_token, request) = next_sync_request(&mut provider_events).await;
        assert_eq!(request, SyncRequest::Status);
        provider
            .respond(status_token, status_response(&record1))
            .await
            .unwrap();
        let (first_token, first_height, _) = next_blocks_request(&mut provider_events).await;
        assert_eq!(first_height, 1);

        // Repeated certified gap hints may raise the target, but they must not
        // supersede the healthy height-one request before its response arrives.
        for request_number in 0..3 {
            observer
                .commands
                .send(Command::SyncResponse {
                    peer: provider_peer,
                    intent: OutboundSyncIntent {
                        request_id: OutboundSyncRequestId::for_test(500 + request_number),
                        peer: provider_peer,
                        request: SyncRequest::Status,
                    },
                    response: status_response(&record2),
                })
                .await
                .unwrap();
        }
        let unexpected = tokio::time::timeout(Duration::from_millis(300), async {
            next_blocks_request(&mut provider_events).await
        })
        .await;
        assert!(
            unexpected.is_err(),
            "gap hints must preserve the active request"
        );

        // Separate the first and successor deadlines enough to inject a time
        // that has expired the old request but not the new one.
        tokio::time::sleep(Duration::from_millis(800)).await;
        provider
            .respond(
                first_token,
                SyncResponse::Blocks {
                    records: vec![borsh::to_vec(&record1).unwrap()],
                },
            )
            .await
            .unwrap();
        let (_second_token, second_height, _) = next_blocks_request(&mut provider_events).await;
        assert_eq!(second_height, 2);

        observer
            .commands
            .send(Command::SyncWatchdog {
                now: Instant::now() + SYNC_REQUEST_WATCHDOG_TIMEOUT - Duration::from_millis(400),
            })
            .await
            .unwrap();
        let stale_deadline_retry = tokio::time::timeout(Duration::from_millis(300), async {
            next_blocks_request(&mut provider_events).await
        })
        .await;
        assert!(
            stale_deadline_retry.is_err(),
            "the predecessor's deadline cannot disrupt its successor"
        );

        // Simulate the response/failure completion being lost by delivering
        // neither one, then force the current actor-side deadline to expire.
        observer
            .commands
            .send(Command::SyncWatchdog {
                now: Instant::now() + SYNC_REQUEST_WATCHDOG_TIMEOUT + Duration::from_secs(1),
            })
            .await
            .unwrap();
        let (_retry_token, retry_height, _) = next_blocks_request(&mut provider_events).await;
        assert_eq!(retry_height, 2);

        observer.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_of_four_real_network_finalizes_and_observer_catches_up() {
        let chain_id = "kcoin-test-1";
        let port0 = free_udp_port();
        let network0 = spawn_network(chain_id.into(), port0, Vec::new(), HashSet::new())
            .await
            .unwrap();
        // QUIC can authenticate the remote during the handshake, so local and
        // Docker demo peers may dial a stable address without knowing the
        // process's randomly generated libp2p PeerId in advance.
        let bootstrap0: libp2p::Multiaddr = format!("/ip4/127.0.0.1/udp/{port0}/quic-v1")
            .parse()
            .unwrap();
        let store0 = Store::in_memory().unwrap();
        let node0 = start_node(validator_config(1, port0), store0.clone(), Some(network0))
            .await
            .unwrap();

        let port1 = free_udp_port();
        let network1 = spawn_network(
            chain_id.into(),
            port1,
            vec![bootstrap0.clone()],
            HashSet::new(),
        )
        .await
        .unwrap();
        let store1 = Store::in_memory().unwrap();
        let node1 = start_node(validator_config(2, port1), store1.clone(), Some(network1))
            .await
            .unwrap();

        let port2 = free_udp_port();
        let network2 = spawn_network(
            chain_id.into(),
            port2,
            vec![bootstrap0.clone()],
            HashSet::new(),
        )
        .await
        .unwrap();
        let store2 = Store::in_memory().unwrap();
        let node2 = start_node(validator_config(3, port2), store2.clone(), Some(network2))
            .await
            .unwrap();

        // Let QUIC and the gossipsub mesh settle before submitting the first
        // transaction. The round-zero proposer (validator index zero) is
        // deliberately absent, so the live quorum must time out into round one.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let wallet = SigningKey::from_bytes(&[42; 32]);
        let challenge = kcoin_protocol::Challenge::for_id(0);
        let transaction = SignedTransaction::sign(
            UnsignedTransaction::new(
                ChainId::new(chain_id).unwrap(),
                wallet.verifying_key().to_bytes(),
                0,
                100,
                TransactionAction::ClaimReward {
                    challenge_id: challenge.id,
                    answer: challenge.answer(),
                },
            ),
            &wallet,
        )
        .unwrap();
        node0.submit(transaction).await.unwrap();

        for _ in 0..200 {
            if [&store0, &store1, &store2]
                .iter()
                .all(|store| store.tip().unwrap().is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let tips = [&store0, &store1, &store2].map(|store| {
            store
                .tip()
                .unwrap()
                .expect("three live validators finalized")
        });
        assert!(tips.iter().all(|tip| tip.height == 1));
        assert!(tips.iter().all(|tip| tip.round == 1));
        assert!(tips.iter().all(
            |tip| tip.block_hash == tips[0].block_hash && tip.state_root == tips[0].state_root
        ));

        // Start the missing validator after finality. It remains non-voting
        // while it downloads and verifies the same certified history.
        let port3 = free_udp_port();
        let network3 = spawn_network(
            chain_id.into(),
            port3,
            vec![bootstrap0.clone()],
            HashSet::new(),
        )
        .await
        .unwrap();
        let store3 = Store::in_memory().unwrap();
        let node3 = start_node(validator_config(0, port3), store3.clone(), Some(network3))
            .await
            .unwrap();
        for _ in 0..200 {
            if store3.tip().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let recovered_tip = store3
            .tip()
            .unwrap()
            .expect("late validator synchronized before rejoining consensus");
        assert_eq!(recovered_tip.block_hash, tips[0].block_hash);
        assert!(!node3.snapshot().syncing);

        // Join a fresh observer only after finality. It must request and verify
        // the missing certified range rather than trusting a peer's status.
        let observer_port = free_udp_port();
        let observer_network = spawn_network(
            chain_id.into(),
            observer_port,
            vec![bootstrap0],
            HashSet::new(),
        )
        .await
        .unwrap();
        let observer_store = Store::in_memory().unwrap();
        let mut observer_config = validator_config(0, observer_port);
        observer_config.role = NodeRole::Observer;
        observer_config.validator_index = None;
        let observer = start_node(
            observer_config,
            observer_store.clone(),
            Some(observer_network),
        )
        .await
        .unwrap();
        for _ in 0..200 {
            if observer_store.tip().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let observer_tip = observer_store
            .tip()
            .unwrap()
            .expect("late observer synchronized certified history");
        assert_eq!(observer_tip.block_hash, tips[0].block_hash);
        assert_eq!(observer_tip.state_root, tips[0].state_root);

        observer.shutdown().await;
        node3.shutdown().await;
        node2.shutdown().await;
        node1.shutdown().await;
        node0.shutdown().await;
    }
}
