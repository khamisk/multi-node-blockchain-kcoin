use std::{convert::Infallible, str::FromStr, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use kcoin_protocol::{
    Address, Block, ChainId, CommitCertificate, MAX_SUPPLY_ATOMS, SignedTransaction,
    TransactionAction, UnsignedTransaction, ValidationError, reward_for_supply,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::{Stream, wrappers::BroadcastStream};
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};

use crate::{
    runtime::{NodeHandle, RuntimeSnapshot, timestamp_iso},
    storage::{BlockRow, TransactionRow},
};

pub fn router(handle: NodeHandle) -> Router {
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/validators", get(validators))
        .route("/api/v1/challenge", get(challenge))
        .route("/api/v1/blocks", get(blocks))
        .route("/api/v1/blocks/{selector}", get(block))
        .route(
            "/api/v1/transactions",
            get(transactions).post(submit_transaction),
        )
        .route("/api/v1/transactions/{id}", get(transaction))
        .route("/api/v1/addresses/{address}", get(address))
        .route("/api/v1/leaderboard", get(leaderboard))
        .route("/api/v1/events", get(events))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/metrics", get(metrics))
        .layer(RequestBodyLimitLayer::new(16 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers([header::CONTENT_TYPE, header::ACCEPT]),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(handle)
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    chain_id: String,
    protocol_version: u16,
    height: String,
    finalized_hash: String,
    state_root: String,
    circulating_supply_atoms: String,
    max_supply_atoms: String,
    mempool_size: String,
    peer_count: String,
    block_time_ms: String,
    validators: Vec<ValidatorResponse>,
    syncing: bool,
    halted: bool,
}

#[derive(Debug, Serialize)]
struct ValidatorResponse {
    id: String,
    name: String,
    index: u16,
    online: bool,
    phase: String,
    height: String,
    round: u32,
    block_hash: String,
    state_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_progress: Option<f32>,
    last_seen_ms: String,
}

fn status_response(snapshot: RuntimeSnapshot) -> StatusResponse {
    StatusResponse {
        chain_id: snapshot.chain_id,
        protocol_version: snapshot.protocol_version,
        height: snapshot.height,
        finalized_hash: snapshot.finalized_hash,
        state_root: snapshot.state_root,
        circulating_supply_atoms: snapshot.circulating_supply_atoms,
        max_supply_atoms: snapshot.max_supply_atoms,
        mempool_size: snapshot.mempool_size.to_string(),
        peer_count: snapshot.peer_count.to_string(),
        block_time_ms: snapshot.block_time_ms.to_string(),
        validators: snapshot
            .validators
            .into_iter()
            .map(|validator| ValidatorResponse {
                id: validator.id,
                name: validator.name,
                index: validator.index,
                online: validator.online,
                phase: validator.phase,
                height: validator.height,
                round: validator.round,
                block_hash: validator.block_hash,
                state_root: validator.state_root,
                sync_progress: validator.sync_progress,
                last_seen_ms: validator.last_seen_ms.to_string(),
            })
            .collect(),
        syncing: snapshot.syncing,
        halted: snapshot.halted,
    }
}

async fn status(State(handle): State<NodeHandle>) -> Json<StatusResponse> {
    Json(status_response(handle.snapshot()))
}

async fn validators(State(handle): State<NodeHandle>) -> Json<Value> {
    Json(json!({
        "validators": status_response(handle.snapshot()).validators
    }))
}

#[derive(Debug, Deserialize, Serialize)]
struct ChallengeResponse {
    challenge_id: String,
    expression: String,
    issued_at_height: String,
    reward_atoms: String,
}

async fn challenge(State(handle): State<NodeHandle>) -> ApiResult<Json<ChallengeResponse>> {
    if let Some(value) = handle.store().metadata("challenge")? {
        let parsed: ChallengeResponse = serde_json::from_str(&value)
            .map_err(|error| ApiError::internal(format!("stored challenge is invalid: {error}")))?;
        return Ok(Json(parsed));
    }
    let snapshot = handle.snapshot();
    let challenge = kcoin_protocol::Challenge::for_id(0);
    let supply = snapshot
        .circulating_supply_atoms
        .parse::<u64>()
        .unwrap_or_default();
    Ok(Json(ChallengeResponse {
        challenge_id: challenge.id.to_string(),
        expression: format!(
            "{} {} {}",
            challenge.left,
            operation_symbol(challenge.operation),
            challenge.right
        ),
        issued_at_height: snapshot.height,
        reward_atoms: reward_for_supply(supply).unwrap_or(0).to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct CursorQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct Paginated<T> {
    items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExplorerBlock {
    height: String,
    hash: String,
    parent_hash: String,
    proposer: String,
    round: u32,
    header_proposer: String,
    header_round: u32,
    timestamp: String,
    transaction_count: String,
    transaction_root: String,
    state_root: String,
    signers: Vec<String>,
    certificate: ExplorerCertificate,
    #[serde(skip_serializing_if = "Option::is_none")]
    transactions: Option<Vec<ExplorerTransaction>>,
}

#[derive(Debug, Serialize)]
struct ExplorerCertificate {
    chain_id: String,
    height: String,
    round: u32,
    consensus_value_hash: String,
    signatures: Vec<ExplorerCommitSignature>,
}

#[derive(Debug, Serialize)]
struct ExplorerCommitSignature {
    validator: String,
    /// Lowercase hexadecimal Ed25519 signature bytes.
    signature: String,
}

#[derive(Debug, Serialize)]
struct ExplorerTransaction {
    id: String,
    kind: String,
    status: &'static str,
    sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipient: Option<String>,
    amount_atoms: String,
    nonce: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    block_height: String,
    block_hash: String,
    timestamp: String,
}

async fn blocks(
    State(handle): State<NodeHandle>,
    Query(query): Query<CursorQuery>,
) -> ApiResult<Json<Paginated<ExplorerBlock>>> {
    let before = query
        .cursor
        .as_deref()
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| ApiError::bad_request("MALFORMED_CURSOR", "cursor must be a block height"))?;
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let mut rows = handle.store().list_blocks(before, limit + 1)?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| {
        rows.last()
            .expect("a page with another item cannot be empty")
            .height
            .to_string()
    });
    let items = rows
        .into_iter()
        .map(|row| explorer_block(&handle, row, false))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(Paginated { items, next_cursor }))
}

async fn block(
    State(handle): State<NodeHandle>,
    Path(selector): Path<String>,
) -> ApiResult<Json<ExplorerBlock>> {
    let row = if let Ok(height) = selector.parse::<u64>() {
        handle.store().block_by_height(height)?
    } else {
        handle.store().block_by_hash(&selector)?
    }
    .ok_or_else(|| ApiError::not_found("BLOCK_NOT_FOUND", "block was not found"))?;
    Ok(Json(explorer_block(&handle, row, true)?))
}

async fn transactions(
    State(handle): State<NodeHandle>,
    Query(query): Query<CursorQuery>,
) -> ApiResult<Json<Paginated<ExplorerTransaction>>> {
    let before = query
        .cursor
        .as_deref()
        .map(parse_transaction_cursor)
        .transpose()?;
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let mut rows = handle.store().list_transactions(before, limit + 1)?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| {
        let row = rows
            .last()
            .expect("a page with another item cannot be empty");
        format!("{}:{}", row.block_height, row.index)
    });
    let items = rows
        .into_iter()
        .map(|row| explorer_transaction(&handle, row))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(Paginated { items, next_cursor }))
}

async fn transaction(
    State(handle): State<NodeHandle>,
    Path(id): Path<String>,
) -> ApiResult<Json<ExplorerTransaction>> {
    let row = handle
        .store()
        .transaction(&id)?
        .ok_or_else(|| ApiError::not_found("TRANSACTION_NOT_FOUND", "transaction was not found"))?;
    Ok(Json(explorer_transaction(&handle, row)?))
}

#[derive(Debug, Serialize)]
struct AddressResponse {
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    balance_atoms: String,
    nonce: String,
    transaction_count: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_seen_height: Option<String>,
    transactions: Vec<ExplorerTransaction>,
}

async fn address(
    State(handle): State<NodeHandle>,
    Path(address): Path<String>,
) -> ApiResult<Json<AddressResponse>> {
    let parsed = Address::from_str(&address).map_err(ApiError::validation)?;
    let account = handle.store().account(&address)?;
    let transactions = handle
        .store()
        .address_transactions(&address, 100)?
        .into_iter()
        .map(|row| explorer_transaction(&handle, row))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(AddressResponse {
        address: parsed.to_string(),
        display_name: account
            .as_ref()
            .and_then(|account| account.display_name.clone()),
        balance_atoms: account.as_ref().map_or_else(
            || "0".to_owned(),
            |account| account.balance_atoms.to_string(),
        ),
        nonce: account
            .as_ref()
            .map_or_else(|| "0".to_owned(), |account| account.nonce.to_string()),
        transaction_count: account
            .as_ref()
            .map_or(0, |account| account.transaction_count)
            .to_string(),
        first_seen_height: handle
            .store()
            .account_first_seen_height(&address)?
            .map(|height| height.to_string()),
        transactions,
    }))
}

#[derive(Debug, Serialize)]
struct LeaderboardEntry {
    rank: u32,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    balance_atoms: String,
    share_bps: u32,
    transaction_count: String,
}

#[derive(Debug, Serialize)]
struct Concentration {
    top_1_bps: u32,
    top_5_bps: u32,
    top_10_bps: u32,
}

#[derive(Debug, Serialize)]
struct LeaderboardResponse {
    entries: Vec<LeaderboardEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    circulating_supply_atoms: String,
    unissued_supply_atoms: String,
    concentration: Concentration,
}

async fn leaderboard(
    State(handle): State<NodeHandle>,
    Query(query): Query<CursorQuery>,
) -> ApiResult<Json<LeaderboardResponse>> {
    let snapshot = handle.snapshot();
    let supply = snapshot
        .circulating_supply_atoms
        .parse::<u64>()
        .unwrap_or_default();
    let offset = query
        .cursor
        .as_deref()
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|_| {
            ApiError::bad_request("MALFORMED_CURSOR", "cursor must be a decimal row offset")
        })?
        .unwrap_or(0);
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let mut accounts = handle.store().leaderboard_page(limit + 1, offset)?;
    let has_more = accounts.len() > limit as usize;
    accounts.truncate(limit as usize);
    let concentration_accounts = handle.store().leaderboard(10)?;
    let entries = accounts
        .iter()
        .enumerate()
        .map(|(index, account)| LeaderboardEntry {
            rank: offset.saturating_add(index as u32).saturating_add(1),
            address: account.address.clone(),
            display_name: account.display_name.clone(),
            balance_atoms: account.balance_atoms.to_string(),
            share_bps: basis_points(account.balance_atoms, supply),
            transaction_count: account.transaction_count.to_string(),
        })
        .collect::<Vec<_>>();
    let concentration_for = |count: usize| {
        basis_points(
            concentration_accounts
                .iter()
                .take(count)
                .map(|account| account.balance_atoms)
                .sum(),
            supply,
        )
    };
    Ok(Json(LeaderboardResponse {
        entries,
        next_cursor: has_more.then(|| offset.saturating_add(limit).to_string()),
        circulating_supply_atoms: supply.to_string(),
        unissued_supply_atoms: MAX_SUPPLY_ATOMS.saturating_sub(supply).to_string(),
        concentration: Concentration {
            top_1_bps: concentration_for(1),
            top_5_bps: concentration_for(5),
            top_10_bps: concentration_for(10),
        },
    }))
}

#[derive(Debug, Deserialize)]
struct TransactionSubmission {
    protocol_version: u16,
    chain_id: String,
    sender_public_key: String,
    nonce: String,
    expiry_height: String,
    action: ActionSubmission,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ActionSubmission {
    Transfer {
        recipient: String,
        amount_atoms: String,
    },
    ClaimReward {
        challenge_id: String,
        answer: String,
    },
    SetDisplayName {
        display_name: Option<String>,
    },
}

#[derive(Debug, Serialize)]
struct SubmissionResponse {
    transaction_id: String,
    status: &'static str,
}

async fn submit_transaction(
    State(handle): State<NodeHandle>,
    payload: std::result::Result<Json<TransactionSubmission>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<SubmissionResponse>)> {
    ensure_submission_ready(&handle.snapshot())?;
    let Json(submission) = payload.map_err(|_| {
        ApiError::bad_request(
            "MALFORMED",
            "request body must be valid transaction JSON with application/json content type",
        )
    })?;
    let transaction = transaction_from_submission(submission)?;
    let receipt = handle
        .submit(transaction)
        .await
        .map_err(ApiError::validation)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SubmissionResponse {
            transaction_id: receipt.transaction_id.to_string(),
            status: "pending",
        }),
    ))
}

fn ensure_submission_ready(snapshot: &RuntimeSnapshot) -> ApiResult<()> {
    if snapshot.halted {
        return Err(ApiError::unavailable(
            "NODE_HALTED",
            "node is safety-halted and cannot accept transactions",
        ));
    }
    if snapshot.syncing {
        return Err(ApiError::unavailable(
            "NODE_SYNCING",
            "node is verifying finalized history and cannot accept transactions",
        ));
    }
    Ok(())
}

async fn events(
    State(handle): State<NodeHandle>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(handle.subscribe());
    let stream = tokio_stream::StreamExt::filter_map(stream, |result| match result {
        Ok(event) => Some(Ok(Event::default()
            .event(event.event_type.clone())
            .json_data(event)
            .expect("API event is serializable"))),
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn health_live() -> Json<Value> {
    Json(json!({ "status": "live" }))
}

async fn health_ready(State(handle): State<NodeHandle>) -> (StatusCode, Json<Value>) {
    let snapshot = handle.snapshot();
    if snapshot.halted {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "halted", "height": snapshot.height })),
        )
    } else if snapshot.syncing {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "syncing", "height": snapshot.height })),
        )
    } else {
        (
            StatusCode::OK,
            Json(json!({ "status": "ready", "height": snapshot.height })),
        )
    }
}

async fn metrics(State(handle): State<NodeHandle>) -> Response {
    let snapshot = handle.snapshot();
    let body = format!(
        concat!(
            "# TYPE kcoin_chain_height gauge\n",
            "kcoin_chain_height {}\n",
            "# TYPE kcoin_mempool_size gauge\n",
            "kcoin_mempool_size {}\n",
            "# TYPE kcoin_connected_peers gauge\n",
            "kcoin_connected_peers {}\n",
            "# TYPE kcoin_circulating_supply_atoms gauge\n",
            "kcoin_circulating_supply_atoms {}\n"
        ),
        snapshot.height,
        snapshot.mempool_size,
        snapshot.peer_count,
        snapshot.circulating_supply_atoms,
    );
    let mut response = Body::from(body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

fn transaction_from_submission(submission: TransactionSubmission) -> ApiResult<SignedTransaction> {
    let public_key = decode_fixed::<32>(&submission.sender_public_key, "sender_public_key")?;
    let signature = decode_fixed::<64>(&submission.signature, "signature")?;
    let chain_id = ChainId::new(submission.chain_id).map_err(ApiError::validation)?;
    let nonce = parse_u64(&submission.nonce, "nonce")?;
    let expiry_height = parse_u64(&submission.expiry_height, "expiry_height")?;
    let action = match submission.action {
        ActionSubmission::Transfer {
            recipient,
            amount_atoms,
        } => TransactionAction::Transfer {
            recipient: Address::from_str(&recipient).map_err(ApiError::validation)?,
            amount_atoms: parse_u64(&amount_atoms, "amount_atoms")?,
        },
        ActionSubmission::ClaimReward {
            challenge_id,
            answer,
        } => TransactionAction::ClaimReward {
            challenge_id: parse_u64(&challenge_id, "challenge_id")?,
            answer: answer.parse::<u16>().map_err(|_| {
                ApiError::bad_request("MALFORMED", "answer must be an unsigned 16-bit integer")
            })?,
        },
        ActionSubmission::SetDisplayName { display_name } => {
            TransactionAction::SetDisplayName { display_name }
        }
    };
    let unsigned = UnsignedTransaction {
        protocol_version: submission.protocol_version,
        chain_id,
        sender_public_key: public_key,
        nonce,
        expiry_height,
        action,
    };
    SignedTransaction::from_parts(unsigned, signature).map_err(ApiError::validation)
}

fn explorer_block(
    handle: &NodeHandle,
    row: BlockRow,
    include_transactions: bool,
) -> ApiResult<ExplorerBlock> {
    let block = Block::decode(&row.block_bytes).map_err(ApiError::validation)?;
    let certificate =
        CommitCertificate::decode(&row.certificate_bytes).map_err(ApiError::validation)?;
    let snapshot = handle.snapshot();
    let validator_label = |validator_hex: &str| {
        snapshot
            .validators
            .iter()
            .find(|validator| validator.id == validator_hex)
            .map_or_else(
                || validator_hex.to_owned(),
                |validator| format!("validator-{}", validator.index + 1),
            )
    };
    let transactions = include_transactions
        .then(|| {
            block
                .transactions
                .iter()
                .map(|transaction| {
                    let row = handle
                        .store()
                        .transaction(&transaction.id().to_string())?
                        .ok_or_else(|| {
                            ApiError::internal(
                                "transaction projection is missing from finalized block",
                            )
                        })?;
                    explorer_transaction(handle, row)
                })
                .collect::<ApiResult<Vec<_>>>()
        })
        .transpose()?;
    let signers = certificate
        .signatures
        .iter()
        .map(|signature| validator_label(&hex::encode(signature.validator.as_bytes())))
        .collect::<Vec<_>>();
    let finality_proposer = if snapshot.validators.is_empty() {
        row.proposer.clone()
    } else {
        let height_offset = certificate.height.saturating_sub(1) % snapshot.validators.len() as u64;
        let round_offset = u64::from(certificate.round) % snapshot.validators.len() as u64;
        let index = ((height_offset + round_offset) % snapshot.validators.len() as u64) as usize;
        format!("validator-{}", snapshot.validators[index].index + 1)
    };
    let certificate_view = ExplorerCertificate {
        chain_id: certificate.chain_id.to_string(),
        height: certificate.height.to_string(),
        round: certificate.round,
        consensus_value_hash: certificate.block_hash.to_string(),
        signatures: certificate
            .signatures
            .iter()
            .map(|signature| ExplorerCommitSignature {
                validator: hex::encode(signature.validator.as_bytes()),
                signature: hex::encode(signature.signature),
            })
            .collect(),
    };
    Ok(ExplorerBlock {
        height: row.height.to_string(),
        hash: row.block_hash,
        parent_hash: row.parent_hash,
        proposer: finality_proposer,
        round: certificate.round,
        header_proposer: validator_label(&row.proposer),
        header_round: row.round,
        timestamp: timestamp_iso(row.timestamp_ms),
        transaction_count: block.transactions.len().to_string(),
        transaction_root: block.header.transactions_root.to_string(),
        state_root: row.state_root,
        signers,
        certificate: certificate_view,
        transactions,
    })
}

fn explorer_transaction(
    handle: &NodeHandle,
    row: TransactionRow,
) -> ApiResult<ExplorerTransaction> {
    let block = handle
        .store()
        .block_by_height(row.block_height)?
        .ok_or_else(|| ApiError::internal("transaction references a missing block"))?;
    let canonical_block = Block::decode(&block.block_bytes).map_err(ApiError::validation)?;
    let canonical_transaction = canonical_block
        .transactions
        .get(row.index as usize)
        .filter(|transaction| transaction.id().to_string() == row.id)
        .ok_or_else(|| {
            ApiError::internal("transaction projection disagrees with canonical block history")
        })?;
    let display_name = match &canonical_transaction.unsigned.action {
        TransactionAction::SetDisplayName { display_name } => display_name.clone(),
        TransactionAction::Transfer { .. } | TransactionAction::ClaimReward { .. } => handle
            .store()
            .account(&row.sender)?
            .and_then(|account| account.display_name),
    };
    Ok(ExplorerTransaction {
        id: row.id,
        kind: row.kind,
        status: "finalized",
        sender: row.sender,
        recipient: row.recipient,
        amount_atoms: row.amount_atoms.to_string(),
        nonce: row.nonce.to_string(),
        display_name,
        block_height: row.block_height.to_string(),
        block_hash: block.block_hash,
        timestamp: timestamp_iso(block.timestamp_ms),
    })
}

fn decode_fixed<const N: usize>(value: &str, field: &str) -> ApiResult<[u8; N]> {
    let bytes = BASE64.decode(value).map_err(|_| {
        ApiError::bad_request("MALFORMED", format!("{field} must be canonical base64"))
    })?;
    bytes
        .try_into()
        .map_err(|_| ApiError::bad_request("MALFORMED", format!("{field} must contain {N} bytes")))
}

fn parse_u64(value: &str, field: &str) -> ApiResult<u64> {
    value.parse::<u64>().map_err(|_| {
        ApiError::bad_request(
            "MALFORMED",
            format!("{field} must be an unsigned 64-bit decimal string"),
        )
    })
}

fn parse_transaction_cursor(value: &str) -> ApiResult<(u64, u32)> {
    let (height, index) = value.split_once(':').ok_or_else(|| {
        ApiError::bad_request(
            "MALFORMED_CURSOR",
            "transaction cursor must have height:index form",
        )
    })?;
    let height = height.parse::<u64>().map_err(|_| {
        ApiError::bad_request(
            "MALFORMED_CURSOR",
            "transaction cursor height must be an unsigned decimal integer",
        )
    })?;
    let index = index.parse::<u32>().map_err(|_| {
        ApiError::bad_request(
            "MALFORMED_CURSOR",
            "transaction cursor index must be an unsigned decimal integer",
        )
    })?;
    Ok((height, index))
}

fn basis_points(value: u64, total: u64) -> u32 {
    if total == 0 {
        0
    } else {
        ((value as u128 * 10_000) / total as u128).min(10_000) as u32
    }
}

fn operation_symbol(operation: kcoin_protocol::ChallengeOperation) -> &'static str {
    match operation {
        kcoin_protocol::ChallengeOperation::Add => "+",
        kcoin_protocol::ChallengeOperation::Subtract => "−",
        kcoin_protocol::ChallengeOperation::Multiply => "×",
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ApiError {
    fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            message: message.into(),
        }
    }

    fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: code.into(),
            message: message.into(),
        }
    }

    fn unavailable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: code.into(),
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL".into(),
            message: message.into(),
        }
    }

    fn validation(error: ValidationError) -> Self {
        let status = match error {
            ValidationError::DuplicateTransaction => StatusCode::CONFLICT,
            ValidationError::InsufficientBalance { .. }
            | ValidationError::NonceMismatch { .. }
            | ValidationError::StaleChallenge { .. }
            | ValidationError::WrongChallengeAnswer
            | ValidationError::Expired { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            code: error.code().into(),
            message: error.to_string(),
        }
    }
}

impl From<crate::storage::StorageError> for ApiError {
    fn from(error: crate::storage::StorageError) -> Self {
        tracing::error!(%error, "storage request failed");
        Self::internal("persistent explorer data is temporarily unavailable")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "code": self.code, "message": self.message })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NodeConfig, NodeRole},
        runtime::start_node,
        storage::Store,
    };
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use tower::ServiceExt;

    fn test_config() -> NodeConfig {
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

    #[test]
    fn structured_json_reconstructs_protocol_transaction() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let unsigned = UnsignedTransaction::new(
            ChainId::new("kcoin-test-1").unwrap(),
            key.verifying_key().to_bytes(),
            0,
            10,
            TransactionAction::ClaimReward {
                challenge_id: 0,
                answer: 3,
            },
        );
        let signature = key.sign(&unsigned.signing_bytes()).to_bytes();
        let submission = TransactionSubmission {
            protocol_version: 1,
            chain_id: "kcoin-test-1".into(),
            sender_public_key: BASE64.encode(key.verifying_key().as_bytes()),
            nonce: "0".into(),
            expiry_height: "10".into(),
            action: ActionSubmission::ClaimReward {
                challenge_id: "0".into(),
                answer: "3".into(),
            },
            signature: BASE64.encode(signature),
        };
        assert!(transaction_from_submission(submission).is_ok());
    }

    #[test]
    fn basis_point_math_is_integer_and_bounded() {
        assert_eq!(basis_points(25, 100), 2_500);
        assert_eq!(basis_points(1, 0), 0);
        assert_eq!(basis_points(200, 100), 10_000);
        assert_eq!(
            operation_symbol(kcoin_protocol::ChallengeOperation::Subtract),
            "−"
        );
        assert_eq!(
            operation_symbol(kcoin_protocol::ChallengeOperation::Multiply),
            "×"
        );

        let proof = serde_json::to_value(ExplorerCertificate {
            chain_id: "kcoin-test-1".into(),
            height: "7".into(),
            round: 2,
            consensus_value_hash: "ab".repeat(32),
            signatures: vec![ExplorerCommitSignature {
                validator: "cd".repeat(32),
                signature: "ef".repeat(64),
            }],
        })
        .unwrap();
        assert_eq!(proof["height"], "7");
        assert_eq!(
            proof["signatures"][0]["signature"].as_str().unwrap().len(),
            128
        );
        assert_eq!(parse_transaction_cursor("42:3").unwrap(), (42, 3));
        assert!(parse_transaction_cursor("42").is_err());
    }

    #[tokio::test]
    async fn status_uses_decimal_strings_and_malformed_json_is_stable() {
        let handle = start_node(test_config(), Store::in_memory().unwrap(), None)
            .await
            .unwrap();
        let status = serde_json::to_value(status_response(handle.snapshot())).unwrap();
        for field in ["height", "mempool_size", "peer_count", "block_time_ms"] {
            assert!(status[field].is_string(), "{field} must be a JSON string");
        }
        assert!(status["validators"][0]["last_seen_ms"].is_string());

        let mut not_ready = handle.snapshot();
        not_ready.syncing = true;
        let syncing = ensure_submission_ready(&not_ready).unwrap_err();
        assert_eq!(syncing.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(syncing.code, "NODE_SYNCING");
        not_ready.syncing = false;
        not_ready.halted = true;
        assert_eq!(
            ensure_submission_ready(&not_ready).unwrap_err().code,
            "NODE_HALTED"
        );

        let response = router(handle.clone())
            .oneshot(
                Request::post("/api/v1/transactions")
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let error: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["code"], "MALFORMED");
        assert!(error["message"].as_str().is_some_and(|message| {
            message.starts_with("request body must be valid transaction JSON")
        }));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn display_name_history_uses_each_canonical_action() {
        let store = Store::in_memory().unwrap();
        let handle = start_node(test_config(), store.clone(), None)
            .await
            .unwrap();
        let key = SigningKey::from_bytes(&[11; 32]);
        let mut ids = Vec::new();
        for (nonce, display_name) in [(0, Some("Ada".to_owned())), (1, None)] {
            let transaction = SignedTransaction::sign(
                UnsignedTransaction::new(
                    ChainId::new("kcoin-test-1").unwrap(),
                    key.verifying_key().to_bytes(),
                    nonce,
                    100,
                    TransactionAction::SetDisplayName { display_name },
                ),
                &key,
            )
            .unwrap();
            let id = transaction.id().to_string();
            handle.submit(transaction).await.unwrap();
            for _ in 0..50 {
                if store.transaction(&id).unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            ids.push(id);
        }

        let named =
            explorer_transaction(&handle, store.transaction(&ids[0]).unwrap().unwrap()).unwrap();
        let cleared =
            explorer_transaction(&handle, store.transaction(&ids[1]).unwrap().unwrap()).unwrap();
        assert_eq!(named.display_name.as_deref(), Some("Ada"));
        assert_eq!(cleared.display_name, None);

        let challenge: Value =
            serde_json::from_str(&store.metadata("challenge").unwrap().unwrap()).unwrap();
        assert_eq!(challenge["challenge_id"], "0");
        assert_eq!(challenge["issued_at_height"], "0");
        handle.shutdown().await;
    }
}
