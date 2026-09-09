//! Blocking client for Esplora-compatible HTTP APIs (blockstream.info,
//! mempool.space) - the de-facto public Bitcoin API. Typed: responses are
//! serde structs.

use std::collections::BTreeMap;

use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::chains::http::RestClient;

pub struct Client {
    rest: RestClient,
}

/// An unspent output of an address.
#[derive(Debug, Clone, Deserialize)]
pub struct Utxo {
    /// Display-order (big-endian) txid hex, as explorers show it.
    pub txid: String,
    pub vout: u32,
    #[serde(rename = "value")]
    pub value_sats: u64,
}

#[derive(Deserialize)]
struct UtxoEntry {
    #[serde(flatten)]
    utxo: Utxo,
    status: UtxoStatus,
}

#[derive(Deserialize)]
struct UtxoStatus {
    confirmed: bool,
}

impl Client {
    pub fn new(base_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rest: RestClient::new(base_url, "Esplora API")?,
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> color_eyre::eyre::Result<T> {
        let (status, body) = self.rest.send(self.rest.get(path))?;
        if !status.is_success() {
            return Err(eyre!("Esplora API error ({status}) from {path}: {body}"));
        }
        serde_json::from_str(&body)
            .wrap_err_with(|| format!("Unexpected Esplora API response from {path}: {body}"))
    }

    /// Confirmed UTXOs of an address.
    pub fn utxos(&self, address: &str) -> color_eyre::eyre::Result<Vec<Utxo>> {
        let entries: Vec<UtxoEntry> = self.get(&format!("/address/{address}/utxo"))?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.status.confirmed)
            .map(|entry| entry.utxo)
            .collect())
    }

    /// Fee rate in sat/vB for the given confirmation target (in blocks).
    pub fn fee_rate(&self, target_blocks: u32) -> color_eyre::eyre::Result<f64> {
        let estimates: BTreeMap<String, f64> = self.get("/fee-estimates")?;
        // Take the closest target at or below the requested one; fall back
        // to the fastest estimate available.
        (1..=target_blocks)
            .rev()
            .find_map(|target| estimates.get(&target.to_string()).copied())
            .or_else(|| estimates.get("1").copied())
            .ok_or_else(|| eyre!("The fee-estimates endpoint returned no usable rate"))
    }

    /// Broadcasts raw transaction hex; returns the txid.
    pub fn broadcast_transaction(&self, tx_hex: &str) -> color_eyre::eyre::Result<String> {
        let request = self.rest.post("/tx").body(tx_hex.to_string());
        let (status, body) = self.rest.send(request)?;
        if !status.is_success() {
            return Err(eyre!(
                "The Bitcoin network rejected the transaction: {body}"
            ));
        }
        Ok(body.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utxo_entries_with_status() {
        let entries: Vec<UtxoEntry> = serde_json::from_str(
            r#"[{"txid":"ab","vout":1,"value":5000,
                 "status":{"confirmed":true,"block_height":800000}},
                {"txid":"cd","vout":0,"value":700,"status":{"confirmed":false}}]"#,
        )
        .unwrap();
        let confirmed: Vec<Utxo> = entries
            .into_iter()
            .filter(|entry| entry.status.confirmed)
            .map(|entry| entry.utxo)
            .collect();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].txid, "ab");
        assert_eq!(confirmed[0].vout, 1);
        assert_eq!(confirmed[0].value_sats, 5000);
    }
}
