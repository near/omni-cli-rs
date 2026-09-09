use std::sync::Arc;

use color_eyre::eyre::ContextCompat;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::ChainAdapter;
use crate::config::{ChainDef, MpcConfig};

pub mod evm;
pub mod sign_as;
pub mod svm;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
pub struct Construct {
    #[interactive_clap(subcommand)]
    family: Family,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the destination chain family:
pub enum Family {
    #[strum_discriminants(strum(
        message = "evm         -   EVM chains (Ethereum, Base, Arbitrum, ...)"
    ))]
    /// EVM chains (Ethereum, Base, Arbitrum, ...)
    Evm(self::evm::EvmChain),
    #[strum_discriminants(strum(
        message = "svm         -   SVM chains (Solana, Fogo, ...)"
    ))]
    /// SVM chains (Solana, Fogo, ...)
    Svm(self::svm::SvmChain),
}

/// Everything accumulated before the `derivation-path` step, family-erased:
/// the logical chain (endpoint resolves once the NEAR network is known) and
/// the adapter carrying the described action. Every family flow converges
/// here, and everything downstream is written once.
#[derive(Clone)]
pub struct SpecContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub chain_key: String,
    pub chain_def: ChainDef,
    pub mpc_config: MpcConfig,
    pub adapter: Arc<dyn ChainAdapter>,
}

/// A logical chain of the given family, loaded from the registry.
pub struct SelectedChain {
    pub chain_key: String,
    pub chain_def: ChainDef,
    pub mpc_config: MpcConfig,
}

pub fn load_chain(family: &str, chain_key: &str) -> color_eyre::eyre::Result<SelectedChain> {
    let omni_config = crate::config::load_or_init()?;
    let chain_def = omni_config
        .chains
        .get(chain_key)
        .wrap_err_with(|| {
            format!(
                "Chain '{chain_key}' is not in the omni chain registry (known {family} chains: {})",
                chain_keys_of_family(&omni_config, family).join(", ")
            )
        })?
        .clone();
    if chain_def.family != family {
        return Err(color_eyre::eyre::eyre!(
            "Chain '{chain_key}' is registered with family '{}', not '{family}'",
            chain_def.family
        ));
    }
    Ok(SelectedChain {
        chain_key: chain_key.to_string(),
        chain_def,
        mpc_config: omni_config.mpc,
    })
}

fn chain_keys_of_family(omni_config: &crate::config::OmniConfig, family: &str) -> Vec<String> {
    omni_config
        .chains
        .iter()
        .filter(|(_, def)| def.family == family)
        .map(|(key, _)| key.clone())
        .collect()
}

/// Interactive chain selection for one family. Shows the logical chain and
/// which NEAR networks it has variants for - the concrete endpoint resolves
/// later, when `network-config` is selected.
pub fn input_chain(family: &str) -> color_eyre::eyre::Result<Option<String>> {
    let omni_config = crate::config::load_or_init()?;
    let mut options: Vec<String> = omni_config
        .chains
        .iter()
        .filter(|(_, def)| def.family == family)
        .map(|(key, def)| {
            format!(
                "{key} (NEAR networks: {})",
                def.networks.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    options.sort();
    if options.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "No {family} chains in the omni chain registry - add a [chains.<name>] entry"
        ));
    }
    let selected = inquire::Select::new(&format!("Which {family} chain?"), options).prompt()?;
    let key = selected
        .split_whitespace()
        .next()
        .wrap_err("Failed to parse the selected chain")?
        .to_string();
    Ok(Some(key))
}
