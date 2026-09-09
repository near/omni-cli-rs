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
#
# explorer_tx_url: the tx hash is appended, or substituted for "{hash}" if
# the placeholder is present.

[mpc]
# Gas per MPC sign call; the live signer contracts require >= 15 TGas.
sign_gas_tgas = 30
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

[chains.eth]
family = "evm"
[chains.eth.networks.mainnet]
rpc_url = "https://ethereum-rpc.publicnode.com"
chain_id = 1
explorer_tx_url = "https://etherscan.io/tx/"
[chains.eth.networks.testnet]
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

[chains.arb]
family = "evm"
[chains.arb.networks.mainnet]
rpc_url = "https://arb1.arbitrum.io/rpc"
chain_id = 42161
explorer_tx_url = "https://arbiscan.io/tx/"
[chains.arb.networks.testnet]
rpc_url = "https://sepolia-rollup.arbitrum.io/rpc"
chain_id = 421614
explorer_tx_url = "https://sepolia.arbiscan.io/tx/"

[chains.bnb]
family = "evm"
[chains.bnb.networks.mainnet]
rpc_url = "https://bsc-dataseed.bnbchain.org"
chain_id = 56
explorer_tx_url = "https://bscscan.com/tx/"
symbol = "BNB"
[chains.bnb.networks.testnet]
rpc_url = "https://bsc-testnet-dataseed.bnbchain.org"
chain_id = 97
explorer_tx_url = "https://testnet.bscscan.com/tx/"
symbol = "BNB"

[chains.pol]
family = "evm"
[chains.pol.networks.mainnet]
rpc_url = "https://polygon-rpc.com"
chain_id = 137
explorer_tx_url = "https://polygonscan.com/tx/"
symbol = "POL"
[chains.pol.networks.testnet]
rpc_url = "https://rpc-amoy.polygon.technology"
chain_id = 80002
explorer_tx_url = "https://amoy.polygonscan.com/tx/"
symbol = "POL"

[chains.hyperevm]
family = "evm"
[chains.hyperevm.networks.mainnet]
rpc_url = "https://rpc.hyperliquid.xyz/evm"
chain_id = 999
explorer_tx_url = "https://hyperevmscan.io/tx/"
symbol = "HYPE"
[chains.hyperevm.networks.testnet]
rpc_url = "https://rpc.hyperliquid-testnet.xyz/evm"
chain_id = 998
explorer_tx_url = "https://testnet.purrsec.com/tx/"
symbol = "HYPE"

[chains.abs]
family = "evm"
[chains.abs.networks.mainnet]
rpc_url = "https://api.mainnet.abs.xyz"
chain_id = 2741
explorer_tx_url = "https://abscan.org/tx/"
[chains.abs.networks.testnet]
rpc_url = "https://api.testnet.abs.xyz"
chain_id = 11124
explorer_tx_url = "https://sepolia.abscan.org/tx/"

[chains.btc]
family = "utxo"
[chains.btc.networks.mainnet]
rpc_url = "https://blockstream.info/api"
explorer_tx_url = "https://mempool.space/tx/"
symbol = "BTC"
decimals = 8
[chains.btc.networks.testnet]
rpc_url = "https://blockstream.info/testnet/api"
explorer_tx_url = "https://mempool.space/testnet/tx/"
symbol = "BTC"
decimals = 8

[chains.ton]
family = "ton"
[chains.ton.networks.mainnet]
rpc_url = "https://toncenter.com/api/v2"
explorer_tx_url = "https://tonviewer.com/transaction/"
symbol = "TON"
decimals = 9
[chains.ton.networks.testnet]
rpc_url = "https://testnet.toncenter.com/api/v2"
explorer_tx_url = "https://testnet.tonviewer.com/transaction/"
symbol = "TON"
decimals = 9

[chains.solana]
family = "svm"
[chains.solana.networks.mainnet]
rpc_url = "https://api.mainnet-beta.solana.com"
explorer_tx_url = "https://solscan.io/tx/"
symbol = "SOL"
decimals = 9
[chains.solana.networks.testnet]
rpc_url = "https://api.devnet.solana.com"
explorer_tx_url = "https://solscan.io/tx/{hash}?cluster=devnet"
symbol = "SOL"
decimals = 9

[chains.aptos]
family = "aptos"
[chains.aptos.networks.mainnet]
rpc_url = "https://fullnode.mainnet.aptoslabs.com"
explorer_tx_url = "https://explorer.aptoslabs.com/txn/{hash}?network=mainnet"
symbol = "APT"
decimals = 8
[chains.aptos.networks.testnet]
rpc_url = "https://fullnode.testnet.aptoslabs.com"
explorer_tx_url = "https://explorer.aptoslabs.com/txn/{hash}?network=testnet"
symbol = "APT"
decimals = 8

[chains.sui]
family = "sui"
[chains.sui.networks.mainnet]
rpc_url = "https://fullnode.mainnet.sui.io:443"
explorer_tx_url = "https://suiscan.xyz/mainnet/tx/"
symbol = "SUI"
decimals = 9
[chains.sui.networks.testnet]
rpc_url = "https://fullnode.testnet.sui.io:443"
explorer_tx_url = "https://suiscan.xyz/testnet/tx/"
symbol = "SUI"
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
    30
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
            "aptos" => ("APT", 8),
            "sui" => ("SUI", 9),
            "utxo" => ("BTC", 8),
            "ton" => ("TON", 9),
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
        self.explorer_tx_url.as_ref().map(|template| {
            if template.contains("{hash}") {
                template.replace("{hash}", tx_hash)
            } else {
                format!("{template}{tx_hash}")
            }
        })
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

        for (key, mainnet_id, testnet_id) in [
            ("eth", 1, 11155111),
            ("base", 8453, 84532),
            ("arb", 42161, 421614),
            ("bnb", 56, 97),
            ("pol", 137, 80002),
            ("hyperevm", 999, 998),
            ("abs", 2741, 11124),
        ] {
            let chain = &config.chains[key];
            assert_eq!(chain.family, "evm");
            assert_eq!(
                chain.resolve(key, "mainnet").unwrap().chain_id,
                Some(mainnet_id)
            );
            assert_eq!(
                chain.resolve(key, "testnet").unwrap().chain_id,
                Some(testnet_id)
            );
        }
        assert_eq!(config.chains["bnb"].resolve("bnb", "mainnet").unwrap().symbol, "BNB");

        let sol = config.chains["solana"].resolve("solana", "testnet").unwrap();
        assert_eq!((sol.symbol.as_str(), sol.decimals, sol.chain_id), ("SOL", 9, None));
        let apt = config.chains["aptos"].resolve("aptos", "mainnet").unwrap();
        assert_eq!((apt.symbol.as_str(), apt.decimals), ("APT", 8));
        assert_eq!(
            apt.explorer_link("0xabc").unwrap(),
            "https://explorer.aptoslabs.com/txn/0xabc?network=mainnet"
        );
        let btc = config.chains["btc"].resolve("btc", "mainnet").unwrap();
        assert_eq!((btc.family.as_str(), btc.symbol.as_str(), btc.decimals), ("utxo", "BTC", 8));
        let ton = config.chains["ton"].resolve("ton", "testnet").unwrap();
        assert_eq!((ton.family.as_str(), ton.symbol.as_str(), ton.decimals), ("ton", "TON", 9));
        let sui = config.chains["sui"].resolve("sui", "testnet").unwrap();
        assert_eq!((sui.symbol.as_str(), sui.decimals), ("SUI", 9));
        assert_eq!(
            sui.explorer_link("Digest123").unwrap(),
            "https://suiscan.xyz/testnet/tx/Digest123"
        );

        let missing = config.chains["eth"].resolve("eth", "localnet").unwrap_err();
        assert!(missing.to_string().contains("localnet"));
    }
}
