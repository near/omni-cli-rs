//! Blocking client for the Aptos fullnode REST API, typed: responses are
//! serde structs (Aptos encodes most u64s as decimal strings).

use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::chains::aptos::abi::{ModuleAbi, MoveModule};
use crate::chains::http::{FlexU64, RestClient, u64_from_number_or_string};

pub struct Client {
    rest: RestClient,
}

/// Chain id and current ledger timestamp.
#[derive(Debug, Clone, Deserialize)]
pub struct LedgerInfo {
    pub chain_id: u8,
    #[serde(
        rename = "ledger_timestamp",
        deserialize_with = "u64_from_number_or_string"
    )]
    timestamp_micros: u64,
}

impl LedgerInfo {
    pub fn timestamp_secs(&self) -> u64 {
        self.timestamp_micros / 1_000_000
    }
}

#[derive(Deserialize)]
struct AccountInfo {
    #[serde(deserialize_with = "u64_from_number_or_string")]
    sequence_number: u64,
}

/// Gas unit price estimates, in octas.
#[derive(Debug, Clone, Deserialize)]
pub struct GasPrices {
    #[serde(rename = "gas_estimate")]
    pub regular: u64,
    #[serde(rename = "prioritized_gas_estimate")]
    prioritized: Option<u64>,
}

impl GasPrices {
    pub fn prioritized(&self) -> u64 {
        self.prioritized.unwrap_or(self.regular * 2)
    }
}

#[derive(Deserialize)]
struct SubmitResponse {
    hash: String,
}

/// One simulated transaction as the node reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct Simulation {
    #[serde(deserialize_with = "u64_from_number_or_string")]
    pub gas_used: u64,
    pub success: bool,
    pub vm_status: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    message: String,
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<ErrorResponse>(body)
        .map_or_else(|_| body.to_string(), |error| error.message)
}

impl Client {
    pub fn new(base_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rest: RestClient::new(base_url, "Aptos REST API")?,
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> color_eyre::eyre::Result<T> {
        self.get_optional(path)?
            .ok_or_else(|| eyre!("Aptos REST API error (404 Not Found) from {path}"))
    }

    /// Like `get`, but a 404 is `Ok(None)` (accounts and modules that do
    /// not exist).
    fn get_optional<T: DeserializeOwned>(&self, path: &str) -> color_eyre::eyre::Result<Option<T>> {
        let (status, body) = self.rest.send(self.rest.get(&format!("/v1{path}")))?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(eyre!(
                "Aptos REST API error ({status}) from {path}: {}",
                error_message(&body)
            ));
        }
        serde_json::from_str(&body)
            .map(Some)
            .wrap_err_with(|| format!("Unexpected Aptos REST API response from {path}: {body}"))
    }

    /// The interface of one published module; `None` if the account or
    /// module does not exist.
    pub fn module_abi(
        &self,
        address_hex: &str,
        module: &str,
    ) -> color_eyre::eyre::Result<Option<ModuleAbi>> {
        Ok(self
            .get_optional::<MoveModule>(&format!("/accounts/{address_hex}/module/{module}"))?
            .and_then(|module| module.abi))
    }

    /// The interfaces of every module an account publishes (first 100).
    pub fn modules(&self, address_hex: &str) -> color_eyre::eyre::Result<Vec<ModuleAbi>> {
        let modules: Vec<MoveModule> = self
            .get_optional(&format!("/accounts/{address_hex}/modules?limit=100"))?
            .unwrap_or_default();
        Ok(modules
            .into_iter()
            .filter_map(|module| module.abi)
            .collect())
    }

    pub fn ledger_info(&self) -> color_eyre::eyre::Result<LedgerInfo> {
        self.get("")
    }

    pub fn sequence_number(&self, address_hex: &str) -> color_eyre::eyre::Result<u64> {
        let account: AccountInfo =
            self.get(&format!("/accounts/{address_hex}"))
                .map_err(|err| {
                    eyre!(
                        "{err}\n(An Aptos account is created by receiving coins - if the derived \
                     account does not exist yet, fund the derived address first.)"
                    )
                })?;
        Ok(account.sequence_number)
    }

    pub fn estimate_gas_price(&self) -> color_eyre::eyre::Result<GasPrices> {
        self.get("/estimate_gas_price")
    }

    /// Best-effort APT balance in octas (0 if the endpoint is unavailable).
    pub fn apt_balance(&self, address_hex: &str) -> u64 {
        self.get::<FlexU64>(&format!(
            "/accounts/{address_hex}/balance/0x1::aptos_coin::AptosCoin"
        ))
        .map_or(0, |FlexU64(balance)| balance)
    }

    /// Simulates BCS `SignedTransaction` bytes (the signature is not
    /// verified, so an all-zero one is fine): the gas it would use and
    /// whether it would abort.
    pub fn simulate(&self, signed_tx: &[u8]) -> color_eyre::eyre::Result<Simulation> {
        let request = self
            .rest
            .post("/v1/transactions/simulate")
            .header("Content-Type", "application/x.aptos.signed_transaction+bcs")
            .body(signed_tx.to_vec());
        let (status, body) = self.rest.send(request)?;
        if !status.is_success() {
            return Err(eyre!(
                "Aptos simulation failed ({status}): {}",
                error_message(&body)
            ));
        }
        let results: Vec<Simulation> = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Unexpected Aptos simulation response: {body}"))?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| eyre!("Aptos simulation returned no result"))
    }

    /// Broadcasts BCS `SignedTransaction` bytes; returns the transaction
    /// hash.
    pub fn submit_transaction(&self, signed_tx: &[u8]) -> color_eyre::eyre::Result<String> {
        let request = self
            .rest
            .post("/v1/transactions")
            .header("Content-Type", "application/x.aptos.signed_transaction+bcs")
            .body(signed_tx.to_vec());
        let (status, body) = self.rest.send(request)?;
        if !status.is_success() {
            return Err(eyre!(
                "Aptos rejected the transaction ({status}): {}",
                error_message(&body)
            ));
        }
        let response: SubmitResponse = serde_json::from_str(&body)
            .wrap_err_with(|| format!("Aptos submit response has no hash: {body}"))?;
        Ok(response.hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aptos encodes most u64s as decimal strings.
    #[test]
    fn parses_ledger_info_and_gas_estimates() {
        let info: LedgerInfo = serde_json::from_str(
            r#"{"chain_id":2,"epoch":"1234","ledger_version":"999",
                "ledger_timestamp":"1757400000123456","node_role":"full_node"}"#,
        )
        .unwrap();
        assert_eq!(info.chain_id, 2);
        assert_eq!(info.timestamp_secs(), 1_757_400_000);

        let with_priority: GasPrices = serde_json::from_str(
            r#"{"deprioritized_gas_estimate":100,"gas_estimate":100,"prioritized_gas_estimate":150}"#,
        )
        .unwrap();
        assert_eq!(with_priority.regular, 100);
        assert_eq!(with_priority.prioritized(), 150);

        let without_priority: GasPrices = serde_json::from_str(r#"{"gas_estimate":100}"#).unwrap();
        assert_eq!(without_priority.prioritized(), 200);
    }
}
