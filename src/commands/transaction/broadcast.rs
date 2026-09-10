//! `omni transaction broadcast <near-tx-hash>`: turns a completed MPC
//! signature on NEAR into a transaction on the destination chain, no matter
//! who or what triggered it - the deciding `act_proposal` vote of a DAO
//! proposal, or a direct `sign` call whose broadcast failed or was
//! interrupted (recovery via `--unsigned-tx`).

use base64::Engine;
use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use color_eyre::owo_colors::OwoColorize;

use crate::envelope::Envelope;
use crate::mpc::MpcSignatureResponse;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = BroadcastContext)]
pub struct Broadcast {
    /// NEAR transaction hash that produced the MPC signature (the deciding act_proposal vote, or a direct sign call):
    tx_hash: near_cli_rs::types::crypto_hash::CryptoHash,
    #[interactive_clap(skip_default_input_arg)]
    /// Who signed that NEAR transaction? (used by the RPC to locate it)
    tx_signer_account_id: near_cli_rs::types::account_id::AccountId,
    /// Base64 envelope with the unsigned transaction (echoed by `construct` for direct sign calls; not needed for DAO proposals)
    #[interactive_clap(long)]
    #[interactive_clap(skip_interactive_input)]
    unsigned_tx: Option<String>,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network::Network,
}

#[derive(Clone)]
pub struct BroadcastContext(near_cli_rs::network::NetworkContext);

impl BroadcastContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<Broadcast as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let tx_hash = scope.tx_hash.to_string();
        let tx_signer: near_primitives::types::AccountId =
            scope.tx_signer_account_id.clone().into();
        let unsigned_tx_arg = scope.unsigned_tx.clone();

        let on_after_getting_network_callback: near_cli_rs::network::OnAfterGettingNetworkCallback =
            std::sync::Arc::new(move |network_config| {
                broadcast(
                    network_config,
                    &tx_hash,
                    &tx_signer,
                    unsigned_tx_arg.as_deref(),
                )
            });

        Ok(Self(near_cli_rs::network::NetworkContext {
            config: previous_context.config,
            interacting_with_account_ids: vec![scope.tx_signer_account_id.clone().into()],
            on_after_getting_network_callback,
        }))
    }
}

impl From<BroadcastContext> for near_cli_rs::network::NetworkContext {
    fn from(item: BroadcastContext) -> Self {
        item.0
    }
}

impl Broadcast {
    pub fn input_tx_signer_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_signer_account_id_from_used_account_list(
            &context.config.credentials_home_dir,
            "Who signed that NEAR transaction? (used by the RPC to locate it)",
        )
    }
}

fn broadcast(
    network_config: &near_cli_rs::config::NetworkConfig,
    tx_hash: &str,
    tx_signer: &near_primitives::types::AccountId,
    unsigned_tx_arg: Option<&str>,
) -> near_cli_rs::CliResult {
    let api_network = crate::mpc::to_near_api_network(network_config)?;

    crate::output::info(format!("Fetching the NEAR transaction {tx_hash} ..."));
    let result = crate::mpc::block_on(
        near_api::Transaction::status_with_options(
            tx_signer
                .as_str()
                .parse()
                .wrap_err("Invalid NEAR signer account id")?,
            tx_hash
                .parse()
                .map_err(|err| eyre!("Invalid NEAR transaction hash '{tx_hash}': {err:?}"))?,
            near_api::types::TxExecutionStatus::Final,
        )
        .fetch_from(&api_network),
    )?
    .wrap_err_with(|| format!("Failed to fetch NEAR transaction {tx_hash}"))?;

    // Collect MPC signatures from the receipt values
    let signatures: Vec<MpcSignatureResponse> = result
        .receipt_outcomes()
        .iter()
        .filter_map(|outcome| match outcome.clone().into_result() {
            Ok(near_api::types::transaction::result::ValueOrReceiptId::Value(value)) => {
                value.raw_bytes().ok()
            }
            _ => None,
        })
        .filter_map(|bytes| crate::mpc::parse_signature_response(&bytes))
        .collect();

    // Locate the envelope: the --unsigned-tx blob, or the DAO proposal the
    // act_proposal transaction voted on.
    let envelope = match unsigned_tx_arg {
        Some(blob) => decode_envelope_blob(blob)?,
        None => envelope_from_dao_proposal(&api_network, &result)?,
    };

    crate::output::info(format!(
        "Finalizing: [{}/{}] {} (derivation path \"{}\"){}",
        envelope.family,
        envelope.chain,
        if envelope.intent.is_empty() {
            "(no intent)"
        } else {
            &envelope.intent
        },
        envelope.path,
        if signatures.is_empty() {
            ""
        } else {
            "\nMPC signature found in the receipts."
        },
    ));

    if signatures.is_empty() {
        return Err(eyre!(
            "No MPC signature was found in the receipts of {tx_hash}. Either the \
             proposal has not reached the approval threshold yet, this vote was not \
             the deciding one, or the MPC is still responding - check the proposal \
             status and retry with the transaction hash of the deciding vote."
        ));
    }

    let omni_config = crate::config::load_or_init()?;
    let chain_def = omni_config.chains.get(&envelope.chain).wrap_err_with(|| {
        format!(
            "Chain '{}' from the envelope is not in the omni chain registry",
            envelope.chain
        )
    })?;
    let chain = chain_def.resolve(&envelope.chain, &network_config.network_name)?;
    if chain.family != envelope.family {
        return Err(eyre!(
            "The envelope says family '{}', but chain '{}' is registered as '{}'",
            envelope.family,
            envelope.chain,
            chain.family
        ));
    }

    let foreign_tx_hash =
        crate::chains::assemble_and_broadcast(&chain, &envelope.unsigned_tx, &signatures)?;
    eprintln!("\n{} {foreign_tx_hash}", "Broadcast successful:".green());
    if let Some(link) = chain.explorer_link(&foreign_tx_hash) {
        eprintln!("Explorer: {}", link.cyan());
    }
    Ok(())
}

fn decode_envelope_blob(blob: &str) -> color_eyre::eyre::Result<Envelope> {
    let json = base64::engine::general_purpose::STANDARD
        .decode(blob.trim())
        .wrap_err("--unsigned-tx is not valid base64")?;
    serde_json::from_slice(&json).wrap_err("--unsigned-tx is not a valid omni envelope")
}

/// For an `act_proposal` transaction: find the DAO and proposal id in the
/// transaction's actions, fetch the proposal, and decode the envelope from
/// its description.
fn envelope_from_dao_proposal(
    api_network: &near_api::NetworkConfig,
    result: &near_api::types::transaction::result::ExecutionFinalResult,
) -> color_eyre::eyre::Result<Envelope> {
    let transaction = result.transaction();
    let dao_account_id = transaction.receiver_id().clone();

    let proposal_id = transaction
        .actions()
        .iter()
        .find_map(|action| match action {
            near_api::types::Action::FunctionCall(call) if call.method_name == "act_proposal" => {
                serde_json::from_slice::<serde_json::Value>(&call.args)
                    .ok()?
                    .get("id")?
                    .as_u64()
            }
            _ => None,
        })
        .wrap_err(
            "This NEAR transaction is not an act_proposal vote, so the unsigned \
             transaction cannot be recovered from a DAO proposal. For direct sign \
             calls, pass the envelope echoed by `construct` via --unsigned-tx.",
        )?;

    crate::output::info(format!(
        "Found act_proposal: proposal #{proposal_id} on {dao_account_id}"
    ));
    let dao: near_primitives::types::AccountId = dao_account_id
        .as_str()
        .parse()
        .wrap_err("Invalid DAO account id")?;
    let proposal = crate::dao::fetch_proposal(api_network, &dao, proposal_id)?;

    let description = proposal
        .get("description")
        .and_then(|description| description.as_str())
        .wrap_err("The proposal has no description")?;
    crate::envelope::decode_description(description).wrap_err(
        "The proposal description carries no omni envelope - was this proposal \
         created by omni-cli?",
    )
}
