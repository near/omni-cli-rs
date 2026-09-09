//! Minimal blocking client for the Aptos fullnode REST API.

use color_eyre::eyre::{WrapErr, eyre};

fn get(base_url: &str, path: &str) -> color_eyre::eyre::Result<serde_json::Value> {
    let url = format!("{}/v1{path}", base_url.trim_end_matches('/'));
    let response = reqwest::blocking::Client::new()
        .get(&url)
        .send()
        .wrap_err_with(|| format!("Failed to reach Aptos REST API at {url}"))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .wrap_err_with(|| format!("Invalid JSON from Aptos REST API at {url}"))?;
    if !status.is_success() {
        return Err(eyre!(
            "Aptos REST API error ({status}) from {path}: {}",
            body["message"].as_str().unwrap_or("unknown error")
        ));
    }
    Ok(body)
}

fn parse_u64(value: &serde_json::Value) -> color_eyre::eyre::Result<u64> {
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| eyre!("Expected a u64, got: {value}")),
        serde_json::Value::String(s) => s
            .parse()
            .wrap_err_with(|| format!("Expected a u64 string, got: {s}")),
        other => Err(eyre!("Expected a u64, got: {other}")),
    }
}

/// Chain id and current ledger timestamp (unix seconds).
pub fn ledger_info(base_url: &str) -> color_eyre::eyre::Result<(u8, u64)> {
    let info = get(base_url, "")?;
    let chain_id = info["chain_id"]
        .as_u64()
        .ok_or_else(|| eyre!("Aptos ledger info has no chain_id"))? as u8;
    let timestamp_secs = parse_u64(&info["ledger_timestamp"])? / 1_000_000;
    Ok((chain_id, timestamp_secs))
}

pub fn sequence_number(base_url: &str, address_hex: &str) -> color_eyre::eyre::Result<u64> {
    let account = get(base_url, &format!("/accounts/{address_hex}")).map_err(|err| {
        eyre!(
            "{err}\n(An Aptos account is created by receiving coins - if the derived \
             account does not exist yet, fund the derived address first.)"
        )
    })?;
    parse_u64(&account["sequence_number"])
}

/// (regular, prioritized) gas unit price estimates, in octas.
pub fn estimate_gas_price(base_url: &str) -> color_eyre::eyre::Result<(u64, u64)> {
    let estimate = get(base_url, "/estimate_gas_price")?;
    let regular = parse_u64(&estimate["gas_estimate"])?;
    let prioritized = estimate
        .get("prioritized_gas_estimate")
        .map(parse_u64)
        .transpose()?
        .unwrap_or(regular * 2);
    Ok((regular, prioritized))
}

/// Best-effort APT balance in octas (0 if the endpoint is unavailable).
pub fn apt_balance(base_url: &str, address_hex: &str) -> u64 {
    get(
        base_url,
        &format!("/accounts/{address_hex}/balance/0x1::aptos_coin::AptosCoin"),
    )
    .ok()
    .and_then(|value| parse_u64(&value).ok())
    .unwrap_or(0)
}

/// Broadcasts BCS `SignedTransaction` bytes; returns the transaction hash.
pub fn submit_transaction(base_url: &str, signed_tx: &[u8]) -> color_eyre::eyre::Result<String> {
    let url = format!("{}/v1/transactions", base_url.trim_end_matches('/'));
    let response = reqwest::blocking::Client::new()
        .post(&url)
        .header(
            "Content-Type",
            "application/x.aptos.signed_transaction+bcs",
        )
        .body(signed_tx.to_vec())
        .send()
        .wrap_err_with(|| format!("Failed to reach Aptos REST API at {url}"))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .wrap_err("Invalid JSON from the Aptos submit endpoint")?;
    if !status.is_success() {
        return Err(eyre!(
            "Aptos rejected the transaction ({status}): {}",
            body["message"].as_str().unwrap_or("unknown error")
        ));
    }
    body["hash"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| eyre!("Aptos submit response has no hash: {body}"))
}
