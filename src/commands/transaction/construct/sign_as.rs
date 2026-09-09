//! The execution-route fork: `derivation-path <path>` followed by
//! `sign-as-account` (your NEAR account calls the MPC signer right now) or
//! `sign-as-dao <dao>` (the request is wrapped in a SputnikDAO proposal).
//!
//! Family-agnostic: everything chain-specific is behind the ChainAdapter in
//! the SpecContext, so every family flow shares this module unchanged.

use std::sync::{Arc, Mutex};

use base64::Engine;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use super::SpecContext;
use crate::chains::{BuiltTransaction, ExecutionLatency};
use crate::config::ResolvedChain;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SpecContext)]
#[interactive_clap(output_context = DerivationPathContext)]
pub struct DerivationPath {
    /// Derivation path (determines the acting foreign account, e.g. base-locker-admin):
    path: String,
    #[interactive_clap(subcommand)]
    sign_as: SignAs,
}

#[derive(Clone)]
pub struct DerivationPathContext {
    spec_context: SpecContext,
    path: String,
}

impl DerivationPathContext {
    pub fn from_previous_context(
        previous_context: SpecContext,
        scope: &<DerivationPath as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            spec_context: previous_context,
            path: scope.path.clone(),
        })
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = DerivationPathContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Who calls the MPC signer (the owner of the derived foreign account)?
pub enum SignAs {
    #[strum_discriminants(strum(
        message = "sign-as-account  -   Your NEAR account calls the MPC right now (sign + broadcast in one go)"
    ))]
    /// Your NEAR account calls the MPC right now (sign + broadcast in one go)
    SignAsAccount(SignAsAccount),
    #[strum_discriminants(strum(
        message = "sign-as-dao      -   Wrap the sign request in a SputnikDAO proposal (owner = the DAO)"
    ))]
    /// Wrap the sign request in a SputnikDAO proposal (owner = the DAO)
    SignAsDao(SignAsDao),
}

/// Resolves the chain against the selected NEAR network, fetches the derived
/// key for the adapter's scheme, and builds the unsigned transaction. Shared
/// by both routes; only the owner and the latency class differ.
fn build_unsigned_tx(
    context: &SpecContext,
    path: &str,
    owner: &near_primitives::types::AccountId,
    latency: ExecutionLatency,
    network_config: &near_cli_rs::config::NetworkConfig,
) -> color_eyre::eyre::Result<(BuiltTransaction, ResolvedChain)> {
    let chain = context
        .chain_def
        .resolve(&context.chain_key, &network_config.network_name)?;

    let mpc_contract = crate::mpc::mpc_contract_id(&context.mpc_config, network_config)?;
    let api_network = crate::mpc::to_near_api_network(network_config)?;
    let domain_id = crate::mpc::domain_id(&context.mpc_config, context.adapter.scheme());

    eprintln!(
        "\nResolving the derived {} address for {owner} / \"{path}\" via {mpc_contract} \
         (key domain {domain_id}) ...",
        chain.family
    );
    let derived_public_key =
        crate::mpc::derived_public_key(&api_network, &mpc_contract, owner, path, domain_id)?;
    eprintln!(
        "Derived {} address: {}",
        chain.chain_key,
        context.adapter.derived_address(&derived_public_key)?
    );

    let built =
        context
            .adapter
            .build(&chain, &derived_public_key, owner.as_str(), path, latency)?;
    eprintln!("{}", built.display);
    Ok((built, chain))
}

/// The recovery/envelope blob for a built transaction: the same Envelope
/// format the DAO route stores in proposal descriptions, base64-encoded.
fn envelope_blob(
    context: &SpecContext,
    path: &str,
    intent: &str,
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<String> {
    let envelope = crate::envelope::Envelope {
        omni: crate::envelope::VERSION,
        family: context.adapter.family().to_string(),
        chain: context.chain_key.clone(),
        path: path.to_string(),
        unsigned_tx: unsigned_tx.clone(),
        intent: intent.to_string(),
        meta: crate::envelope::EnvelopeMeta {
            nonce: None,
            builder_version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&envelope)?))
}

// ------------------------------------------------------------ sign-as-account

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = DerivationPathContext)]
#[interactive_clap(output_context = SignAsAccountContext)]
pub struct SignAsAccount {
    #[interactive_clap(skip_default_input_arg)]
    /// What NEAR account calls the MPC signer (owner of the derived foreign account)?
    signer_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network_for_transaction::NetworkForTransactionArgs,
}

#[derive(Clone)]
pub struct SignAsAccountContext {
    spec_context: SpecContext,
    path: String,
    signer_account_id: near_primitives::types::AccountId,
}

impl SignAsAccountContext {
    pub fn from_previous_context(
        previous_context: DerivationPathContext,
        scope: &<SignAsAccount as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            spec_context: previous_context.spec_context,
            path: previous_context.path,
            signer_account_id: scope.signer_account_id.clone().into(),
        })
    }
}

impl SignAsAccount {
    pub fn input_signer_account_id(
        context: &DerivationPathContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_signer_account_id_from_used_account_list(
            &context
                .spec_context
                .global_context
                .config
                .credentials_home_dir,
            "What NEAR account calls the MPC signer (owner of the derived foreign account)?",
        )
    }
}

impl From<SignAsAccountContext> for near_cli_rs::commands::ActionContext {
    fn from(item: SignAsAccountContext) -> Self {
        // Built in the prepopulated-transaction callback (once the network is
        // known), consumed in the after-sending callback to assemble and
        // broadcast the signed transaction.
        let unsigned_tx_holder: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));

        let get_prepopulated_transaction_after_getting_network_callback: near_cli_rs::commands::GetPrepopulatedTransactionAfterGettingNetworkCallback = {
            let spec_context = item.spec_context.clone();
            let path = item.path.clone();
            let owner = item.signer_account_id.clone();
            let unsigned_tx_holder = unsigned_tx_holder.clone();
            Arc::new(move |network_config| {
                let (built, _chain) = build_unsigned_tx(
                    &spec_context,
                    &path,
                    &owner,
                    ExecutionLatency::Immediate,
                    network_config,
                )?;

                let blob = envelope_blob(&spec_context, &path, "", &built.unsigned_tx)?;
                eprintln!(
                    "If the final broadcast fails, recover with:\n  \
                     omni transaction broadcast <NEAR-TX-HASH> {owner} --unsigned-tx {blob} \
                     network-config {network_name}\n",
                    network_name = network_config.network_name,
                );
                *unsigned_tx_holder.lock().unwrap() = Some(built.unsigned_tx.clone());

                let mpc_contract =
                    crate::mpc::mpc_contract_id(&spec_context.mpc_config, network_config)?;
                let actions = crate::mpc::sign_actions(
                    &built.payloads,
                    spec_context.adapter.scheme(),
                    &path,
                    &spec_context.mpc_config,
                );
                Ok(near_cli_rs::commands::PrepopulatedTransaction {
                    signer_id: owner.clone(),
                    receiver_id: mpc_contract,
                    actions,
                })
            })
        };

        let on_after_sending_transaction_callback: near_cli_rs::transaction_signature_options::OnAfterSendingTransactionCallback = {
            let spec_context = item.spec_context.clone();
            let path = item.path.clone();
            let unsigned_tx_holder = unsigned_tx_holder.clone();
            Arc::new(move |outcome_view, network_config| {
                let unsigned_tx = unsigned_tx_holder
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("the unsigned transaction is always built before sending");
                let chain = spec_context
                    .chain_def
                    .resolve(&spec_context.chain_key, &network_config.network_name)?;
                let recovery_hint = || {
                    let blob = envelope_blob(&spec_context, &path, "", &unsigned_tx)
                        .expect("the envelope was already encodable before sending");
                    format!(
                        "omni transaction broadcast {} {} --unsigned-tx {blob} network-config {}",
                        outcome_view.transaction.hash,
                        outcome_view.transaction.signer_id,
                        network_config.network_name,
                    )
                };

                let signatures = crate::mpc::extract_signature_responses(outcome_view);
                if signatures.is_empty() {
                    eprintln!(
                        "The NEAR transaction succeeded, but no MPC signature was found in \
                         its receipts yet (the MPC responds asynchronously). Broadcast once \
                         it lands with:\n  {}",
                        recovery_hint()
                    );
                    return Ok(());
                }

                match spec_context
                    .adapter
                    .assemble_and_broadcast(&chain, &unsigned_tx, &signatures)
                {
                    Ok(tx_hash) => {
                        eprintln!("\nBroadcast successful: {tx_hash}");
                        if let Some(link) = chain.explorer_link(&tx_hash) {
                            eprintln!("Explorer: {link}");
                        }
                        Ok(())
                    }
                    Err(err) => {
                        eprintln!(
                            "The MPC signature was produced, but broadcasting failed. \
                             Retry with:\n  {}",
                            recovery_hint()
                        );
                        Err(err)
                    }
                }
            })
        };

        Self {
            global_context: item.spec_context.global_context,
            interacting_with_account_ids: vec![item.signer_account_id],
            get_prepopulated_transaction_after_getting_network_callback,
            on_before_signing_callback: Arc::new(
                |_prepopulated_unsigned_transaction, _network_config| Ok(()),
            ),
            on_before_sending_transaction_callback: Arc::new(
                |_signed_transaction, _network_config| Ok(String::new()),
            ),
            on_after_sending_transaction_callback,
            on_sending_delegate_action_callback: None,
            sign_as_delegate_action: false,
        }
    }
}

// ---------------------------------------------------------------- sign-as-dao

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = DerivationPathContext)]
#[interactive_clap(output_context = SignAsDaoContext)]
pub struct SignAsDao {
    #[interactive_clap(skip_default_input_arg)]
    /// What is the SputnikDAO account ID (owner of the derived foreign account)?
    dao_account_id: near_cli_rs::types::account_id::AccountId,
    /// Short human-readable intent for reviewers (shown first in the proposal):
    intent: String,
    #[interactive_clap(skip_default_input_arg)]
    /// What NEAR account submits the proposal (must have AddProposal permission)?
    proposer_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network_for_transaction::NetworkForTransactionArgs,
}

#[derive(Clone)]
pub struct SignAsDaoContext {
    spec_context: SpecContext,
    path: String,
    dao_account_id: near_primitives::types::AccountId,
    intent: String,
    proposer_account_id: near_primitives::types::AccountId,
}

impl SignAsDaoContext {
    pub fn from_previous_context(
        previous_context: DerivationPathContext,
        scope: &<SignAsDao as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            spec_context: previous_context.spec_context,
            path: previous_context.path,
            dao_account_id: scope.dao_account_id.clone().into(),
            intent: scope.intent.clone(),
            proposer_account_id: scope.proposer_account_id.clone().into(),
        })
    }
}

impl SignAsDao {
    pub fn input_dao_account_id(
        context: &DerivationPathContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_non_signer_account_id_from_used_account_list(
            &context
                .spec_context
                .global_context
                .config
                .credentials_home_dir,
            "What is the SputnikDAO account ID (owner of the derived foreign account)?",
        )
    }

    pub fn input_proposer_account_id(
        context: &DerivationPathContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_signer_account_id_from_used_account_list(
            &context
                .spec_context
                .global_context
                .config
                .credentials_home_dir,
            "What NEAR account submits the proposal (must have AddProposal permission)?",
        )
    }
}

impl From<SignAsDaoContext> for near_cli_rs::commands::ActionContext {
    fn from(item: SignAsDaoContext) -> Self {
        let get_prepopulated_transaction_after_getting_network_callback: near_cli_rs::commands::GetPrepopulatedTransactionAfterGettingNetworkCallback = {
            let spec_context = item.spec_context.clone();
            let path = item.path.clone();
            let dao_account_id = item.dao_account_id.clone();
            let intent = item.intent.clone();
            let proposer_account_id = item.proposer_account_id.clone();
            Arc::new(move |network_config| {
                let (built, _chain) = build_unsigned_tx(
                    &spec_context,
                    &path,
                    &dao_account_id,
                    ExecutionLatency::Governance,
                    network_config,
                )?;

                let envelope = crate::envelope::Envelope {
                    omni: crate::envelope::VERSION,
                    family: spec_context.adapter.family().to_string(),
                    chain: spec_context.chain_key.clone(),
                    path: path.clone(),
                    unsigned_tx: built.unsigned_tx.clone(),
                    intent: intent.clone(),
                    meta: crate::envelope::EnvelopeMeta {
                        nonce: None,
                        builder_version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                };
                let description = crate::envelope::encode_description(&envelope)?;

                let mpc_contract =
                    crate::mpc::mpc_contract_id(&spec_context.mpc_config, network_config)?;
                let api_network = crate::mpc::to_near_api_network(network_config)?;
                let proposal_bond =
                    crate::dao::fetch_proposal_bond(&api_network, &dao_account_id)?;
                eprintln!(
                    "Proposal bond: {proposal_bond} yoctoNEAR (returned unless the \
                     proposal is rejected)\n"
                );

                let sign_args_list: Vec<serde_json::Value> = built
                    .payloads
                    .iter()
                    .map(|payload| {
                        crate::mpc::sign_request_args(
                            payload,
                            spec_context.adapter.scheme(),
                            &path,
                            &spec_context.mpc_config,
                        )
                    })
                    .collect();
                Ok(near_cli_rs::commands::PrepopulatedTransaction {
                    signer_id: proposer_account_id.clone(),
                    receiver_id: dao_account_id.clone(),
                    actions: vec![crate::dao::add_proposal_action(
                        &description,
                        &mpc_contract,
                        &sign_args_list,
                        &spec_context.mpc_config,
                        proposal_bond,
                    )],
                })
            })
        };

        let on_after_sending_transaction_callback: near_cli_rs::transaction_signature_options::OnAfterSendingTransactionCallback = {
            let dao_account_id = item.dao_account_id.clone();
            Arc::new(move |outcome_view, _network_config| {
                match crate::dao::proposal_id_from_outcome(outcome_view) {
                    Some(proposal_id) => {
                        eprintln!(
                            "\nProposal #{proposal_id} created on {dao_account_id}.\n\
                             Share with the other members for review:\n  \
                             omni proposal review {dao_account_id} {proposal_id}\n\
                             Once approved, the deciding vote's transaction hash finalizes it:\n  \
                             omni transaction broadcast <NEAR-TX-HASH> <voter-account-id>"
                        );
                    }
                    None => {
                        eprintln!(
                            "\nThe transaction succeeded, but no proposal id was found in the \
                             outcome - check the proposal list on {dao_account_id}."
                        );
                    }
                }
                Ok(())
            })
        };

        Self {
            global_context: item.spec_context.global_context,
            interacting_with_account_ids: vec![item.proposer_account_id, item.dao_account_id],
            get_prepopulated_transaction_after_getting_network_callback,
            on_before_signing_callback: Arc::new(
                |_prepopulated_unsigned_transaction, _network_config| Ok(()),
            ),
            on_before_sending_transaction_callback: Arc::new(
                |_signed_transaction, _network_config| Ok(String::new()),
            ),
            on_after_sending_transaction_callback,
            on_sending_delegate_action_callback: None,
            sign_as_delegate_action: false,
        }
    }
}
