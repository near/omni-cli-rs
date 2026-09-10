//! `omni proposal`: the DAO-route lifecycle - list omni proposals, review one
//! (recompute the signing payloads from the envelope and byte-compare them
//! against the proposal's `sign` args - the trust anchor), and vote.

use base64::Engine;
use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use color_eyre::owo_colors::OwoColorize;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::envelope::Envelope;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
pub struct Proposal {
    #[interactive_clap(subcommand)]
    proposal_actions: ProposalActions,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum ProposalActions {
    #[strum_discriminants(strum(
        message = "list     -   Recent proposals on a DAO (omni proposals decoded)"
    ))]
    /// Recent proposals on a DAO (omni proposals decoded)
    List(List),
    #[strum_discriminants(strum(
        message = "review   -   Verify an omni proposal: recompute the signing payloads and compare byte-for-byte"
    ))]
    /// Verify an omni proposal: recompute the signing payloads and compare byte-for-byte
    Review(Review),
    #[strum_discriminants(strum(message = "vote     -   Vote on a proposal (approve/reject)"))]
    /// Vote on a proposal (approve/reject)
    Vote(Vote),
}

fn input_dao_account_id(
    context: &near_cli_rs::GlobalContext,
) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
    near_cli_rs::common::input_non_signer_account_id_from_used_account_list(
        &context.config.credentials_home_dir,
        "What is the SputnikDAO account ID?",
    )
}

/// One `sign` request parsed out of a proposal's FunctionCall actions.
struct ParsedSignRequest {
    path: String,
    domain_id: u64,
    payload: Vec<u8>,
}

/// Parses and structurally validates the proposal kind: it must be a
/// FunctionCall to `expected_mpc` whose actions are all `sign` calls.
fn parse_sign_actions(
    kind: &serde_json::Value,
    expected_mpc: &near_primitives::types::AccountId,
) -> color_eyre::eyre::Result<Vec<ParsedSignRequest>> {
    let function_call = kind
        .get("FunctionCall")
        .wrap_err("The proposal is not a FunctionCall proposal")?;
    let receiver = function_call["receiver_id"]
        .as_str()
        .wrap_err("The proposal has no receiver_id")?;
    if receiver != expected_mpc.as_str() {
        return Err(eyre!(
            "The proposal calls '{receiver}', not the MPC signer contract \
             '{expected_mpc}' - this is NOT a chain-signature request."
        ));
    }

    let actions = function_call["actions"]
        .as_array()
        .wrap_err("The proposal has no actions")?;
    actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let method = action["method_name"].as_str().unwrap_or_default();
            if method != "sign" {
                return Err(eyre!(
                    "Action #{index} calls '{method}', not 'sign' - this is NOT a \
                     plain chain-signature request."
                ));
            }
            let args_base64 = action["args"]
                .as_str()
                .wrap_err_with(|| format!("Action #{index} has no args"))?;
            let args: serde_json::Value = serde_json::from_slice(
                &base64::engine::general_purpose::STANDARD
                    .decode(args_base64)
                    .wrap_err_with(|| format!("Action #{index} args are not base64"))?,
            )
            .wrap_err_with(|| format!("Action #{index} args are not JSON"))?;
            let request = &args["request"];
            let path = request["path"]
                .as_str()
                .wrap_err_with(|| format!("Action #{index} has no request.path"))?
                .to_string();
            let domain_id = request["domain_id"]
                .as_u64()
                .wrap_err_with(|| format!("Action #{index} has no request.domain_id"))?;
            let payload_hex = request["payload_v2"]["Ecdsa"]
                .as_str()
                .or_else(|| request["payload_v2"]["Eddsa"].as_str())
                .wrap_err_with(|| {
                    format!("Action #{index} has no request.payload_v2 (Ecdsa/Eddsa)")
                })?;
            let payload = hex::decode(payload_hex)
                .wrap_err_with(|| format!("Action #{index} payload is not hex"))?;
            Ok(ParsedSignRequest {
                path,
                domain_id,
                payload,
            })
        })
        .collect()
}

/// The trust anchor: every claim the envelope makes must match what the DAO
/// would actually ask the MPC to sign. Returns the human-readable list of
/// passed checks; any mismatch is an error.
fn verify_envelope_against_kind(
    envelope: &Envelope,
    kind: &serde_json::Value,
    expected_mpc: &near_primitives::types::AccountId,
    expected_domain: u64,
) -> color_eyre::eyre::Result<Vec<String>> {
    let mut checks = Vec::new();

    let requests = parse_sign_actions(kind, expected_mpc)?;
    checks.push(format!(
        "receiver is the MPC signer contract ({expected_mpc})"
    ));
    checks.push(format!(
        "{} action(s), all plain `sign` calls",
        requests.len()
    ));

    for (index, request) in requests.iter().enumerate() {
        if request.path != envelope.path {
            return Err(eyre!(
                "Action #{index} signs with derivation path '{}', but the envelope \
                 says '{}' - a DIFFERENT foreign account would sign.",
                request.path,
                envelope.path
            ));
        }
        if request.domain_id != expected_domain {
            return Err(eyre!(
                "Action #{index} uses key domain {}, but family '{}' signs with \
                 domain {expected_domain}.",
                request.domain_id,
                envelope.family
            ));
        }
    }
    checks.push(format!(
        "derivation path matches the envelope (\"{}\")",
        envelope.path
    ));
    checks.push(format!(
        "key domain matches the '{}' family ({expected_domain})",
        envelope.family
    ));

    let recomputed =
        crate::chains::signing_payloads_from_envelope(&envelope.family, &envelope.unsigned_tx)?;
    if recomputed.len() != requests.len() {
        return Err(eyre!(
            "The envelope's transaction needs {} signature(s), but the proposal \
             requests {}.",
            recomputed.len(),
            requests.len()
        ));
    }
    for (index, (recomputed_payload, request)) in recomputed.iter().zip(requests.iter()).enumerate()
    {
        if *recomputed_payload != request.payload {
            return Err(eyre!(
                "Payload #{index} does NOT match the envelope's transaction:\n  \
                 proposal signs: {}\n  envelope needs: {}\n\
                 The MPC would sign something other than what the envelope shows.",
                hex::encode(&request.payload),
                hex::encode(recomputed_payload)
            ));
        }
    }
    checks.push(format!(
        "{} signing payload(s) match the envelope byte-for-byte",
        recomputed.len()
    ));

    Ok(checks)
}

/// One table row per proposal: decoded envelope summary for omni proposals,
/// a terse kind/description fallback for everything else.
fn proposal_row(proposal: &serde_json::Value) -> prettytable::Row {
    let id = proposal["id"].as_u64().unwrap_or_default();
    let status = proposal["status"].as_str().unwrap_or("?");
    let status_cell = prettytable::Cell::new(status).style_spec(match status {
        "Approved" => "Fg",
        "Rejected" | "Removed" | "Expired" | "Failed" => "Fr",
        "InProgress" => "Fy",
        _ => "",
    });
    let description = proposal["description"].as_str().unwrap_or_default();
    let (chain, intent, path) =
        if let Some(envelope) = crate::envelope::decode_description(description) {
            (
                format!("{}/{}", envelope.family, envelope.chain),
                if envelope.intent.is_empty() {
                    "(no intent)".to_string()
                } else {
                    envelope.intent
                },
                envelope.path,
            )
        } else {
            let kind = proposal["kind"]
                .as_object()
                .and_then(|kind| kind.keys().next().cloned())
                .or_else(|| proposal["kind"].as_str().map(str::to_string))
                .unwrap_or_else(|| "?".to_string());
            (
                kind,
                description.chars().take(48).collect(),
                "-".to_string(),
            )
        };
    prettytable::Row::new(vec![
        prettytable::Cell::new(&id.to_string()),
        status_cell,
        prettytable::Cell::new(&chain),
        prettytable::Cell::new(&intent),
        prettytable::Cell::new(&path),
    ])
}

// ----------------------------------------------------------------------- list

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = ListContext)]
pub struct List {
    #[interactive_clap(skip_default_input_arg)]
    /// What is the SputnikDAO account ID?
    dao_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network::Network,
}

#[derive(Clone)]
pub struct ListContext(near_cli_rs::network::NetworkContext);

impl ListContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<List as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let dao: near_primitives::types::AccountId = scope.dao_account_id.clone().into();
        let on_after_getting_network_callback: near_cli_rs::network::OnAfterGettingNetworkCallback =
            std::sync::Arc::new(move |network_config| list(network_config, &dao));
        Ok(Self(near_cli_rs::network::NetworkContext {
            config: previous_context.config,
            interacting_with_account_ids: vec![scope.dao_account_id.clone().into()],
            on_after_getting_network_callback,
        }))
    }
}

impl From<ListContext> for near_cli_rs::network::NetworkContext {
    fn from(item: ListContext) -> Self {
        item.0
    }
}

impl List {
    pub fn input_dao_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        input_dao_account_id(context)
    }
}

const LIST_PAGE_SIZE: u64 = 20;

fn list(
    network_config: &near_cli_rs::config::NetworkConfig,
    dao: &near_primitives::types::AccountId,
) -> near_cli_rs::CliResult {
    let api_network = crate::mpc::to_near_api_network(network_config)?;
    let last_id = crate::dao::fetch_last_proposal_id(&api_network, dao)?;
    if last_id == 0 {
        eprintln!("\n{dao} has no proposals yet.");
        return Ok(());
    }
    let from_index = last_id.saturating_sub(LIST_PAGE_SIZE);
    let proposals = crate::dao::fetch_proposals(&api_network, dao, from_index, LIST_PAGE_SIZE)?;

    eprintln!(
        "\nLast {} of {last_id} proposal(s) on {dao}:",
        proposals.len()
    );
    let mut table = crate::commands::new_table();
    table.set_titles(crate::commands::title_row(&[
        "#",
        "Status",
        "Chain / kind",
        "Intent / description",
        "Path",
    ]));
    for proposal in &proposals {
        table.add_row(proposal_row(proposal));
    }
    table.printstd();
    eprintln!(
        "Review one with: {}",
        format!(
            "omni proposal review {dao} <id> network-config {}",
            network_config.network_name
        )
        .yellow()
    );
    Ok(())
}

// --------------------------------------------------------------------- review

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = ReviewContext)]
pub struct Review {
    #[interactive_clap(skip_default_input_arg)]
    /// What is the SputnikDAO account ID?
    dao_account_id: near_cli_rs::types::account_id::AccountId,
    /// Proposal id to review:
    proposal_id: u64,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network::Network,
}

#[derive(Clone)]
pub struct ReviewContext(near_cli_rs::network::NetworkContext);

impl ReviewContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<Review as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let dao: near_primitives::types::AccountId = scope.dao_account_id.clone().into();
        let proposal_id = scope.proposal_id;
        let on_after_getting_network_callback: near_cli_rs::network::OnAfterGettingNetworkCallback =
            std::sync::Arc::new(move |network_config| review(network_config, &dao, proposal_id));
        Ok(Self(near_cli_rs::network::NetworkContext {
            config: previous_context.config,
            interacting_with_account_ids: vec![scope.dao_account_id.clone().into()],
            on_after_getting_network_callback,
        }))
    }
}

impl From<ReviewContext> for near_cli_rs::network::NetworkContext {
    fn from(item: ReviewContext) -> Self {
        item.0
    }
}

impl Review {
    pub fn input_dao_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        input_dao_account_id(context)
    }
}

fn review(
    network_config: &near_cli_rs::config::NetworkConfig,
    dao: &near_primitives::types::AccountId,
    proposal_id: u64,
) -> near_cli_rs::CliResult {
    let omni_config = crate::config::load_or_init()?;
    let api_network = crate::mpc::to_near_api_network(network_config)?;

    crate::output::info(format!("Fetching proposal #{proposal_id} from {dao} ..."));
    let proposal = crate::dao::fetch_proposal(&api_network, dao, proposal_id)?;
    let status = proposal["status"].as_str().unwrap_or("?");
    let proposer = proposal["proposer"].as_str().unwrap_or("?");
    let description = proposal["description"].as_str().unwrap_or_default();

    let envelope = crate::envelope::decode_description(description).wrap_err(
        "The proposal description carries no omni envelope - this is not an \
         omni-cli proposal (review it in your DAO UI instead).",
    )?;

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

    // What the DAO's derived account actually is for this path
    let keys =
        crate::mpc::fetch_derived_keys(network_config, &omni_config.mpc, dao, &envelope.path)?;
    let derived_address =
        crate::chains::derived_address_for_chain(&chain, &keys.secp256k1, &keys.ed25519)?;

    eprintln!(
        "\nProposal #{proposal_id} on {dao} [{status}] (proposed by {proposer})\n\
         ------------------------------------------------------------\n\
         intent:          {intent}\n\
         chain:           {chain_key} ({family}, NEAR {near_network})\n\
         acting account:  {derived_address} ({dao} / \"{path}\")\n\
         unsigned tx:\n{unsigned_tx}\n\
         ------------------------------------------------------------",
        intent = if envelope.intent.is_empty() {
            "(no intent)"
        } else {
            &envelope.intent
        },
        chain_key = envelope.chain,
        family = envelope.family,
        near_network = network_config.network_name,
        path = envelope.path,
        unsigned_tx = serde_json::to_string_pretty(&envelope.unsigned_tx)?,
    );

    let expected_mpc = crate::mpc::mpc_contract_id(&omni_config.mpc, network_config)?;
    let expected_domain = crate::mpc::domain_id(
        &omni_config.mpc,
        crate::chains::family_scheme(&envelope.family)?,
    );
    match verify_envelope_against_kind(&envelope, &proposal["kind"], &expected_mpc, expected_domain)
    {
        Ok(checks) => {
            for check in checks {
                eprintln!("{} {check}", "[OK]".green());
            }
            eprintln!(
                "\n{}",
                "VERIFIED: the MPC would sign exactly the transaction shown above.".green()
            );
            // What to do next depends on where the proposal is in its life.
            match status {
                "InProgress" => eprintln!(
                    "Vote with:\n  {}\n  {}",
                    format!(
                        "omni proposal vote {dao} {proposal_id} approve <your-account> \
                         network-config {network} sign-with-keychain send",
                        network = network_config.network_name
                    )
                    .yellow(),
                    format!(
                        "omni proposal vote {dao} {proposal_id} reject <your-account> \
                         network-config {network} sign-with-keychain send",
                        network = network_config.network_name
                    )
                    .yellow()
                ),
                "Approved" => eprintln!(
                    "Already approved - voting is over. If the destination-chain transaction \
                     has not been broadcast yet, finalize it with the deciding vote's NEAR \
                     transaction hash:\n  {}",
                    format!(
                        "omni transaction broadcast <NEAR-TX-HASH> <voter-account-id> \
                         network-config {}",
                        network_config.network_name
                    )
                    .yellow()
                ),
                other => eprintln!(
                    "Proposal status is {other} - nothing left to do; the MPC will not sign it."
                ),
            }
            Ok(())
        }
        Err(err) => Err(err.wrap_err(
            "VERIFICATION FAILED - do NOT approve this proposal: what the DAO would \
             ask the MPC to sign does not match what the envelope shows",
        )),
    }
}

// ----------------------------------------------------------------------- vote

#[derive(Debug, EnumDiscriminants, Clone, clap::ValueEnum)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// How do you vote?
pub enum VoteAction {
    #[strum_discriminants(strum(message = "approve - Vote to approve the proposal"))]
    /// Vote to approve the proposal
    Approve,
    #[strum_discriminants(strum(message = "reject  - Vote to reject the proposal"))]
    /// Vote to reject the proposal
    Reject,
}

impl VoteAction {
    fn as_sputnik_action(&self) -> &'static str {
        match self {
            Self::Approve => "VoteApprove",
            Self::Reject => "VoteReject",
        }
    }
}

impl interactive_clap::ToCli for VoteAction {
    type CliVariant = VoteAction;
}

impl std::str::FromStr for VoteAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "approve" => Ok(Self::Approve),
            "reject" => Ok(Self::Reject),
            _ => Err("VoteAction: expected 'approve' or 'reject'".to_string()),
        }
    }
}

impl std::fmt::Display for VoteAction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Approve => write!(f, "approve"),
            Self::Reject => write!(f, "reject"),
        }
    }
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = VoteContext)]
pub struct Vote {
    #[interactive_clap(skip_default_input_arg)]
    /// What is the SputnikDAO account ID?
    dao_account_id: near_cli_rs::types::account_id::AccountId,
    /// Proposal id to vote on:
    proposal_id: u64,
    #[interactive_clap(value_enum)]
    #[interactive_clap(skip_default_input_arg)]
    /// How do you vote?
    action: VoteAction,
    #[interactive_clap(skip_default_input_arg)]
    /// What NEAR account votes (a DAO member)?
    voter_account_id: near_cli_rs::types::account_id::AccountId,
    #[interactive_clap(named_arg)]
    /// Select network
    network_config: near_cli_rs::network_for_transaction::NetworkForTransactionArgs,
}

#[derive(Clone)]
pub struct VoteContext {
    global_context: near_cli_rs::GlobalContext,
    dao_account_id: near_primitives::types::AccountId,
    proposal_id: u64,
    action: VoteAction,
    voter_account_id: near_primitives::types::AccountId,
}

impl VoteContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<Vote as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            dao_account_id: scope.dao_account_id.clone().into(),
            proposal_id: scope.proposal_id,
            action: scope.action.clone(),
            voter_account_id: scope.voter_account_id.clone().into(),
        })
    }
}

impl Vote {
    pub fn input_dao_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        input_dao_account_id(context)
    }

    fn input_action(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<VoteAction>> {
        use strum::IntoEnumIterator;
        let selected = inquire::Select::new(
            "How do you vote?",
            VoteActionDiscriminants::iter()
                .map(|action| action.get_message().unwrap_or_default().to_string())
                .collect(),
        )
        .prompt()?;
        if selected.starts_with("approve") {
            Ok(Some(VoteAction::Approve))
        } else {
            Ok(Some(VoteAction::Reject))
        }
    }

    pub fn input_voter_account_id(
        context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<near_cli_rs::types::account_id::AccountId>> {
        near_cli_rs::common::input_signer_account_id_from_used_account_list(
            &context.config.credentials_home_dir,
            "What NEAR account votes (a DAO member)?",
        )
    }
}

impl From<VoteContext> for near_cli_rs::commands::ActionContext {
    fn from(item: VoteContext) -> Self {
        let get_prepopulated_transaction_after_getting_network_callback: near_cli_rs::commands::GetPrepopulatedTransactionAfterGettingNetworkCallback = {
            let dao = item.dao_account_id.clone();
            let voter = item.voter_account_id.clone();
            let proposal_id = item.proposal_id;
            let action = item.action.clone();
            std::sync::Arc::new(move |network_config| {
                // Newer SputnikDAO versions require the proposal kind to be
                // passed back with the vote, so fetch the proposal first.
                let api_network = crate::mpc::to_near_api_network(network_config)?;
                let proposal = crate::dao::fetch_proposal(&api_network, &dao, proposal_id)?;
                Ok(near_cli_rs::commands::PrepopulatedTransaction {
                    signer_id: voter.clone(),
                    receiver_id: dao.clone(),
                    actions: vec![crate::dao::act_proposal_action(
                        proposal_id,
                        action.as_sputnik_action(),
                        &proposal["kind"],
                    )],
                })
            })
        };

        let on_after_sending_transaction_callback: near_cli_rs::transaction_signature_options::OnAfterSendingTransactionCallback = {
            let dao = item.dao_account_id.clone();
            let proposal_id = item.proposal_id;
            std::sync::Arc::new(move |outcome_view, network_config| {
                let signatures = crate::mpc::extract_signature_responses(outcome_view);
                if signatures.is_empty() {
                    eprintln!(
                        "\nVote recorded on {dao} proposal #{proposal_id}. If a later vote \
                         crosses the threshold, finalize with that vote's transaction hash:\n  {}",
                        format!(
                            "omni transaction broadcast <NEAR-TX-HASH> <voter-account-id> \
                             network-config {}",
                            network_config.network_name
                        )
                        .yellow()
                    );
                } else {
                    eprintln!(
                        "\nThis vote crossed the threshold - the MPC signature(s) are in this \
                         transaction. Broadcast the foreign transaction with:\n  {}",
                        format!(
                            "omni transaction broadcast {} {} network-config {}",
                            outcome_view.transaction.hash,
                            outcome_view.transaction.signer_id,
                            network_config.network_name
                        )
                        .yellow()
                    );
                }
                Ok(())
            })
        };

        Self {
            global_context: item.global_context,
            interacting_with_account_ids: vec![item.voter_account_id, item.dao_account_id],
            get_prepopulated_transaction_after_getting_network_callback,
            on_before_signing_callback: std::sync::Arc::new(|_tx, _network| Ok(())),
            on_before_sending_transaction_callback: std::sync::Arc::new(|_tx, _network| {
                Ok(String::new())
            }),
            on_after_sending_transaction_callback,
            on_sending_delegate_action_callback: None,
            sign_as_delegate_action: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeMeta;

    /// End-to-end review verification on a fabricated EVM proposal: the happy
    /// path passes, and every tampering vector (payload, path, domain,
    /// receiver, foreign method) is rejected.
    #[test]
    fn review_verification_catches_tampering() {
        let mpc: near_primitives::types::AccountId = "v1.signer-prod.testnet".parse().unwrap();
        let tx = crate::chains::evm::build_unsigned(
            11_155_111,
            &crate::chains::evm::EvmActionSpec {
                to: [0xde; 20],
                value_wei: 1,
                data: vec![],
                summary: "test".to_string(),
            },
            &crate::chains::evm::EvmTxParams {
                nonce: 0,
                gas_limit: 21_000,
                max_fee_per_gas: 1,
                max_priority_fee_per_gas: 1,
            },
        );
        let payload = crate::chains::evm::sighash(&tx);
        let envelope = Envelope {
            omni: crate::envelope::VERSION,
            intent: "test".to_string(),
            family: "evm".to_string(),
            chain: "eth".to_string(),
            path: "omni-1".to_string(),
            unsigned_tx: serde_json::to_value(crate::chains::evm::EvmTxJson::from(&tx)).unwrap(),
            meta: EnvelopeMeta::default(),
        };

        let sign_args = serde_json::json!({
            "request": {
                "path": "omni-1",
                "payload_v2": { "Ecdsa": hex::encode(payload) },
                "domain_id": 0,
            }
        });
        let kind = |args: &serde_json::Value, receiver: &str, method: &str| {
            serde_json::json!({
                "FunctionCall": {
                    "receiver_id": receiver,
                    "actions": [{
                        "method_name": method,
                        "args": base64::engine::general_purpose::STANDARD
                            .encode(args.to_string()),
                        "deposit": "1",
                        "gas": "30000000000000",
                    }],
                }
            })
        };

        // Happy path
        let checks = verify_envelope_against_kind(
            &envelope,
            &kind(&sign_args, mpc.as_str(), "sign"),
            &mpc,
            0,
        )
        .unwrap();
        assert_eq!(checks.len(), 5);

        // Tampered payload
        let mut bad = sign_args.clone();
        bad["request"]["payload_v2"]["Ecdsa"] = serde_json::Value::String("ab".repeat(32));
        let err =
            verify_envelope_against_kind(&envelope, &kind(&bad, mpc.as_str(), "sign"), &mpc, 0)
                .unwrap_err();
        assert!(err.to_string().contains("does NOT match"));

        // Tampered path
        let mut bad = sign_args.clone();
        bad["request"]["path"] = serde_json::Value::String("evil".to_string());
        assert!(
            verify_envelope_against_kind(&envelope, &kind(&bad, mpc.as_str(), "sign"), &mpc, 0)
                .is_err()
        );

        // Wrong domain
        let mut bad = sign_args.clone();
        bad["request"]["domain_id"] = serde_json::json!(1);
        assert!(
            verify_envelope_against_kind(&envelope, &kind(&bad, mpc.as_str(), "sign"), &mpc, 0)
                .is_err()
        );

        // Wrong receiver
        assert!(
            verify_envelope_against_kind(
                &envelope,
                &kind(&sign_args, "evil.near", "sign"),
                &mpc,
                0
            )
            .is_err()
        );

        // Foreign method
        assert!(
            verify_envelope_against_kind(
                &envelope,
                &kind(&sign_args, mpc.as_str(), "transfer_ownership"),
                &mpc,
                0
            )
            .is_err()
        );
    }
}
