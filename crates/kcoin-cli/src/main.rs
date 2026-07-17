use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clap::{Args, Parser, Subcommand};
use ed25519_dalek::SigningKey;
use kcoin_node::{
    runtime::{reindex_store, verify_store},
    storage::Store,
};
use kcoin_protocol::{Address, ChainId, SignedTransaction, TransactionAction, UnsignedTransaction};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_API_URL: &str = "http://127.0.0.1:4100";
const WALLET_FORMAT: &str = "kcoin-secret-seed-v1";

#[derive(Debug, Parser)]
#[command(
    name = "kcoin",
    version,
    about = "Wallet, inspection, and reproducible demo tools for KCoin"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create or inspect a local development wallet.
    Wallet(WalletArgs),
    /// Print the node's currently claimable arithmetic challenge.
    Challenge(ChallengeArgs),
    /// Inspect and reconstruct a node database.
    Db(DatabaseArgs),
    /// Measure end-to-end finalization latency against a running node.
    Benchmark(BenchmarkArgs),
}

#[derive(Debug, Args)]
struct WalletArgs {
    #[command(subcommand)]
    command: WalletCommand,
}

#[derive(Debug, Subcommand)]
enum WalletCommand {
    /// Generate a development Ed25519 wallet file.
    #[command(alias = "keygen")]
    Generate(WalletGenerateArgs),
    /// Validate a wallet file and show its public address.
    #[command(alias = "show-address")]
    Address(WalletAddressArgs),
}

#[derive(Debug, Args)]
struct WalletGenerateArgs {
    /// Destination for the private wallet JSON. Created with owner-only permissions on Unix.
    #[arg(long, short)]
    output: PathBuf,
    /// Replace an existing file.
    #[arg(long)]
    force: bool,
    /// Emit machine-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct WalletAddressArgs {
    /// Private wallet JSON created by `kcoin wallet generate`.
    #[arg(long, short)]
    wallet: PathBuf,
    /// Emit machine-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ChallengeArgs {
    /// Base URL of a running KCoin node.
    #[arg(long, env = "KCOIN_API_URL", default_value = DEFAULT_API_URL)]
    api_url: String,
    /// Emit the unmodified JSON response.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DatabaseArgs {
    #[command(subcommand)]
    command: DatabaseCommand,
}

#[derive(Debug, Subcommand)]
enum DatabaseCommand {
    /// Decode, authenticate, and replay all finalized blocks without changing projections.
    Verify(DatabaseVerifyArgs),
    /// Replay canonical history and print the reconstructed ledger summary.
    Replay(DatabaseVerifyArgs),
    /// Verify canonical history, then rebuild explorer projections from it.
    Reindex(DatabaseVerifyArgs),
}

#[derive(Debug, Clone, Args)]
struct DatabaseVerifyArgs {
    /// SQLite database owned by a stopped node.
    #[arg(long, short)]
    db: PathBuf,
    /// Chain ID expected in every persisted block.
    #[arg(long, env = "KCOIN_CHAIN_ID", default_value = "kcoin-local-1")]
    chain_id: String,
    /// Emit machine-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct BenchmarkArgs {
    /// Base URL of a running standalone KCoin node.
    #[arg(long, env = "KCOIN_API_URL", default_value = DEFAULT_API_URL)]
    api_url: String,
    /// Number of sequential one-atom transfers to finalize (1-1000).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
    samples: u32,
    /// Per-transaction confirmation timeout in seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    timeout_seconds: u64,
    /// Reuse a funded CLI wallet. Otherwise an ephemeral wallet claims one reward first.
    #[arg(long)]
    wallet: Option<PathBuf>,
    /// Also save the complete raw report as JSON.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct WalletFile {
    format: String,
    secret_seed: String,
    public_key: String,
    address: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChallengeResponse {
    challenge_id: String,
    expression: String,
    issued_at_height: String,
    reward_atoms: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationReport {
    database: String,
    chain_id: String,
    block_count: usize,
    transaction_count: usize,
    account_count: usize,
    height: String,
    finalized_hash: String,
    state_root: String,
    circulating_supply_atoms: String,
    tip_metadata_matches: bool,
    certificate_trust: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    benchmark_version: &'static str,
    started_at_unix_ms: String,
    git_commit: Option<String>,
    client_platform: String,
    methodology: &'static str,
    chain_id: String,
    protocol_version: u64,
    node_url: String,
    node_height_at_start: String,
    connected_peer_count: u64,
    reported_online_validators: usize,
    concurrency: u32,
    sample_count: usize,
    funding_claims: u32,
    transfer_amount_atoms: String,
    elapsed_ms: f64,
    observed_sequential_finalizations_per_second: f64,
    latency_ms: Vec<f64>,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    caveat: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Wallet(args) => wallet(args),
        Command::Challenge(args) => challenge(args).await,
        Command::Db(args) => database(args),
        Command::Benchmark(args) => benchmark(args).await,
    }
}

fn wallet(args: WalletArgs) -> Result<()> {
    match args.command {
        WalletCommand::Generate(args) => generate_wallet(args),
        WalletCommand::Address(args) => show_wallet_address(args),
    }
}

fn generate_wallet(args: WalletGenerateArgs) -> Result<()> {
    if args.output.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to replace it",
            args.output.display()
        );
    }
    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create wallet directory {}", parent.display()))?;
    }

    let signing_key = SigningKey::generate(&mut OsRng);
    let public_key = signing_key.verifying_key().to_bytes();
    let address = Address::from_public_key(&public_key).to_string();
    let file = WalletFile {
        format: WALLET_FORMAT.into(),
        secret_seed: BASE64.encode(signing_key.to_bytes()),
        public_key: BASE64.encode(public_key),
        address: address.clone(),
    };
    let encoded = serde_json::to_vec_pretty(&file).context("serialize wallet")?;
    write_secret_file(&args.output, &encoded, args.force)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "address": address,
                "wallet": args.output,
            }))?
        );
    } else {
        println!("Address: {address}");
        println!("Wallet:  {}", args.output.display());
        println!("Keep this file private; it contains the wallet's signing seed.");
    }
    Ok(())
}

fn show_wallet_address(args: WalletAddressArgs) -> Result<()> {
    let key = load_wallet(&args.wallet)?;
    let public_key = key.verifying_key().to_bytes();
    let address = Address::from_public_key(&public_key).to_string();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "address": address,
                "publicKey": BASE64.encode(public_key),
            }))?
        );
    } else {
        println!("{address}");
    }
    Ok(())
}

async fn challenge(args: ChallengeArgs) -> Result<()> {
    let client = http_client()?;
    let url = endpoint(&args.api_url, "/api/v1/challenge");
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("request challenge from {url}"))?;
    let challenge: ChallengeResponse = parse_response(response).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&challenge)?);
    } else {
        println!(
            "Challenge #{}: {}",
            challenge.challenge_id, challenge.expression
        );
        println!("Reward: {} atoms", challenge.reward_atoms);
        println!("Issued at height: {}", challenge.issued_at_height);
    }
    Ok(())
}

fn database(args: DatabaseArgs) -> Result<()> {
    match args.command {
        DatabaseCommand::Verify(args) => {
            let report = verify_database(&args)?;
            print_verification(&report, args.json, "verified")
        }
        DatabaseCommand::Replay(args) => {
            let report = verify_database(&args)?;
            print_verification(&report, args.json, "replayed")
        }
        DatabaseCommand::Reindex(args) => {
            let report = reindex_database(&args)?;
            print_verification(&report, args.json, "verified and reindexed")
        }
    }
}

fn verify_database(args: &DatabaseVerifyArgs) -> Result<VerificationReport> {
    ensure!(
        args.db.exists(),
        "database {} does not exist",
        args.db.display()
    );
    let chain_id = ChainId::new(args.chain_id.clone()).context("invalid chain id")?;
    let store =
        Store::open(&args.db).with_context(|| format!("open database {}", args.db.display()))?;

    let block_count = store.canonical_block_rows()?.len();
    let replay = verify_store(&store, chain_id.clone())?;
    let tip_matches = store.tip()?.is_none_or(|tip| {
        tip.height == replay.height
            && tip.block_hash == replay.block_hash
            && tip.state_root == replay.state_root
    });
    let metadata_matches = metadata_is(&store, "tip_height", replay.height)?
        && metadata_is(&store, "tip_hash", &replay.block_hash)?
        && metadata_is(&store, "state_root", &replay.state_root)?
        && metadata_is(
            &store,
            "issued_supply_atoms",
            replay.circulating_supply_atoms,
        )?;
    let tip_metadata_matches = tip_matches && (block_count == 0 || metadata_matches);
    ensure!(
        tip_metadata_matches,
        "persisted tip metadata does not match replayed history"
    );

    Ok(VerificationReport {
        database: args.db.display().to_string(),
        chain_id: chain_id.to_string(),
        block_count,
        transaction_count: replay.transaction_count,
        account_count: replay.account_count,
        height: replay.height.to_string(),
        finalized_hash: replay.block_hash,
        state_root: replay.state_root,
        circulating_supply_atoms: replay.circulating_supply_atoms.to_string(),
        tip_metadata_matches,
        certificate_trust: "every certificate is checked against KCoin's fixed four-validator local development set",
    })
}

fn reindex_database(args: &DatabaseVerifyArgs) -> Result<VerificationReport> {
    ensure!(
        args.db.exists(),
        "database {} does not exist",
        args.db.display()
    );
    let chain_id = ChainId::new(args.chain_id.clone()).context("invalid chain id")?;
    let store =
        Store::open(&args.db).with_context(|| format!("open database {}", args.db.display()))?;
    reindex_store(&store, chain_id).context("rebuild explorer projections")?;
    verify_database(args)
}

fn metadata_is(store: &Store, key: &str, expected: impl ToString) -> Result<bool> {
    Ok(store.metadata(key)?.as_deref() == Some(expected.to_string().as_str()))
}

fn print_verification(report: &VerificationReport, json_output: bool, verb: &str) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!(
            "{} blocks / {} transactions ({verb})",
            report.block_count, report.transaction_count
        );
        println!("Height:     {}", report.height);
        println!("Block hash: {}", report.finalized_hash);
        println!("State root: {}", report.state_root);
        println!("Supply:     {} atoms", report.circulating_supply_atoms);
        println!(
            "Tip metadata matches replay: {}",
            report.tip_metadata_matches
        );
        println!("Trust note: {}", report.certificate_trust);
    }
    Ok(())
}

async fn benchmark(args: BenchmarkArgs) -> Result<()> {
    let client = http_client()?;
    let status = get_json(&client, &args.api_url, "/api/v1/status").await?;
    let chain_id = string_field(&status, "chain_id")?;
    let protocol_version = status
        .get("protocol_version")
        .and_then(Value::as_u64)
        .context("node response omitted numeric field protocol_version")?;
    let mut height = decimal_field(&status, "height")?;
    let node_height_at_start = height.to_string();
    let connected_peer_count = decimal_field(&status, "peer_count")?;
    let reported_online_validators = status
        .get("validators")
        .and_then(Value::as_array)
        .map(|validators| {
            validators
                .iter()
                .filter(|validator| {
                    validator
                        .get("online")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count()
        })
        .context("node response omitted validators array")?;
    let signing_key = match &args.wallet {
        Some(path) => load_wallet(path)?,
        None => SigningKey::generate(&mut OsRng),
    };
    let sender = Address::from_public_key(&signing_key.verifying_key().to_bytes());
    let mut account = get_account(&client, &args.api_url, &sender).await?;
    let mut nonce = account.as_ref().map_or(0, |value| value.nonce);
    let mut balance = account.as_ref().map_or(0, |value| value.balance_atoms);
    let mut funding_claims = 0;
    let timeout = Duration::from_secs(args.timeout_seconds);

    if balance < u64::from(args.samples) {
        let challenge = get_challenge(&client, &args.api_url).await?;
        let challenge_id = challenge.challenge_id.parse::<u64>()?;
        let answer = kcoin_protocol::Challenge::for_id(challenge_id).answer();
        let transaction = sign_transaction(
            &chain_id,
            &signing_key,
            nonce,
            height.saturating_add(100),
            TransactionAction::ClaimReward {
                challenge_id,
                answer,
            },
        )?;
        let id = submit(&client, &args.api_url, &transaction).await?;
        wait_for_finality(&client, &args.api_url, &id, timeout).await?;
        funding_claims = 1;
        account = get_account(&client, &args.api_url, &sender).await?;
        let funded = account.context("funding claim finalized but account was not indexed")?;
        nonce = funded.nonce;
        balance = funded.balance_atoms;
        let refreshed = get_json(&client, &args.api_url, "/api/v1/status").await?;
        height = decimal_field(&refreshed, "height")?;
    }
    ensure!(
        balance >= u64::from(args.samples),
        "wallet has {balance} atoms; benchmark requires at least {}",
        args.samples
    );

    let recipient_key = SigningKey::generate(&mut OsRng);
    let recipient = Address::from_public_key(&recipient_key.verifying_key().to_bytes());
    let started_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();
    let started = Instant::now();
    let mut raw_micros = Vec::with_capacity(args.samples as usize);
    for offset in 0..args.samples {
        let transaction = sign_transaction(
            &chain_id,
            &signing_key,
            nonce + u64::from(offset),
            height.saturating_add(100 + u64::from(offset)),
            TransactionAction::Transfer {
                recipient,
                amount_atoms: 1,
            },
        )?;
        let sample_started = Instant::now();
        let id = submit(&client, &args.api_url, &transaction).await?;
        wait_for_finality(&client, &args.api_url, &id, timeout).await?;
        raw_micros.push(sample_started.elapsed().as_micros() as u64);
    }
    let elapsed = started.elapsed();
    let latency_ms = raw_micros
        .iter()
        .map(|micros| *micros as f64 / 1_000.0)
        .collect::<Vec<_>>();
    let report = BenchmarkReport {
        benchmark_version: concat!("kcoin-cli/", env!("CARGO_PKG_VERSION")),
        started_at_unix_ms,
        git_commit: git_commit(),
        client_platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        methodology: "sequential client wall-clock time from HTTP submission start until the transaction endpoint reports finalized",
        chain_id,
        protocol_version,
        node_url: args.api_url.trim_end_matches('/').into(),
        node_height_at_start,
        connected_peer_count,
        reported_online_validators,
        concurrency: 1,
        sample_count: latency_ms.len(),
        funding_claims,
        transfer_amount_atoms: "1".into(),
        elapsed_ms: elapsed.as_secs_f64() * 1_000.0,
        observed_sequential_finalizations_per_second: latency_ms.len() as f64
            / elapsed.as_secs_f64(),
        p50_ms: percentile(&raw_micros, 50) / 1_000.0,
        p95_ms: percentile(&raw_micros, 95) / 1_000.0,
        p99_ms: percentile(&raw_micros, 99) / 1_000.0,
        latency_ms,
        caveat: "this concurrency-one observation is not a saturation or capacity TPS claim; use it only with the reported live-node topology",
    };
    let output = serde_json::to_string_pretty(&report)?;
    println!("{output}");
    if let Some(path) = args.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, format!("{output}\n"))
            .with_context(|| format!("write benchmark report {}", path.display()))?;
    }
    Ok(())
}

#[derive(Debug)]
struct AccountView {
    balance_atoms: u64,
    nonce: u64,
}

async fn get_account(
    client: &reqwest::Client,
    api_url: &str,
    address: &Address,
) -> Result<Option<AccountView>> {
    let url = endpoint(api_url, &format!("/api/v1/addresses/{address}"));
    let response = client.get(&url).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let value: Value = parse_response(response).await?;
    Ok(Some(AccountView {
        balance_atoms: decimal_field(&value, "balance_atoms")?,
        nonce: decimal_field(&value, "nonce")?,
    }))
}

async fn get_challenge(client: &reqwest::Client, api_url: &str) -> Result<ChallengeResponse> {
    let url = endpoint(api_url, "/api/v1/challenge");
    parse_response(client.get(url).send().await?).await
}

fn sign_transaction(
    chain_id: &str,
    signing_key: &SigningKey,
    nonce: u64,
    expiry_height: u64,
    action: TransactionAction,
) -> Result<SignedTransaction> {
    SignedTransaction::sign(
        UnsignedTransaction::new(
            ChainId::new(chain_id.to_owned())?,
            signing_key.verifying_key().to_bytes(),
            nonce,
            expiry_height,
            action,
        ),
        signing_key,
    )
    .context("sign transaction")
}

async fn submit(
    client: &reqwest::Client,
    api_url: &str,
    transaction: &SignedTransaction,
) -> Result<String> {
    let unsigned = &transaction.unsigned;
    let action = match &unsigned.action {
        TransactionAction::Transfer {
            recipient,
            amount_atoms,
        } => json!({
            "type": "transfer",
            "recipient": recipient.to_string(),
            "amount_atoms": amount_atoms.to_string(),
        }),
        TransactionAction::ClaimReward {
            challenge_id,
            answer,
        } => json!({
            "type": "claim_reward",
            "challenge_id": challenge_id.to_string(),
            "answer": answer.to_string(),
        }),
        TransactionAction::SetDisplayName { display_name } => json!({
            "type": "set_display_name",
            "display_name": display_name,
        }),
    };
    let body = json!({
        "protocol_version": unsigned.protocol_version,
        "chain_id": unsigned.chain_id.to_string(),
        "sender_public_key": BASE64.encode(unsigned.sender_public_key),
        "nonce": unsigned.nonce.to_string(),
        "expiry_height": unsigned.expiry_height.to_string(),
        "action": action,
        "signature": BASE64.encode(transaction.signature),
    });
    let url = endpoint(api_url, "/api/v1/transactions");
    let response: Value = parse_response(client.post(url).json(&body).send().await?).await?;
    string_field(&response, "transaction_id")
}

async fn wait_for_finality(
    client: &reqwest::Client,
    api_url: &str,
    transaction_id: &str,
    timeout: Duration,
) -> Result<()> {
    let url = endpoint(api_url, &format!("/api/v1/transactions/{transaction_id}"));
    let deadline = Instant::now() + timeout;
    loop {
        let response = client.get(&url).send().await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            let _: Value = parse_response(response).await?;
        }
        ensure!(
            Instant::now() < deadline,
            "transaction {transaction_id} did not finalize within {} seconds",
            timeout.as_secs()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn get_json(client: &reqwest::Client, api_url: &str, path: &str) -> Result<Value> {
    let url = endpoint(api_url, path);
    parse_response(client.get(&url).send().await?).await
}

async fn parse_response<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response.bytes().await.context("read HTTP response")?;
    if !status.is_success() {
        let detail = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        bail!("node returned HTTP {status}: {detail}");
    }
    serde_json::from_slice(&bytes).context("decode node JSON response")
}

fn string_field(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("node response omitted string field {field}"))
}

fn decimal_field(value: &Value, field: &str) -> Result<u64> {
    string_field(value, field)?
        .parse::<u64>()
        .with_context(|| format!("node field {field} is not an unsigned decimal string"))
}

fn percentile(raw_micros: &[u64], percentile: usize) -> f64 {
    let mut sorted = raw_micros.to_vec();
    sorted.sort_unstable();
    let index = (percentile * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted[index] as f64
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("kcoin-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build HTTP client")
}

fn git_commit() -> Option<String> {
    if let Ok(value) = std::env::var("KCOIN_GIT_SHA")
        && !value.trim().is_empty()
    {
        return Some(value.trim().to_owned());
    }
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn load_wallet(path: &Path) -> Result<SigningKey> {
    let bytes = fs::read(path).with_context(|| format!("read wallet {}", path.display()))?;
    let wallet: WalletFile = serde_json::from_slice(&bytes).context("decode wallet JSON")?;
    ensure!(wallet.format == WALLET_FORMAT, "unsupported wallet format");
    let seed: [u8; 32] = BASE64
        .decode(&wallet.secret_seed)
        .context("wallet seed is not base64")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("wallet seed must contain exactly 32 bytes"))?;
    let key = SigningKey::from_bytes(&seed);
    let public = key.verifying_key().to_bytes();
    ensure!(
        BASE64.encode(public) == wallet.public_key,
        "wallet public key does not match secret seed"
    );
    ensure!(
        Address::from_public_key(&public).to_string() == wallet.address,
        "wallet address does not match secret seed"
    );
    Ok(key)
}

fn write_secret_file(path: &Path, contents: &[u8], force: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if !force {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("create wallet {}", path.display()))?;
    file.write_all(contents)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_use_observed_samples() {
        let samples = [1_000, 2_000, 3_000, 4_000, 5_000];
        assert_eq!(percentile(&samples, 50), 3_000.0);
        assert_eq!(percentile(&samples, 95), 5_000.0);
        assert_eq!(percentile(&samples, 99), 5_000.0);
    }

    #[test]
    fn endpoints_have_exactly_one_separator() {
        assert_eq!(
            endpoint("http://127.0.0.1:4100/", "/api/v1/status"),
            "http://127.0.0.1:4100/api/v1/status"
        );
    }
}
