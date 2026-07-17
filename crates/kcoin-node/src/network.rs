use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, Swarm, SwarmBuilder,
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identify, identity, ping,
    request_response::{self, ProtocolSupport},
    swarm::{ConnectionId, NetworkBehaviour, dial_opts::DialOpts},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{debug, info, warn};

pub const TRANSACTIONS_TOPIC: &str = "kcoin/transactions/1";
pub const CONSENSUS_TOPIC: &str = "kcoin/consensus/1";
pub const FINALIZED_TOPIC: &str = "kcoin/finalized/1";
pub const MAX_GOSSIP_MESSAGE_BYTES: usize =
    kcoin_protocol::MAX_BLOCK_BYTES + kcoin_protocol::MAX_COMMIT_CERTIFICATE_BYTES + 64 * 1024;
pub const MAX_SYNC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const BOOTSTRAP_REDIAL_INTERVAL: Duration = Duration::from_secs(3);
const MAX_BOOTSTRAP_DIALS_PER_TICK: usize = 4;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub enum GossipKind {
    Transaction,
    Consensus,
    Finalized,
    Status,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub struct GossipEnvelope {
    pub version: u16,
    pub chain_id: String,
    pub kind: GossipKind,
    pub payload: Vec<u8>,
}

impl GossipEnvelope {
    pub fn encode(&self) -> Result<Vec<u8>> {
        borsh::to_vec(self).context("encode network envelope")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let envelope: Self = borsh::from_slice(bytes).context("decode network envelope")?;
        if envelope.version != 1 {
            anyhow::bail!("unsupported network envelope version {}", envelope.version);
        }
        Ok(envelope)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SyncRequest {
    Status,
    Blocks { from_height: u64, limit: u16 },
}

/// Stable request id assigned before an outbound request enters the libp2p
/// task. The network task keeps it paired with libp2p's own request id so the
/// ledger actor can reject stale or mismatched responses deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutboundSyncRequestId(u64);

impl OutboundSyncRequestId {
    #[cfg(test)]
    pub(crate) const fn for_test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundSyncIntent {
    pub request_id: OutboundSyncRequestId,
    pub peer: PeerId,
    pub request: SyncRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SyncResponse {
    Status {
        height: u64,
        block_hash: String,
        state_root: String,
        syncing: bool,
        /// Canonical Borsh `FinalizedWireRecord` proving the claimed tip.
        /// `None` is valid only at genesis height zero.
        finalized_tip: Option<Vec<u8>>,
    },
    Blocks {
        records: Vec<Vec<u8>>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum NetworkEvent {
    Gossip {
        peer: Option<PeerId>,
        envelope: GossipEnvelope,
    },
    SyncRequest {
        peer: PeerId,
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
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
}

enum NetworkCommand {
    Publish {
        topic: &'static str,
        envelope: GossipEnvelope,
        response: oneshot::Sender<Result<()>>,
    },
    Request {
        intent: OutboundSyncIntent,
    },
    Respond {
        response_token: u64,
        response: SyncResponse,
    },
    Dial(Multiaddr),
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    sync: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}

#[derive(Debug)]
struct StaticBootstrap {
    address: Multiaddr,
    peer_id: Option<PeerId>,
    pending_connection: Option<ConnectionId>,
}

impl StaticBootstrap {
    fn new(address: Multiaddr) -> Self {
        Self {
            address,
            peer_id: None,
            pending_connection: None,
        }
    }

    fn should_dial(&self, connected_peers: &HashSet<PeerId>) -> bool {
        self.pending_connection.is_none()
            && self
                .peer_id
                .is_none_or(|peer_id| !connected_peers.contains(&peer_id))
    }
}

#[derive(Debug)]
struct StaticBootstrapSet {
    targets: Vec<StaticBootstrap>,
    cursor: usize,
}

impl StaticBootstrapSet {
    fn new(addresses: Vec<Multiaddr>) -> Self {
        let mut unique = HashSet::new();
        let targets = addresses
            .into_iter()
            .filter(|address| unique.insert(address.clone()))
            .map(StaticBootstrap::new)
            .collect();
        Self { targets, cursor: 0 }
    }

    fn redial_due(&mut self, swarm: &mut Swarm<Behaviour>) {
        let len = self.targets.len();
        if len == 0 {
            return;
        }

        let connected_peers = swarm.connected_peers().copied().collect::<HashSet<_>>();
        let mut scanned = 0;
        let mut attempts = 0;
        while scanned < len && attempts < MAX_BOOTSTRAP_DIALS_PER_TICK {
            let index = (self.cursor + scanned) % len;
            scanned += 1;
            let target = &mut self.targets[index];
            if !target.should_dial(&connected_peers) {
                continue;
            }

            // Bootstrap addresses intentionally omit `/p2p`: local node identities
            // are regenerated on restart. Dial the address as an unknown peer, then
            // remember the authenticated peer id only to suppress duplicate dials
            // while that peer still has an active connection.
            let options = DialOpts::unknown_peer_id()
                .address(target.address.clone())
                .build();
            let connection_id = options.connection_id();
            match swarm.dial(options) {
                Ok(()) => {
                    target.pending_connection = Some(connection_id);
                    attempts += 1;
                    debug!(address = %target.address, "dialing static bootstrap peer");
                }
                Err(error) => {
                    debug!(address = %target.address, %error, "static bootstrap dial was not started");
                }
            }
        }
        self.cursor = (self.cursor + scanned) % len;
    }

    fn connection_established(&mut self, connection_id: ConnectionId, peer_id: PeerId) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.pending_connection == Some(connection_id))
        {
            let previous_peer_id = target.peer_id.replace(peer_id);
            target.pending_connection = None;
            if previous_peer_id.is_some_and(|previous| previous != peer_id) {
                info!(address = %target.address, peer = %peer_id, "static bootstrap peer restarted with a new identity");
            }
        }
    }

    fn connection_failed(&mut self, connection_id: ConnectionId) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.pending_connection == Some(connection_id))
        {
            target.pending_connection = None;
        }
    }
}

pub struct NetworkHandle {
    local_peer_id: PeerId,
    commands: mpsc::Sender<NetworkCommand>,
    events: broadcast::Sender<NetworkEvent>,
    connected_peers: watch::Receiver<HashSet<PeerId>>,
    next_request_id: Arc<AtomicU64>,
}

impl Clone for NetworkHandle {
    fn clone(&self) -> Self {
        Self {
            local_peer_id: self.local_peer_id,
            commands: self.commands.clone(),
            events: self.events.clone(),
            connected_peers: self.connected_peers.clone(),
            next_request_id: self.next_request_id.clone(),
        }
    }
}

impl NetworkHandle {
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NetworkEvent> {
        self.events.subscribe()
    }

    /// Snapshot active peers even if their connection events happened before
    /// the ledger actor attached its broadcast receiver.
    pub fn connected_peers(&self) -> HashSet<PeerId> {
        self.connected_peers.borrow().clone()
    }

    pub async fn publish_transaction(&self, chain_id: &str, bytes: Vec<u8>) -> Result<()> {
        self.publish(TRANSACTIONS_TOPIC, chain_id, GossipKind::Transaction, bytes)
            .await
    }

    pub async fn publish_consensus(&self, chain_id: &str, bytes: Vec<u8>) -> Result<()> {
        self.publish(CONSENSUS_TOPIC, chain_id, GossipKind::Consensus, bytes)
            .await
    }

    pub async fn publish_finalized(&self, chain_id: &str, bytes: Vec<u8>) -> Result<()> {
        self.publish(FINALIZED_TOPIC, chain_id, GossipKind::Finalized, bytes)
            .await
    }

    pub async fn publish_status(&self, chain_id: &str, bytes: Vec<u8>) -> Result<()> {
        self.publish(CONSENSUS_TOPIC, chain_id, GossipKind::Status, bytes)
            .await
    }

    async fn publish(
        &self,
        topic: &'static str,
        chain_id: &str,
        kind: GossipKind,
        payload: Vec<u8>,
    ) -> Result<()> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(NetworkCommand::Publish {
                topic,
                envelope: GossipEnvelope {
                    version: 1,
                    chain_id: chain_id.to_owned(),
                    kind,
                    payload,
                },
                response,
            })
            .await
            .context("network task stopped")?;
        receiver.await.context("network response dropped")?
    }

    pub async fn request(
        &self,
        peer: PeerId,
        request: SyncRequest,
    ) -> Result<OutboundSyncRequestId> {
        let request_id =
            OutboundSyncRequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        self.commands
            .send(NetworkCommand::Request {
                intent: OutboundSyncIntent {
                    request_id,
                    peer,
                    request,
                },
            })
            .await
            .context("network task stopped")?;
        Ok(request_id)
    }

    pub async fn respond(&self, response_token: u64, response: SyncResponse) -> Result<()> {
        self.commands
            .send(NetworkCommand::Respond {
                response_token,
                response,
            })
            .await
            .context("network task stopped")
    }

    pub async fn dial(&self, address: Multiaddr) -> Result<()> {
        self.commands
            .send(NetworkCommand::Dial(address))
            .await
            .context("network task stopped")
    }
}

pub async fn spawn_network(
    chain_id: String,
    listen_port: u16,
    bootstrap: Vec<Multiaddr>,
    allowlist: HashSet<PeerId>,
) -> Result<NetworkHandle> {
    let identity_key = identity::Keypair::generate_ed25519();
    let local_peer_id = identity_key.public().to_peer_id();

    let topic_chain_id = chain_id.clone();
    let mut swarm = SwarmBuilder::with_existing_identity(identity_key)
        .with_tokio()
        .with_quic()
        .with_dns()?
        .with_behaviour(move |key| {
            let message_id = |message: &gossipsub::Message| {
                gossipsub::MessageId::from(blake3::hash(&message.data).to_hex().to_string())
            };
            let config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1))
                .validation_mode(ValidationMode::Strict)
                .max_transmit_size(MAX_GOSSIP_MESSAGE_BYTES)
                .message_id_fn(message_id)
                .build()
                .expect("static gossipsub configuration is valid");
            let mut gossipsub =
                gossipsub::Behaviour::new(MessageAuthenticity::Signed(key.clone()), config)
                    .expect("ed25519 identity supports signed gossip");
            for topic in [TRANSACTIONS_TOPIC, CONSENSUS_TOPIC, FINALIZED_TOPIC] {
                gossipsub.subscribe(&IdentTopic::new(format!("{topic_chain_id}/{topic}")))?;
            }
            let protocols = [(
                libp2p::StreamProtocol::new("/kcoin/sync/1"),
                ProtocolSupport::Full,
            )];
            let codec =
                request_response::cbor::codec::Codec::<SyncRequest, SyncResponse>::default()
                    .set_request_size_maximum(64 * 1024)
                    .set_response_size_maximum(MAX_SYNC_RESPONSE_BYTES as u64);
            let sync = request_response::Behaviour::with_codec(
                codec,
                protocols,
                request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
            );
            Ok(Behaviour {
                gossipsub,
                sync,
                identify: identify::Behaviour::new(identify::Config::new(
                    "/kcoin/1.0.0".into(),
                    key.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
            })
        })?
        .build();

    let listen: Multiaddr = format!("/ip4/0.0.0.0/udp/{listen_port}/quic-v1").parse()?;
    swarm.listen_on(listen)?;

    let (commands, command_rx) = mpsc::channel(256);
    let (events, _) = broadcast::channel(1_024);
    let (connected_peers_tx, connected_peers_rx) = watch::channel(HashSet::new());
    let handle = NetworkHandle {
        local_peer_id,
        commands,
        events: events.clone(),
        connected_peers: connected_peers_rx,
        next_request_id: Arc::new(AtomicU64::new(1)),
    };

    tokio::spawn(run_network(
        swarm,
        command_rx,
        events,
        connected_peers_tx,
        bootstrap,
        allowlist,
    ));

    info!(%local_peer_id, listen_port, "libp2p network started");
    Ok(handle)
}

async fn run_network(
    mut swarm: Swarm<Behaviour>,
    mut commands: mpsc::Receiver<NetworkCommand>,
    events: broadcast::Sender<NetworkEvent>,
    connected_peers: watch::Sender<HashSet<PeerId>>,
    bootstrap: Vec<Multiaddr>,
    allowlist: HashSet<PeerId>,
) {
    let mut response_token = 0_u64;
    let mut pending_responses = HashMap::new();
    let mut pending_requests = HashMap::new();
    let mut bootstrap = StaticBootstrapSet::new(bootstrap);
    let mut bootstrap_redial = tokio::time::interval(BOOTSTRAP_REDIAL_INTERVAL);
    bootstrap_redial.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            Some(command) = commands.recv() => handle_command(
                &mut swarm,
                &mut pending_responses,
                &mut pending_requests,
                command,
            ),
            _ = bootstrap_redial.tick() => bootstrap.redial_due(&mut swarm),
            event = swarm.select_next_some() => {
                use libp2p::swarm::SwarmEvent;
                match event {
                    SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    })) => {
                        if !allowlist.is_empty() && !allowlist.contains(&propagation_source) {
                            warn!(peer = %propagation_source, "ignored gossip from peer outside allowlist");
                            continue;
                        }
                        match GossipEnvelope::decode(&message.data) {
                            Ok(envelope) => { let _ = events.send(NetworkEvent::Gossip { peer: message.source, envelope }); }
                            Err(error) => warn!(peer = %propagation_source, %error, "ignored malformed gossip"),
                        }
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message { peer, message, .. })) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                response_token = response_token.wrapping_add(1);
                                let token = response_token;
                                pending_responses.insert(token, channel);
                                if events.send(NetworkEvent::SyncRequest {
                                    peer,
                                    response_token: token,
                                    request,
                                }).is_err()
                                    && let Some(channel) = pending_responses.remove(&token)
                                {
                                    let fallback = SyncResponse::Error {
                                        code: "NOT_READY".into(),
                                        message: "sync provider has not attached a response handler".into(),
                                    };
                                    let _ = swarm.behaviour_mut().sync.send_response(channel, fallback);
                                }
                            }
                            request_response::Message::Response { request_id, response } => {
                                if let Some(intent) = pending_requests.remove(&request_id) {
                                    let _ = events.send(NetworkEvent::SyncResponse {
                                        peer,
                                        intent,
                                        response,
                                    });
                                } else {
                                    warn!(?request_id, %peer, "ignored response for an unknown outbound sync request");
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::OutboundFailure {
                        peer,
                        request_id,
                        error,
                        ..
                    })) => {
                        if let Some(intent) = pending_requests.remove(&request_id) {
                            let _ = events.send(NetworkEvent::SyncFailure {
                                peer,
                                intent,
                                error: error.to_string(),
                            });
                        } else {
                            warn!(?request_id, %peer, %error, "ignored failure for an unknown outbound sync request");
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => {
                        bootstrap.connection_established(connection_id, peer_id);
                        info!(peer = %peer_id, "peer connected");
                        let mut peers = connected_peers.borrow().clone();
                        peers.insert(peer_id);
                        connected_peers.send_replace(peers);
                        let _ = events.send(NetworkEvent::PeerConnected(peer_id));
                    }
                    SwarmEvent::ConnectionClosed {
                        peer_id,
                        num_established,
                        ..
                    } => {
                        // A peer can have simultaneous inbound and outbound
                        // QUIC connections. Closing one must not erase the peer
                        // from runtime telemetry while another remains active.
                        if num_established == 0 {
                            info!(peer = %peer_id, "peer disconnected");
                            let mut peers = connected_peers.borrow().clone();
                            peers.remove(&peer_id);
                            connected_peers.send_replace(peers);
                            let _ = events.send(NetworkEvent::PeerDisconnected(peer_id));
                        } else {
                            debug!(peer = %peer_id, remaining = num_established, "peer connection closed; another remains active");
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { connection_id, peer_id, error } => {
                        bootstrap.connection_failed(connection_id);
                        debug!(?peer_id, %error, "outbound connection failed");
                    }
                    SwarmEvent::NewListenAddr { address, .. } => info!(%address, "listening"),
                    other => debug!(?other, "network event"),
                }
            }
            else => break,
        }
    }
}

fn handle_command(
    swarm: &mut Swarm<Behaviour>,
    pending_responses: &mut HashMap<u64, request_response::ResponseChannel<SyncResponse>>,
    pending_requests: &mut HashMap<request_response::OutboundRequestId, OutboundSyncIntent>,
    command: NetworkCommand,
) {
    match command {
        NetworkCommand::Publish {
            topic,
            envelope,
            response,
        } => {
            let result = envelope.encode().and_then(|bytes| {
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(
                        IdentTopic::new(format!("{}/{topic}", envelope.chain_id)),
                        bytes,
                    )
                    .map(|_| ())
                    .map_err(anyhow::Error::from)
            });
            let _ = response.send(result);
        }
        NetworkCommand::Request { intent } => {
            let request_id = swarm
                .behaviour_mut()
                .sync
                .send_request(&intent.peer, intent.request.clone());
            pending_requests.insert(request_id, intent);
        }
        NetworkCommand::Respond {
            response_token,
            response,
        } => {
            if let Some(channel) = pending_responses.remove(&response_token) {
                let _ = swarm.behaviour_mut().sync.send_response(channel, response);
            } else {
                warn!(
                    response_token,
                    "sync response arrived after its channel closed"
                );
            }
        }
        NetworkCommand::Dial(address) => {
            if let Err(error) = swarm.dial(address.clone()) {
                warn!(%address, %error, "failed to dial bootstrap peer");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_canonically() {
        let envelope = GossipEnvelope {
            version: 1,
            chain_id: "kcoin-test-1".into(),
            kind: GossipKind::Transaction,
            payload: vec![1, 2, 3],
        };
        let bytes = envelope.encode().unwrap();
        assert_eq!(GossipEnvelope::decode(&bytes).unwrap(), envelope);
        assert_eq!(
            GossipEnvelope::decode(&bytes).unwrap().encode().unwrap(),
            bytes
        );
        let largest_protocol_record = GossipEnvelope {
            version: 1,
            chain_id: "kcoin-test-1".into(),
            kind: GossipKind::Finalized,
            payload: vec![
                0;
                kcoin_protocol::MAX_BLOCK_BYTES
                    + kcoin_protocol::MAX_COMMIT_CERTIFICATE_BYTES
            ],
        }
        .encode()
        .unwrap();
        assert!(largest_protocol_record.len() <= MAX_GOSSIP_MESSAGE_BYTES);
        assert!(largest_protocol_record.len() < MAX_SYNC_RESPONSE_BYTES);
    }

    #[test]
    fn static_bootstrap_reconnects_only_after_its_peer_disconnects() {
        let address: Multiaddr = "/ip4/127.0.0.1/udp/5100/quic-v1".parse().unwrap();
        let mut target = StaticBootstrap::new(address);
        let peer_id = PeerId::random();
        let connection_id = DialOpts::unknown_peer_id()
            .address(target.address.clone())
            .build()
            .connection_id();

        assert!(target.should_dial(&HashSet::new()));
        target.pending_connection = Some(connection_id);
        assert!(!target.should_dial(&HashSet::new()));

        let mut bootstraps = StaticBootstrapSet {
            targets: vec![target],
            cursor: 0,
        };
        bootstraps.connection_established(connection_id, peer_id);
        assert!(!bootstraps.targets[0].should_dial(&HashSet::from([peer_id])));
        assert!(bootstraps.targets[0].should_dial(&HashSet::new()));
    }

    #[test]
    fn failed_static_bootstrap_attempt_becomes_retryable() {
        let address: Multiaddr = "/ip4/127.0.0.1/udp/5100/quic-v1".parse().unwrap();
        let mut bootstraps = StaticBootstrapSet::new(vec![address.clone(), address]);
        assert_eq!(
            bootstraps.targets.len(),
            1,
            "duplicate addresses are collapsed"
        );

        let connection_id = DialOpts::unknown_peer_id()
            .address(bootstraps.targets[0].address.clone())
            .build()
            .connection_id();
        bootstraps.targets[0].pending_connection = Some(connection_id);
        bootstraps.connection_failed(connection_id);

        assert!(bootstraps.targets[0].should_dial(&HashSet::new()));
    }

    fn free_udp_port() -> u16 {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn response_event_preserves_the_exact_outbound_request_intent() {
        let provider_port = free_udp_port();
        let provider = spawn_network(
            "kcoin-network-test-1".into(),
            provider_port,
            Vec::new(),
            HashSet::new(),
        )
        .await
        .unwrap();
        let mut provider_events = provider.subscribe();
        let requester = spawn_network(
            "kcoin-network-test-1".into(),
            free_udp_port(),
            vec![
                format!("/ip4/127.0.0.1/udp/{provider_port}/quic-v1")
                    .parse()
                    .unwrap(),
            ],
            HashSet::new(),
        )
        .await
        .unwrap();
        let mut requester_events = requester.subscribe();
        let provider_peer = provider.local_peer_id();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(
                    requester_events.recv().await.unwrap(),
                    NetworkEvent::PeerConnected(peer) if peer == provider_peer
                ) {
                    break;
                }
            }
        })
        .await
        .expect("requester connects to provider");

        let request = SyncRequest::Blocks {
            from_height: 17,
            limit: 23,
        };
        let request_id = requester
            .request(provider_peer, request.clone())
            .await
            .unwrap();
        let response_token = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let NetworkEvent::SyncRequest {
                    response_token,
                    request: received,
                    ..
                } = provider_events.recv().await.unwrap()
                {
                    assert_eq!(received, request);
                    break response_token;
                }
            }
        })
        .await
        .expect("provider receives block request");

        provider
            .respond(
                response_token,
                SyncResponse::Status {
                    height: 0,
                    block_hash: "00".repeat(32),
                    state_root: "00".repeat(32),
                    syncing: false,
                    finalized_tip: None,
                },
            )
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let NetworkEvent::SyncResponse {
                    peer,
                    intent,
                    response: SyncResponse::Status { .. },
                } = requester_events.recv().await.unwrap()
                {
                    assert_eq!(peer, provider_peer);
                    assert_eq!(intent.request_id, request_id);
                    assert_eq!(intent.peer, provider_peer);
                    assert_eq!(intent.request, request);
                    break;
                }
            }
        })
        .await
        .expect("requester receives response with its original intent");
    }
}
