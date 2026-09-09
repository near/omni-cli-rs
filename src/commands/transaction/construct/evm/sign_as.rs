//! The execution-route fork: `derivation-path <path>` followed by
//! `sign-as-account` (your NEAR account calls the MPC signer right now) or
//! `sign-as-dao <dao>` (the request is wrapped in a SputnikDAO proposal).
//!
//! Everything upstream (chain, action, path) is shared; the route only
//! changes which NEAR transaction is submitted and what happens afterwards.

use std::sync::{Arc, Mutex};

use base64::Engine;
use color_eyre::eyre::{WrapErr, eyre};
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use super::EvmSpecContext;
use crate::chains::ExecutionLatency;
use crate::chains::evm;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = EvmSpecContext)]
#[interactive_clap(output_context = DerivationPathContext)]
pub struct DerivationPath {
    /// Derivation path (determines the acting foreign account, e.g. base-locker-admin):
    path: String,
    #[interactive_clap(subcommand)]
    sign_as: SignAs,
}

#[derive(Debug, Clone)]
pub struct DerivationPathContext {
    spec_context: EvmSpecContext,
    path: String,
}

impl DerivationPathContext {
    pub fn from_previous_context(
        previous_context: EvmSpecContext,
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

/// Resolves the derived EVM address and builds the unsigned transaction.
/// Shared by both routes; only the owner and the latency class differ.
fn build_unsigned_evm_tx(
    context: &EvmSpecContext,
    path: &str,
    owner: &near_primitives::types::AccountId,
    latency: ExecutionLatency,
    network_config: &near_cli_rs::config::NetworkConfig,
) -> color_eyre::eyre::Result<(omni_transaction::evm::EVMTransaction, [u8; 32])> {
    if context.chain.near_network != network_config.network_name {
        return Err(eyre!(
            "Chain '{}' is pinned to NEAR {}, but network-config '{}' was selected. \
             Pick a matching chain from the registry (or fix the registry entry).",
            context.chain_key,
            context.chain.near_network,
            network_config.network_name
        ));
    }

    let mpc_contract = crate::mpc::mpc_contract_id(&context.mpc_config, network_config)?;
    let api_network = crate::mpc::to_near_api_network(network_config)?;

    eprintln!(
        "\nResolving the derived address for {owner} / \"{path}\" via {mpc_contract} ..."
    );
    let derived_pk =
        crate::mpc::derived_public_key_secp256k1(&api_network, &mpc_contract, owner, path)?;
    let derived_address = evm::address_from_derived_pk(&derived_pk);

    let params = evm::fetch_tx_params(&context.chain, derived_address, &context.spec, latency)
        .wrap_err_with(|| {
            format!(
                "Failed to prepare the transaction on '{}' for derived sender {}",
                context.chain_key,
                evm::checksum(derived_address)
            )
        })?;
    let tx = evm::build_unsigned(context.chain.chain_id, &context.spec, &params);
    let payload = evm::sighash(&tx);

    eprintln!(
        "{}",
        evm::describe(
            &context.chain_key,
            &context.chain,
            &tx,
            owner.as_str(),
            path,
            derived_address,
            &context.spec,
        )
    );
    Ok((tx, payload))
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

#[derive(Debug, Clone)]
pub struct SignAsAccountContext {
    spec_context: EvmSpecContext,
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
            &context.spec_context.global_context.config.credentials_home_dir,
            "What NEAR account calls the MPC signer (owner of the derived foreign account)?",
        )
    }
}

impl From<SignAsAccountContext> for near_cli_rs::commands::ActionContext {
    fn from(item: SignAsAccountContext) -> Self {
        // Built in the prepopulated-transaction callback (once the network is
        // known), consumed in the after-sending callback to assemble and
        // broadcast the signed transaction.
        let unsigned_tx_holder: Arc<Mutex<Option<omni_transaction::evm::EVMTransaction>>> =
            Arc::new(Mutex::new(None));

        let get_prepopulated_transaction_after_getting_network_callback: near_cli_rs::commands::GetPrepopulatedTransactionAfterGettingNetworkCallback = {
            let spec_context = item.spec_context.clone();
            let path = item.path.clone();
            let owner = item.signer_account_id.clone();
            let unsigned_tx_holder = unsigned_tx_holder.clone();
            Arc::new(move |network_config| {
                let (tx, payload) = build_unsigned_evm_tx(
                    &spec_context,
                    &path,
                    &owner,
                    ExecutionLatency::Immediate,
                    network_config,
                )?;

                let tx_base64 = base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_vec(&tx)?);
                eprintln!(
                    "If the final broadcast fails, recover with:\n  \
                     omni broadcast <NEAR-TX-HASH> --unsigned-tx {tx_base64}\n"
                );
                *unsigned_tx_holder.lock().unwrap() = Some(tx);

                let mpc_contract =
                    crate::mpc::mpc_contract_id(&spec_context.mpc_config, network_config)?;
                Ok(near_cli_rs::commands::PrepopulatedTransaction {
                    signer_id: owner.clone(),
                    receiver_id: mpc_contract,
                    actions: vec![crate::mpc::sign_action(
                        payload,
                        &path,
                        &spec_context.mpc_config,
                    )],
                })
            })
        };

        let on_after_sending_transaction_callback: near_cli_rs::transaction_signature_options::OnAfterSendingTransactionCallback = {
            let chain = item.spec_context.chain.clone();
            let unsigned_tx_holder = unsigned_tx_holder.clone();
            Arc::new(move |outcome_view, _network_config| {
                let tx = unsigned_tx_holder
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("the unsigned transaction is always built before sending");
                let recovery_hint = || {
                    let tx_base64 = base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&tx).expect("EVMTransaction serializes"));
                    format!(
                        "omni broadcast {} --unsigned-tx {tx_base64}",
                        outcome_view.transaction.hash
                    )
                };

                let Some(response) =
                    crate::mpc::extract_signature_responses(outcome_view).into_iter().next()
                else {
                    eprintln!(
                        "The NEAR transaction succeeded, but no MPC signature was found in \
                         its receipts yet (the MPC responds asynchronously). Broadcast once \
                         it lands with:\n  {}",
                        recovery_hint()
                    );
                    return Ok(());
                };

                let signature = evm::signature_from_mpc(&response)?;
                let raw_tx = tx.build_with_signature(&signature);
                match evm::broadcast(&chain, &raw_tx) {
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
            on_before_signing_callback: Arc::new(|_prepopulated_unsigned_transaction, _network_config| Ok(())),
            on_before_sending_transaction_callback: Arc::new(|_signed_transaction, _network_config| {
                Ok(String::new())
            }),
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

#[derive(Debug, Clone)]
pub struct SignAsDaoContext {
    spec_context: EvmSpecContext,
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
            &context.spec_context.global_context.config.credentials_home_dir,
            "What is the SputnikDAO account ID (owner of the derived foreign account)?",
        )
    }

    pub fn input_proposer_account_id(
        context: &DerivationPathContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_signer_account_id_from_used_account_list(
            &context.spec_context.global_context.config.credentials_home_dir,
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
                let (tx, payload) = build_unsigned_evm_tx(
                    &spec_context,
                    &path,
                    &dao_account_id,
                    ExecutionLatency::Governance,
                    network_config,
                )?;

                let envelope = crate::envelope::Envelope {
                    omni: crate::envelope::VERSION,
                    family: crate::chains::evm::FAMILY.to_string(),
                    chain: spec_context.chain_key.clone(),
                    path: path.clone(),
                    unsigned_tx: serde_json::to_value(&tx)?,
                    intent: intent.clone(),
                    meta: crate::envelope::EnvelopeMeta {
                        nonce: Some(tx.nonce),
                        builder_version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                };
                let description = crate::envelope::encode_description(&envelope)?;

                let mpc_contract =
                    crate::mpc::mpc_contract_id(&spec_context.mpc_config, network_config)?;
                let api_network = crate::mpc::to_near_api_network(network_config)?;
                let proposal_bond = crate::dao::fetch_proposal_bond(&api_network, &dao_account_id)?;
                eprintln!(
                    "Proposal bond: {} yoctoNEAR (returned unless the proposal is rejected)\n",
                    proposal_bond
                );

                let sign_args = crate::mpc::sign_request_args(payload, &path);
                Ok(near_cli_rs::commands::PrepopulatedTransaction {
                    signer_id: proposer_account_id.clone(),
                    receiver_id: dao_account_id.clone(),
                    actions: vec![crate::dao::add_proposal_action(
                        &description,
                        &mpc_contract,
                        &sign_args,
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
                             omni broadcast <NEAR-TX-HASH>"
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
            on_before_signing_callback: Arc::new(|_prepopulated_unsigned_transaction, _network_config| Ok(())),
            on_before_sending_transaction_callback: Arc::new(|_signed_transaction, _network_config| {
                Ok(String::new())
            }),
            on_after_sending_transaction_callback,
            on_sending_delegate_action_callback: None,
            sign_as_delegate_action: false,
        }
    }
}
