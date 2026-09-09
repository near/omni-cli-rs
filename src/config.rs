use std::collections::BTreeMap;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};

const CONFIG_FILE_NAME: &str = "omni-config.toml";

/// Default registry written on first run. A chain is a *logical* chain
/// (ethereum, base, solana, ...) with one variant per NEAR network: picking
/// the chain never asks about mainnet/testnet - the concrete endpoint and
/// chain id resolve later, when `network-config` is selected.
const DEFAULT_CONFIG_TOML: &str = r#"# omni-cli configuration: destination-chain registry and MPC signer settings.
#
# Chains are logical: each has one variant per NEAR network, and the concrete
# RPC endpoint / chain id resolve when `network-config` is chosen (NEAR
# mainnet -> the chain's mainnet, NEAR testnet -> its testnet). Adding one
# more chain is a new [chains.<name>] entry, not a new omni-cli release.

[mpc]
sign_gas_tgas = 250
sign_deposit_yoctonear = 1
# Key domains on the MPC signer contract (see its state() view method):
secp256k1_domain_id = 0
ed25519_domain_id = 1
# The MPC signer contract account comes from the near-cli-rs network
# connection (mpc_contract_account_id in its config.toml). Override per
# NEAR network here if needed:
# [mpc.contracts]
# mainnet = "v1.signer"
# testnet = "v1.signer-prod.testnet"

[chains.ethereum]
family = "evm"
[chains.ethereum.networks.mainnet]
rpc_url = "https://ethereum-rpc.publicnode.com"
chain_id = 1
explorer_tx_url = "https://etherscan.io/tx/"
[chains.ethereum.networks.testnet]
rpc_url = "https://ethereum-sepolia-rpc.publicnode.com"
chain_id = 11155111
explorer_tx_url = "https://sepolia.etherscan.io/tx/"

[chains.base]
family = "evm"
[chains.base.networks.mainnet]
rpc_url = "https://mainnet.base.org"
chain_id = 8453
explorer_tx_url = "https://basescan.org/tx/"
[chains.base.networks.testnet]
rpc_url = "https://sepolia.base.org"
chain_id = 84532
explorer_tx_url = "https://sepolia.basescan.org/tx/"

[chains.arbitrum]
family = "evm"
[chains.arbitrum.networks.mainnet]
rpc_url = "https://arb1.arbitrum.io/rpc"
chain_id = 42161
explorer_tx_url = "https://arbiscan.io/tx/"
[chains.arbitrum.networks.testnet]
rpc_url = "https://sepolia-rollup.arbitrum.io/rpc"
chain_id = 421614
explorer_tx_url = "https://sepolia.arbiscan.io/tx/"

[chains.solana]
family = "svm"
[chains.solana.networks.mainnet]
rpc_url = "https://api.mainnet-beta.solana.com"
explorer_tx_url = "https://solscan.io/tx/"
symbol = "SOL"
decimals = 9
[chains.solana.networks.testnet]
rpc_url = "https://api.devnet.solana.com"
explorer_tx_url = "https://solscan.io/tx/?cluster=devnet&tx="
symbol = "SOL"
decimals = 9
"#;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OmniConfig {
    #[serde(default)]
    pub mpc: MpcConfig,
    #[serde(default)]
    pub chains: BTreeMap<String, ChainDef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MpcConfig {
    #[serde(default = "default_sign_gas_tgas")]
    pub sign_gas_tgas: u64,
    #[serde(
        default = "default_sign_deposit_yoctonear",
        deserialize_with = "deserialize_u128_flexible",
        serialize_with = "serialize_u128_as_string"
    )]
    pub sign_deposit_yoctonear: u128,
    #[serde(default)]
    pub secp256k1_domain_id: u64,
    #[serde(default = "default_ed25519_domain_id")]
    pub ed25519_domain_id: u64,
    #[serde(default)]
    pub contracts: BTreeMap<String, String>,
}

impl Default for MpcConfig {
    fn default() -> Self {
        Self {
            sign_gas_tgas: default_sign_gas_tgas(),
            sign_deposit_yoctonear: default_sign_deposit_yoctonear(),
            secp256k1_domain_id: 0,
            ed25519_domain_id: default_ed25519_domain_id(),
            contracts: BTreeMap::new(),
        }
    }
}

fn default_sign_gas_tgas() -> u64 {
    250
}

fn default_sign_deposit_yoctonear() -> u128 {
    1
}

fn default_ed25519_domain_id() -> u64 {
    1
}

/// TOML has no u128; accept both an integer (within u64) and a string.
fn deserialize_u128_flexible<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u128, D::Error> {
    use serde::Deserialize;

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Flexible {
        Number(u64),
        String(String),
    }
    match Flexible::deserialize(deserializer)? {
        Flexible::Number(value) => Ok(value as u128),
        Flexible::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

fn serialize_u128_as_string<S: serde::Serializer>(
    value: &u128,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// A logical chain: a family plus one endpoint variant per NEAR network.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChainDef {
    pub family: String,
    #[serde(default)]
    pub networks: BTreeMap<String, NetworkVariant>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetworkVariant {
    pub rpc_url: String,
    /// EVM only; other families ignore it.
    #[serde(default)]
    pub chain_id: Option<u64>,
    #[serde(default)]
    pub explorer_tx_url: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub decimals: Option<u8>,
}

/// A chain resolved against the selected NEAR network: everything the chain
/// adapters need to fetch context and broadcast.
#[derive(Debug, Clone)]
pub struct ResolvedChain {
    pub chain_key: String,
    pub family: String,
    pub near_network: String,
    pub rpc_url: String,
    pub chain_id: Option<u64>,
    pub explorer_tx_url: Option<String>,
    pub symbol: String,
    pub decimals: u8,
}

impl ChainDef {
    pub fn resolve(
        &self,
        chain_key: &str,
        near_network: &str,
    ) -> color_eyre::eyre::Result<ResolvedChain> {
        let variant = self.networks.get(near_network).ok_or_else(|| {
            eyre!(
                "Chain '{chain_key}' has no variant for NEAR network '{near_network}' \
                 (configured: {}). Add [chains.{chain_key}.networks.{near_network}] \
                 to the omni config.",
                self.networks
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let (default_symbol, default_decimals) = match self.family.as_str() {
            "svm" => ("SOL", 9),
            _ => ("ETH", 18),
        };
        Ok(ResolvedChain {
            chain_key: chain_key.to_string(),
            family: self.family.clone(),
            near_network: near_network.to_string(),
            rpc_url: variant.rpc_url.clone(),
            chain_id: variant.chain_id,
            explorer_tx_url: variant.explorer_tx_url.clone(),
            symbol: variant
                .symbol
                .clone()
                .unwrap_or_else(|| default_symbol.to_string()),
            decimals: variant.decimals.unwrap_or(default_decimals),
        })
    }
}

impl ResolvedChain {
    pub fn explorer_link(&self, tx_hash: &str) -> Option<String> {
        self.explorer_tx_url
            .as_ref()
            .map(|prefix| format!("{prefix}{tx_hash}"))
    }
}

/// Loads the omni chain registry, creating a default one next to the
/// near-cli-rs config.toml on first run.
pub fn load_or_init() -> color_eyre::eyre::Result<OmniConfig> {
    let mut path = dirs::config_dir().wrap_err("Impossible to get your config dir!")?;
    path.push("near-cli");
    std::fs::create_dir_all(&path)
        .wrap_err_with(|| format!("Failed to create config directory: {}", path.display()))?;
    path.push(CONFIG_FILE_NAME);

    if !path.exists() {
        std::fs::write(&path, DEFAULT_CONFIG_TOML)
            .wrap_err_with(|| format!("Failed to write default config: {}", path.display()))?;
        eprintln!(
            "Note: created a default omni chain registry at {}",
            path.display()
        );
    }

    let content = std::fs::read_to_string(&path)
        .wrap_err_with(|| format!("Failed to read config: {}", path.display()))?;

    // Pre-networks format: [chains.<name>] carried rpc_url/near_network at the
    // top level instead of per-network variants under networks.<near_network>.
    let raw: toml::Value = toml::from_str(&content)
        .wrap_err_with(|| format!("Failed to parse config: {}", path.display()))?;
    let has_legacy_chain = raw
        .get("chains")
        .and_then(|chains| chains.as_table())
        .is_some_and(|chains| {
            chains
                .values()
                .any(|chain| chain.get("rpc_url").is_some() || chain.get("near_network").is_some())
        });
    if has_legacy_chain {
        return Err(eyre!(
            "The omni chain registry at {} uses the old flat format (one entry per \
             chain+network). The format changed to logical chains with per-NEAR-network \
             variants ([chains.<name>.networks.<near_network>]). Delete the file to \
             regenerate the defaults, or restructure your custom entries.",
            path.display()
        ));
    }

    let config: OmniConfig = toml::from_str(&content)
        .wrap_err_with(|| format!("Failed to parse config: {}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_and_resolves() {
        let config: OmniConfig = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(config.mpc.secp256k1_domain_id, 0);
        assert_eq!(config.mpc.ed25519_domain_id, 1);

        let ethereum = &config.chains["ethereum"];
        let mainnet = ethereum.resolve("ethereum", "mainnet").unwrap();
        assert_eq!(mainnet.chain_id, Some(1));
        let testnet = ethereum.resolve("ethereum", "testnet").unwrap();
        assert_eq!(testnet.chain_id, Some(11155111));

        let solana = &config.chains["solana"];
        let sol = solana.resolve("solana", "testnet").unwrap();
        assert_eq!(sol.symbol, "SOL");
        assert_eq!(sol.decimals, 9);
        assert!(sol.chain_id.is_none());

        let missing = ethereum.resolve("ethereum", "localnet").unwrap_err();
        assert!(missing.to_string().contains("localnet"));
    }
}
