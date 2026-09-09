//! `omni account`: inspect the foreign accounts derived from a NEAR account
//! (a DAO or a plain account) and a derivation path.

use std::collections::BTreeMap;

use color_eyre::eyre::{ContextCompat, WrapErr};
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::SignatureScheme;
use crate::config::ResolvedChain;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
pub struct Account {
    #[interactive_clap(subcommand)]
    account_actions: AccountActions,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum AccountActions {
    #[strum_discriminants(strum(
        message = "show      -   Derived addresses of an owner + path across all registered chains"
    ))]
    /// Derived addresses of an owner + path across all registered chains
    Show(Show),
    #[strum_discriminants(strum(
        message = "balance   -   Native balance of the derived address on one chain"
    ))]
    /// Native balance of the derived address on one chain
    Balance(Balance),
}

/// The MPC-derived keys of both domains for `(owner, path)` - enough to
/// compute the derived address on every registered chain.
struct DerivedKeys {
    secp256k1: [u8; 64],
    ed25519: [u8; 32],
}

fn fetch_derived_keys(
    network_config: &near_cli_rs::config::NetworkConfig,
    mpc_config: &crate::config::MpcConfig,
    owner: &near_primitives::types::AccountId,
    path: &str,
) -> color_eyre::eyre::Result<DerivedKeys> {
    let mpc_contract = crate::mpc::mpc_contract_id(mpc_config, network_config)?;
    let api_network = crate::mpc::to_near_api_network(network_config)?;
    eprintln!("\nResolving the derived keys for {owner} / \"{path}\" via {mpc_contract} ...");

    let secp256k1 = crate::mpc::secp256k1_bytes(&crate::mpc::derived_public_key(
        &api_network,
        &mpc_contract,
        owner,
        path,
        crate::mpc::domain_id(mpc_config, SignatureScheme::Secp256k1),
    )?)?;
    let ed25519 = crate::mpc::ed25519_bytes(&crate::mpc::derived_public_key(
        &api_network,
        &mpc_contract,
        owner,
        path,
        crate::mpc::domain_id(mpc_config, SignatureScheme::Ed25519),
    )?)?;
    Ok(DerivedKeys { secp256k1, ed25519 })
}

/// The registry's chains resolved for the selected NEAR network, keyed by
/// family (chains of one family share the derived address on that network).
fn resolved_chains_by_family(
    omni_config: &crate::config::OmniConfig,
    near_network: &str,
) -> BTreeMap<String, (ResolvedChain, Vec<String>)> {
    let mut families: BTreeMap<String, (ResolvedChain, Vec<String>)> = BTreeMap::new();
    for (chain_key, chain_def) in &omni_config.chains {
        if let Ok(resolved) = chain_def.resolve(chain_key, near_network) {
            families
                .entry(resolved.family.clone())
                .or_insert_with(|| (resolved, Vec::new()))
                .1
                .push(chain_key.clone());
        }
    }
    families
}

fn input_owner_account_id(
    context: &near_cli_rs::GlobalContext,
) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
    near_cli_rs::common::input_non_signer_account_id_from_used_account_list(
        &context.config.credentials_home_dir,
        "Which NEAR account owns the derived addresses (a DAO or a plain account)?",
    )
}

// ----------------------------------------------------------------------- show

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = ShowContext)]
pub struct Show {
    #[interactive_clap(skip_default_input_arg)]
    /// Which NEAR account owns the derived addresses (a DAO or a plain account)?
    owner_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(skip_default_input_arg)]
    /// Derivation path (determines the acting foreign account):
    path: String,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network::Network,
}

#[derive(Clone)]
pub struct ShowContext(near_cli_rs::network::NetworkContext);

impl ShowContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<Show as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let owner: near_primitives::types::AccountId = scope.owner_account_id.clone().into();
        let path = scope.path.clone();

        let on_after_getting_network_callback: near_cli_rs::network::OnAfterGettingNetworkCallback =
            std::sync::Arc::new(move |network_config| show(network_config, &owner, &path));

        Ok(Self(near_cli_rs::network::NetworkContext {
            config: previous_context.config,
            interacting_with_account_ids: vec![scope.owner_account_id.clone().into()],
            on_after_getting_network_callback,
        }))
    }
}

impl From<ShowContext> for near_cli_rs::network::NetworkContext {
    fn from(item: ShowContext) -> Self {
        item.0
    }
}

impl Show {
    pub fn input_owner_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        input_owner_account_id(context)
    }

    fn input_path(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::input_derivation_path()
    }
}

fn show(
    network_config: &near_cli_rs::config::NetworkConfig,
    owner: &near_primitives::types::AccountId,
    path: &str,
) -> near_cli_rs::CliResult {
    let omni_config = crate::config::load_or_init()?;
    let families = resolved_chains_by_family(&omni_config, &network_config.network_name);
    if families.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "No registered chain has a variant for NEAR network '{}'.",
            network_config.network_name
        ));
    }

    let keys = fetch_derived_keys(network_config, &omni_config.mpc, owner, path)?;

    eprintln!(
        "\nDerived addresses for {owner} / \"{path}\" (NEAR {}):\n\
         ------------------------------------------------------------",
        network_config.network_name
    );
    for (family, (resolved, chain_keys)) in &families {
        match crate::chains::derived_address_for_chain(resolved, &keys.secp256k1, &keys.ed25519) {
            Ok(address) => {
                eprintln!("{family:<7} {address}  ({})", chain_keys.join(", "));
            }
            Err(err) => eprintln!("{family:<7} <error: {err}>"),
        }
    }
    eprintln!("------------------------------------------------------------");
    Ok(())
}

// -------------------------------------------------------------------- balance

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = BalanceContext)]
pub struct Balance {
    #[interactive_clap(skip_default_input_arg)]
    /// Which NEAR account owns the derived addresses (a DAO or a plain account)?
    owner_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(skip_default_input_arg)]
    /// Derivation path (determines the acting foreign account):
    path: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Which chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network::Network,
}

#[derive(Clone)]
pub struct BalanceContext(near_cli_rs::network::NetworkContext);

impl BalanceContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<Balance as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let owner: near_primitives::types::AccountId = scope.owner_account_id.clone().into();
        let path = scope.path.clone();
        let chain_key = scope.chain.clone();

        let on_after_getting_network_callback: near_cli_rs::network::OnAfterGettingNetworkCallback =
            std::sync::Arc::new(move |network_config| {
                balance(network_config, &owner, &path, &chain_key)
            });

        Ok(Self(near_cli_rs::network::NetworkContext {
            config: previous_context.config,
            interacting_with_account_ids: vec![scope.owner_account_id.clone().into()],
            on_after_getting_network_callback,
        }))
    }
}

impl From<BalanceContext> for near_cli_rs::network::NetworkContext {
    fn from(item: BalanceContext) -> Self {
        item.0
    }
}

impl Balance {
    pub fn input_owner_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        input_owner_account_id(context)
    }

    fn input_path(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::input_derivation_path()
    }

    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        let omni_config = crate::config::load_or_init()?;
        let mut options: Vec<String> = omni_config.chains.keys().cloned().collect();
        options.sort();
        let selected = inquire::Select::new("Which chain?", options).prompt()?;
        Ok(Some(selected))
    }
}

fn balance(
    network_config: &near_cli_rs::config::NetworkConfig,
    owner: &near_primitives::types::AccountId,
    path: &str,
    chain_key: &str,
) -> near_cli_rs::CliResult {
    let omni_config = crate::config::load_or_init()?;
    let chain_def = omni_config
        .chains
        .get(chain_key)
        .wrap_err_with(|| format!("Chain '{chain_key}' is not in the omni chain registry"))?;
    let chain = chain_def.resolve(chain_key, &network_config.network_name)?;

    let keys = fetch_derived_keys(network_config, &omni_config.mpc, owner, path)?;
    let address = crate::chains::derived_address_for_chain(&chain, &keys.secp256k1, &keys.ed25519)?;
    let formatted =
        crate::chains::derived_balance_for_chain(&chain, &keys.secp256k1, &keys.ed25519)
            .wrap_err_with(|| format!("Failed to fetch the balance from {}", chain.rpc_url))?;

    eprintln!("\n{chain_key} balance of {address} ({owner} / \"{path}\"): {formatted}");
    Ok(())
}
