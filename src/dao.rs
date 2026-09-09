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

/// Builds the `add_proposal` FunctionCall action wrapping the MPC `sign`
/// request(s) as a SputnikDAO FunctionCall proposal. One sign action per
/// payload (UTXO chains need one signature per input).
pub fn add_proposal_action(
    description: &str,
    mpc_contract: &near_primitives::types::AccountId,
    sign_args_list: &[serde_json::Value],
    mpc_config: &MpcConfig,
    proposal_bond: u128,
) -> near_primitives::transaction::Action {
    let sign_actions: Vec<serde_json::Value> = sign_args_list
        .iter()
        .map(|sign_args| {
            serde_json::json!({
                "method_name": "sign",
                "args": base64::engine::general_purpose::STANDARD
                    .encode(sign_args.to_string()),
                "deposit": mpc_config.sign_deposit_yoctonear.to_string(),
                "gas": near_gas::NearGas::from_tgas(mpc_config.sign_gas_tgas)
                    .as_gas()
                    .to_string(),
            })
        })
        .collect();
    let args = serde_json::json!({
        "proposal": {
            "description": description,
            "kind": {
                "FunctionCall": {
                    "receiver_id": mpc_contract.as_str(),
                    "actions": sign_actions,
                }
            },
        }
    });
    near_primitives::transaction::Action::FunctionCall(Box::new(
        near_primitives::transaction::FunctionCallAction {
            method_name: "add_proposal".to_string(),
            args: args.to_string().into_bytes(),
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
