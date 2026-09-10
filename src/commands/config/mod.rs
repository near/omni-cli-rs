//! `omni config`: manage the omni chain registry (omni-config.toml) without
//! hand-editing TOML - show it, add/remove chains, pull in newly shipped
//! default chains, or reset to the defaults.

use std::collections::BTreeMap;

use color_eyre::eyre::{WrapErr, eyre};
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::config::{ChainDef, NetworkVariant, family_defaults};

/// The NEAR networks a chain entry can carry variants for.
const NEAR_NETWORKS: [&str; 2] = ["mainnet", "testnet"];

const FAMILIES: [&str; 6] = ["evm", "svm", "utxo", "aptos", "sui", "ton"];

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
pub struct Config {
    #[interactive_clap(subcommand)]
    config_actions: ConfigActions,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum ConfigActions {
    #[strum_discriminants(strum(
        message = "show           -   Where the registry lives and what it holds"
    ))]
    /// Where the registry lives and what it holds
    Show(Show),
    #[strum_discriminants(strum(
        message = "add-chain      -   Add a chain to the registry (or reconfigure one)"
    ))]
    /// Add a chain to the registry (or reconfigure one)
    AddChain(AddChain),
    #[strum_discriminants(strum(message = "remove-chain   -   Remove a chain from the registry"))]
    /// Remove a chain from the registry
    RemoveChain(RemoveChain),
    #[strum_discriminants(strum(
        message = "sync           -   Add default chains this CLI version ships that your registry is missing"
    ))]
    /// Add default chains this CLI version ships that your registry is missing
    Sync(Sync),
    #[strum_discriminants(strum(
        message = "reset          -   Restore the default registry (the old file is backed up)"
    ))]
    /// Restore the default registry (the old file is backed up)
    Reset(Reset),
}

// ----------------------------------------------------------------------- show

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = ShowContext)]
pub struct Show;

#[derive(Debug, Clone)]
pub struct ShowContext;

impl ShowContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        _scope: &<Show as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let config = crate::config::load_or_init()?;
        let path = crate::config::config_path()?;

        eprintln!("\nomni chain registry: {}", path.display());
        eprintln!(
            "default derivation path: \"{}\"",
            config.default_derivation_path
        );
        eprintln!(
            "mpc: {} TGas / {} yoctoNEAR per sign, domains secp256k1={} ed25519={}{}",
            config.mpc.sign_gas_tgas,
            config.mpc.sign_deposit_yoctonear,
            config.mpc.secp256k1_domain_id,
            config.mpc.ed25519_domain_id,
            if config.mpc.contracts.is_empty() {
                " (signer contracts from the near-cli-rs network connections)".to_string()
            } else {
                format!(", signer contracts: {:?}", config.mpc.contracts)
            },
        );
        eprintln!(
            "\n{} chain(s):\n------------------------------------------------------------",
            config.chains.len()
        );
        for (key, chain) in &config.chains {
            eprintln!("{key:<10} [{}]", chain.family);
            for (network, variant) in &chain.networks {
                eprintln!(
                    "  {network:<9} {}{}",
                    variant.rpc_url,
                    variant
                        .chain_id
                        .map(|id| format!(" (chain id {id})"))
                        .unwrap_or_default(),
                );
            }
        }
        eprintln!("------------------------------------------------------------");
        Ok(Self)
    }
}

// ------------------------------------------------------------------ add-chain

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = AddChainContext)]
pub struct AddChain {
    /// Chain key for the registry (e.g. "base", "op", "fogo"):
    chain_key: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Chain family (evm, svm, utxo, aptos, sui, ton):
    family: String,
}

#[derive(Debug, Clone)]
pub struct AddChainContext;

impl AddChain {
    fn input_family(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        Ok(Some(
            inquire::Select::new("Chain family:", FAMILIES.map(String::from).to_vec()).prompt()?,
        ))
    }
}

impl AddChainContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        scope: &<AddChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let chain_key = scope.chain_key.trim().to_lowercase();
        let family = scope.family.trim().to_lowercase();
        if !FAMILIES.contains(&family.as_str()) {
            return Err(eyre!(
                "Unknown chain family '{family}' (expected one of: {})",
                FAMILIES.join(", ")
            ));
        }
        if chain_key.is_empty()
            || !chain_key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(eyre!(
                "Chain key '{chain_key}' must be non-empty ascii alphanumeric (plus - and _)"
            ));
        }

        let mut config = crate::config::load_or_init()?;
        let existing = config.chains.get(&chain_key);
        if let Some(existing) = existing {
            if existing.family != family {
                return Err(eyre!(
                    "Chain '{chain_key}' already exists with family '{}' - remove it first \
                     to change the family",
                    existing.family
                ));
            }
            eprintln!("\nChain '{chain_key}' already exists - reconfiguring its networks.");
        }

        // The endpoint details are interactive by design: a chain entry is a
        // small tree (per-NEAR-network variants), not a flat flag list.
        let mut networks: BTreeMap<String, NetworkVariant> = BTreeMap::new();
        let (default_symbol, default_decimals) = family_defaults(&family);
        for near_network in NEAR_NETWORKS {
            let existing_variant =
                existing.and_then(|existing| existing.networks.get(near_network));
            let configure = inquire::Confirm::new(&format!(
                "Configure the {near_network} variant (used when `network-config {near_network}` is selected)?"
            ))
            .with_default(existing_variant.is_some() || near_network == "mainnet")
            .prompt()?;
            if !configure {
                continue;
            }

            let rpc_message = format!("{near_network} RPC URL:");
            let mut rpc_prompt = inquire::Text::new(&rpc_message);
            let existing_rpc = existing_variant.map(|variant| variant.rpc_url.clone());
            if let Some(existing_rpc) = &existing_rpc {
                rpc_prompt = rpc_prompt.with_initial_value(existing_rpc);
            }
            let rpc_url = rpc_prompt.prompt()?.trim().to_string();
            let _: reqwest::Url = rpc_url
                .parse()
                .wrap_err_with(|| format!("Invalid RPC URL: '{rpc_url}'"))?;

            let chain_id = if family == "evm" {
                Some(
                    inquire::CustomType::<u64>::new(&format!(
                        "{near_network} EVM chain id (see chainlist.org):"
                    ))
                    .prompt()?,
                )
            } else {
                None
            };

            let explorer = inquire::Text::new(&format!(
                "{near_network} explorer tx URL (prefix, or template with {{hash}}; empty to skip):"
            ))
            .with_initial_value(
                existing_variant
                    .and_then(|variant| variant.explorer_tx_url.as_deref())
                    .unwrap_or(""),
            )
            .prompt()?;
            let explorer_tx_url = Some(explorer.trim().to_string()).filter(|s| !s.is_empty());

            let symbol = inquire::Text::new("Native token symbol:")
                .with_initial_value(
                    existing_variant
                        .and_then(|variant| variant.symbol.as_deref())
                        .unwrap_or(default_symbol),
                )
                .prompt()?
                .trim()
                .to_string();
            let decimals = inquire::CustomType::<u8>::new("Native token decimals:")
                .with_default(
                    existing_variant
                        .and_then(|variant| variant.decimals)
                        .unwrap_or(default_decimals),
                )
                .prompt()?;

            networks.insert(
                near_network.to_string(),
                NetworkVariant {
                    rpc_url,
                    chain_id,
                    explorer_tx_url,
                    symbol: Some(symbol),
                    decimals: Some(decimals),
                },
            );
        }
        if networks.is_empty() {
            return Err(eyre!(
                "No network variant was configured - the chain entry was not saved."
            ));
        }

        let chain = ChainDef {
            family: family.clone(),
            networks,
        };
        let snippet = toml::to_string_pretty(&BTreeMap::from([(
            "chains".to_string(),
            BTreeMap::from([(chain_key.clone(), chain.clone())]),
        )]))?;
        config.chains.insert(chain_key.clone(), chain);

        let backup = crate::config::backup_config_file()?;
        let path = crate::config::save(&config)?;
        eprintln!("\nSaved to {}:\n\n{snippet}", path.display());
        if let Some(backup) = backup {
            eprintln!("(previous config backed up to {})", backup.display());
        }
        eprintln!(
            "Use it with: omni transaction construct {family} {chain_key} ...\n\
             Check the derived address first: omni account show"
        );
        Ok(Self)
    }
}

// --------------------------------------------------------------- remove-chain

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = RemoveChainContext)]
pub struct RemoveChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Chain key to remove from the registry:
    chain_key: String,
}

#[derive(Debug, Clone)]
pub struct RemoveChainContext;

impl RemoveChain {
    fn input_chain_key(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        let config = crate::config::load_or_init()?;
        let keys: Vec<String> = config.chains.keys().cloned().collect();
        if keys.is_empty() {
            return Err(eyre!("The chain registry is empty - nothing to remove."));
        }
        Ok(Some(
            inquire::Select::new("Which chain should be removed?", keys).prompt()?,
        ))
    }
}

impl RemoveChainContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        scope: &<RemoveChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let mut config = crate::config::load_or_init()?;
        if config.chains.remove(&scope.chain_key).is_none() {
            return Err(eyre!(
                "Chain '{}' is not in the registry (configured: {})",
                scope.chain_key,
                config.chains.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let backup = crate::config::backup_config_file()?;
        let path = crate::config::save(&config)?;
        eprintln!(
            "\nRemoved chain '{}' from {}.",
            scope.chain_key,
            path.display()
        );
        if let Some(backup) = backup {
            eprintln!("(previous config backed up to {})", backup.display());
        }
        Ok(Self)
    }
}

// ----------------------------------------------------------------------- sync

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = SyncContext)]
pub struct Sync;

#[derive(Debug, Clone)]
pub struct SyncContext;

impl SyncContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        _scope: &<Sync as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let mut config = crate::config::load_or_init()?;
        let added = merge_missing_default_chains(&mut config);
        if added.is_empty() {
            eprintln!(
                "\nAlready up to date: the registry has every default chain this CLI \
                 version ships."
            );
            return Ok(Self);
        }
        let backup = crate::config::backup_config_file()?;
        let path = crate::config::save(&config)?;
        eprintln!(
            "\nAdded {} default chain(s) to {}: {}",
            added.len(),
            path.display(),
            added.join(", ")
        );
        if let Some(backup) = backup {
            eprintln!("(previous config backed up to {})", backup.display());
        }
        Ok(Self)
    }
}

/// Inserts default chains missing from the registry; existing entries
/// (including modified defaults) are never touched. Returns the added keys.
fn merge_missing_default_chains(config: &mut crate::config::OmniConfig) -> Vec<String> {
    let mut added = Vec::new();
    for (key, chain) in crate::config::default_config().chains {
        if !config.chains.contains_key(&key) {
            config.chains.insert(key.clone(), chain);
            added.push(key);
        }
    }
    added
}

// ---------------------------------------------------------------------- reset

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = ResetContext)]
pub struct Reset;

#[derive(Debug, Clone)]
pub struct ResetContext;

impl ResetContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        _scope: &<Reset as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let backup = crate::config::backup_config_file()?;
        let path = crate::config::write_default_config_file()?;
        eprintln!(
            "\nRestored the default chain registry at {}.",
            path.display()
        );
        if let Some(backup) = backup {
            eprintln!("Your previous config is backed up at {}.", backup.display());
        }
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_adds_only_missing_default_chains() {
        let mut config = crate::config::default_config();
        // A user config missing one default chain, with a customized one kept.
        config.chains.remove("fogo").unwrap();
        let custom_rpc = "https://my-own-eth-node.example.com";
        config
            .chains
            .get_mut("eth")
            .unwrap()
            .networks
            .get_mut("mainnet")
            .unwrap()
            .rpc_url = custom_rpc.to_string();

        let added = merge_missing_default_chains(&mut config);
        assert_eq!(added, vec!["fogo".to_string()]);
        assert_eq!(
            config.chains["eth"].networks["mainnet"].rpc_url, custom_rpc,
            "sync must not overwrite user-modified chains"
        );

        assert!(merge_missing_default_chains(&mut config).is_empty());
    }
}
