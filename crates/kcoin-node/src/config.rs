use std::{net::SocketAddr, path::PathBuf, str::FromStr};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NodeRole {
    Validator,
    Observer,
    Standalone,
}

impl FromStr for NodeRole {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "validator" => Ok(Self::Validator),
            "observer" => Ok(Self::Observer),
            "standalone" => Ok(Self::Standalone),
            _ => anyhow::bail!("role must be validator, observer, or standalone"),
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(name = "kcoin-node", version, about = "KCoin blockchain node")]
pub struct NodeArgs {
    #[arg(long, env = "KCOIN_CHAIN_ID", default_value = "kcoin-local-1")]
    pub chain_id: String,
    #[arg(long, env = "KCOIN_ROLE", default_value = "standalone")]
    pub role: NodeRole,
    #[arg(long, env = "KCOIN_VALIDATOR_INDEX")]
    pub validator_index: Option<u16>,
    #[arg(long, env = "KCOIN_API_ADDR", default_value = "127.0.0.1:4100")]
    pub api_addr: SocketAddr,
    #[arg(long, env = "KCOIN_P2P_PORT", default_value_t = 5100)]
    pub p2p_port: u16,
    #[arg(long, env = "KCOIN_DB", default_value = ".kcoin/node.db")]
    pub db_path: PathBuf,
    #[arg(long, env = "KCOIN_PEERS", value_delimiter = ',')]
    pub peers: Vec<String>,
    #[arg(long, env = "KCOIN_HEARTBEAT_MS", default_value_t = 5_000)]
    pub heartbeat_ms: u64,
    #[arg(long, env = "KCOIN_DEMO", default_value_t = false)]
    pub demo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfig {
    pub chain_id: String,
    pub role: NodeRole,
    pub validator_index: Option<u16>,
    pub api_addr: SocketAddr,
    pub p2p_port: u16,
    pub db_path: PathBuf,
    pub peers: Vec<String>,
    pub heartbeat_ms: u64,
    pub demo: bool,
}

impl TryFrom<NodeArgs> for NodeConfig {
    type Error = anyhow::Error;

    fn try_from(args: NodeArgs) -> Result<Self> {
        if args.chain_id.is_empty() || args.chain_id.len() > 50 {
            anyhow::bail!("chain id must be between 1 and 50 characters");
        }
        if args.role == NodeRole::Validator && args.validator_index.is_none() {
            anyhow::bail!("validator nodes require --validator-index");
        }
        if let Some(index) = args.validator_index
            && index >= 4
            && args.chain_id == "kcoin-local-1"
        {
            anyhow::bail!("the default local network has validator indices 0 through 3");
        }
        let db_path = args
            .db_path
            .canonicalize()
            .or_else(|_| {
                let parent = args
                    .db_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                std::fs::create_dir_all(parent)?;
                Ok::<_, std::io::Error>(args.db_path.clone())
            })
            .context("prepare database path")?;
        Ok(Self {
            chain_id: args.chain_id,
            role: args.role,
            validator_index: args.validator_index,
            api_addr: args.api_addr,
            p2p_port: args.p2p_port,
            db_path,
            peers: args.peers,
            heartbeat_ms: args.heartbeat_ms.max(250),
            demo: args.demo,
        })
    }
}
