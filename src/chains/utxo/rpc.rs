//! Minimal blocking client for Esplora-compatible HTTP APIs
//! (blockstream.info, mempool.space) - the de-facto public Bitcoin API.

use color_eyre::eyre::{WrapErr, eyre};

fn get(base_url: &str, path: &str) -> color_eyre::eyre::Result<serde_json::Value> {
    let url = format!("{}{path}", base_url.trim_end_matches('/'));
    let response = reqwest::blocking::Client::new()
        .get(&url)
        .send()
        .wrap_err_with(|| format!("Failed to reach the Esplora API at {url}"))?;
    let status = response.status();
    let body = response.text().wrap_err("Failed to read the Esplora response")?;
    if !status.is_success() {
        return Err(eyre!("Esplora API error ({status}) from {path}: {body}"));
    }
    serde_json::from_str(&body).wrap_err_with(|| format!("Invalid JSON from {url}: {body}"))
}

/// An unspent output of an address.
#[derive(Debug, Clone)]
pub struct Utxo {
    /// Display-order (big-endian) txid hex, as explorers show it.
    pub txid: String,
    pub vout: u32,
    pub value_sats: u64,
}

/// Confirmed UTXOs of an address.
pub fn utxos(base_url: &str, address: &str) -> color_eyre::eyre::Result<Vec<Utxo>> {
    let list = get(base_url, &format!("/address/{address}/utxo"))?;
    let entries = list
        .as_array()
        .ok_or_else(|| eyre!("The UTXO endpoint returned a non-array"))?;
    entries
        .iter()
        .filter(|utxo| utxo["status"]["confirmed"].as_bool().unwrap_or(false))
        .map(|utxo| {
            Ok(Utxo {
                txid: utxo["txid"]
                    .as_str()
                    .ok_or_else(|| eyre!("UTXO entry has no txid"))?
                    .to_string(),
                vout: utxo["vout"]
                    .as_u64()
                    .ok_or_else(|| eyre!("UTXO entry has no vout"))? as u32,
                value_sats: utxo["value"]
                    .as_u64()
                    .ok_or_else(|| eyre!("UTXO entry has no value"))?,
            })
        })
        .collect()
}

/// Fee rate in sat/vB for the given confirmation target (in blocks).
pub fn fee_rate(base_url: &str, target_blocks: u32) -> color_eyre::eyre::Result<f64> {
    let estimates = get(base_url, "/fee-estimates")?;
    // Take the closest target at or below the requested one; fall back to
    // the fastest estimate available.
    (1..=target_blocks)
        .rev()
        .find_map(|target| estimates[target.to_string()].as_f64())
        .or_else(|| estimates["1"].as_f64())
        .ok_or_else(|| eyre!("The fee-estimates endpoint returned no usable rate"))
}

/// Broadcasts raw transaction hex; returns the txid.
pub fn broadcast_transaction(base_url: &str, tx_hex: &str) -> color_eyre::eyre::Result<String> {
    let url = format!("{}/tx", base_url.trim_end_matches('/'));
    let response = reqwest::blocking::Client::new()
        .post(&url)
        .body(tx_hex.to_string())
        .send()
        .wrap_err_with(|| format!("Failed to reach the Esplora API at {url}"))?;
    let status = response.status();
    let body = response.text().wrap_err("Failed to read the broadcast response")?;
    if !status.is_success() {
        return Err(eyre!("The Bitcoin network rejected the transaction: {body}"));
    }
    Ok(body.trim().to_string())
}
