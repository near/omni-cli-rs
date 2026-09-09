//! SputnikDAO v2 client: proposal bond lookup and add_proposal construction.

use base64::Engine;
use color_eyre::eyre::{WrapErr, eyre};

use crate::config::MpcConfig;

pub fn fetch_proposal_bond(
    network: &near_api::NetworkConfig,
    dao_account_id: &near_primitives::types::AccountId,
) -> color_eyre::eyre::Result<u128> {
    let contract = near_api::Contract(
        dao_account_id
            .as_str()
            .parse()
            .wrap_err("Invalid DAO account id")?,
    );
    let policy = crate::mpc::block_on(
        contract
            .call_function("get_policy", serde_json::json!({}))
            .read_only::<serde_json::Value>()
            .fetch_from(network),
    )?
    .wrap_err_with(|| {
        format!("Failed to fetch the DAO policy from {dao_account_id} (is it a SputnikDAO v2?)")
    })?;

    policy
        .data
        .get("proposal_bond")
        .and_then(|bond| bond.as_str())
        .ok_or_else(|| eyre!("DAO policy from {dao_account_id} has no proposal_bond"))?
        .parse()
        .wrap_err("DAO proposal_bond is not a valid u128")
}

/// SputnikDAO `add_proposal` arguments (v2 wire format).
#[derive(serde::Serialize)]
struct AddProposalArgs {
    proposal: ProposalInput,
}

#[derive(serde::Serialize)]
struct ProposalInput {
    description: String,
    kind: ProposalKind,
}

/// Externally tagged to match the contract: `{"FunctionCall": {...}}`.
#[derive(serde::Serialize)]
enum ProposalKind {
    FunctionCall {
        receiver_id: String,
        actions: Vec<ActionCall>,
    },
}

/// One action of a FunctionCall proposal. `args` is base64 of the call's
/// JSON arguments; SputnikDAO takes deposit and gas as decimal strings.
#[derive(serde::Serialize)]
struct ActionCall {
    method_name: String,
    args: String,
    deposit: String,
    gas: String,
}

/// Builds the `add_proposal` FunctionCall action wrapping the MPC `sign`
/// request(s) as a SputnikDAO FunctionCall proposal. One sign action per
/// payload (UTXO chains need one signature per input).
pub fn add_proposal_action(
    description: &str,
    mpc_contract: &near_primitives::types::AccountId,
    sign_args_list: &[crate::mpc::SignArgs],
    mpc_config: &MpcConfig,
    proposal_bond: u128,
) -> near_primitives::transaction::Action {
    let gas_tgas = crate::mpc::sign_gas_per_action_tgas(mpc_config, sign_args_list.len());
    let sign_actions: Vec<ActionCall> = sign_args_list
        .iter()
        .map(|sign_args| ActionCall {
            method_name: "sign".to_string(),
            args: base64::engine::general_purpose::STANDARD.encode(
                serde_json::to_vec(sign_args).expect("SignArgs serialization is infallible"),
            ),
            deposit: mpc_config.sign_deposit_yoctonear.to_string(),
            gas: near_gas::NearGas::from_tgas(gas_tgas).as_gas().to_string(),
        })
        .collect();
    let args = AddProposalArgs {
        proposal: ProposalInput {
            description: description.to_string(),
            kind: ProposalKind::FunctionCall {
                receiver_id: mpc_contract.to_string(),
                actions: sign_actions,
            },
        },
    };
    near_primitives::transaction::Action::FunctionCall(Box::new(
        near_primitives::transaction::FunctionCallAction {
            method_name: "add_proposal".to_string(),
            args: serde_json::to_vec(&args).expect("AddProposalArgs serialization is infallible"),
            gas: near_primitives::gas::Gas::from_gas(near_gas::NearGas::from_tgas(100).as_gas()),
            deposit: near_token::NearToken::from_yoctonear(proposal_bond),
        },
    ))
}

/// `add_proposal` returns the new proposal id as a JSON number.
pub fn proposal_id_from_outcome(
    outcome: &near_primitives::views::FinalExecutionOutcomeView,
) -> Option<u64> {
    match &outcome.status {
        near_primitives::views::FinalExecutionStatus::SuccessValue(value) => {
            serde_json::from_slice::<u64>(value).ok()
        }
        _ => None,
    }
}

/// The next proposal id (== total number of proposals so far).
pub fn fetch_last_proposal_id(
    network: &near_api::NetworkConfig,
    dao_account_id: &near_primitives::types::AccountId,
) -> color_eyre::eyre::Result<u64> {
    let contract = near_api::Contract(
        dao_account_id
            .as_str()
            .parse()
            .wrap_err("Invalid DAO account id")?,
    );
    Ok(crate::mpc::block_on(
        contract
            .call_function("get_last_proposal_id", serde_json::json!({}))
            .read_only::<u64>()
            .fetch_from(network),
    )?
    .wrap_err_with(|| format!("Failed to fetch the proposal count from {dao_account_id}"))?
    .data)
}

pub fn fetch_proposals(
    network: &near_api::NetworkConfig,
    dao_account_id: &near_primitives::types::AccountId,
    from_index: u64,
    limit: u64,
) -> color_eyre::eyre::Result<Vec<serde_json::Value>> {
    #[derive(serde::Serialize)]
    struct GetProposalsArgs {
        from_index: u64,
        limit: u64,
    }

    let contract = near_api::Contract(
        dao_account_id
            .as_str()
            .parse()
            .wrap_err("Invalid DAO account id")?,
    );
    Ok(crate::mpc::block_on(
        contract
            .call_function("get_proposals", GetProposalsArgs { from_index, limit })
            .read_only::<Vec<serde_json::Value>>()
            .fetch_from(network),
    )?
    .wrap_err_with(|| format!("Failed to fetch proposals from {dao_account_id}"))?
    .data)
}

pub fn fetch_proposal(
    network: &near_api::NetworkConfig,
    dao_account_id: &near_primitives::types::AccountId,
    proposal_id: u64,
) -> color_eyre::eyre::Result<serde_json::Value> {
    #[derive(serde::Serialize)]
    struct GetProposalArgs {
        id: u64,
    }

    let contract = near_api::Contract(
        dao_account_id
            .as_str()
            .parse()
            .wrap_err("Invalid DAO account id")?,
    );
    Ok(crate::mpc::block_on(
        contract
            .call_function("get_proposal", GetProposalArgs { id: proposal_id })
            .read_only::<serde_json::Value>()
            .fetch_from(network),
    )?
    .wrap_err_with(|| format!("Failed to fetch proposal #{proposal_id} from {dao_account_id}"))?
    .data)
}

/// The `act_proposal` FunctionCall action for voting. Full gas: the deciding
/// vote executes the proposal (the MPC sign calls) in the same transaction.
/// Newer SputnikDAO versions require the proposal kind to be passed back and
/// assert it matches the stored one (ERR_WRONG_KIND); older versions ignore
/// the extra field.
pub fn act_proposal_action(
    proposal_id: u64,
    vote_action: &str,
    proposal_kind: &serde_json::Value,
) -> near_primitives::transaction::Action {
    #[derive(serde::Serialize)]
    struct ActProposalArgs<'a> {
        id: u64,
        action: &'a str,
        proposal: &'a serde_json::Value,
    }
    near_primitives::transaction::Action::FunctionCall(Box::new(
        near_primitives::transaction::FunctionCallAction {
            method_name: "act_proposal".to_string(),
            args: serde_json::to_vec(&ActProposalArgs {
                id: proposal_id,
                action: vote_action,
                proposal: proposal_kind,
            })
            .expect("ActProposalArgs serialization is infallible"),
            gas: near_primitives::gas::Gas::from_gas(near_gas::NearGas::from_tgas(300).as_gas()),
            deposit: near_token::NearToken::from_yoctonear(0),
        },
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn act_proposal_args_carry_the_proposal_kind() {
        let kind = serde_json::json!({
            "FunctionCall": { "receiver_id": "v1.signer", "actions": [] }
        });
        let near_primitives::transaction::Action::FunctionCall(call) =
            super::act_proposal_action(4, "VoteApprove", &kind)
        else {
            panic!("expected a FunctionCall action");
        };
        let args: serde_json::Value = serde_json::from_slice(&call.args).unwrap();
        assert_eq!(args["id"], 4);
        assert_eq!(args["action"], "VoteApprove");
        assert_eq!(args["proposal"], kind);
    }
}
