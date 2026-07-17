//! A deterministic, transport-agnostic consensus state machine for KCoin.
//!
//! The engine deliberately performs no signing, persistence, networking, or
//! clock I/O. When it needs to send a proposal or vote it emits
//! [`Effect::PersistBeforeBroadcast`]. The embedding node must sign the body,
//! atomically persist both the signed message and supplied [`SafetyState`], and
//! feed it back as [`Event::Persisted`]. Only then does the engine emit
//! [`Effect::Broadcast`]. This makes the crash-safety ordering part of the API.

mod engine;
mod types;

#[cfg(any(test, feature = "simulation"))]
pub mod simulation;

pub use engine::{
    Consensus, ConsensusError, MessageValidator, ValidationError, verify_ed25519_proposal,
    verify_ed25519_vote,
};
pub use types::{
    BlockId, CommitCertificate, ConsensusConfig, Effect, Event, Evidence, Finalization, Height,
    Phase, Proposal, ProtocolCertificateError, Round, SafetyState, SignableMessage, SignedMessage,
    SignedProposal, SignedVote, SigningBytesError, Timeout, ValidatorId, ValidatorSet,
    ValidatorSetError, Vote, VoteKind, VoteValue,
};
