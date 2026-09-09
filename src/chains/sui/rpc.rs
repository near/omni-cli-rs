//! Blocking JSON-RPC client for Sui fullnodes, typed: requests and
//! responses are serde structs.

use serde::Deserialize;

use crate::chains::http::{FlexU64, JsonRpcClient, NO_PARAMS, u64_from_number_or_string};

pub struct Client {
    rpc: JsonRpcClient,
}

/// A SUI gas coin owned by an address.
#[derive(Debug, Clone, Deserialize)]
pub struct SuiCoin {
    #[serde(rename = "coinObjectId")]
    pub object_id: String,
    #[serde(deserialize_with = "u64_from_number_or_string")]
    pub version: u64,
    #[serde(rename = "digest")]
    pub digest_base58: String,
    #[serde(deserialize_with = "u64_from_number_or_string")]
    pub balance: u64,
}

#[derive(Deserialize)]
struct CoinPage {
    data: Vec<SuiCoin>,
}

#[derive(Deserialize)]
struct ExecuteResponse {
    digest: String,
}

impl Client {
    pub fn new(rpc_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rpc: JsonRpcClient::new(rpc_url, "Sui RPC")?,
        })
    }

    pub fn reference_gas_price(&self) -> color_eyre::eyre::Result<u64> {
        let FlexU64(price) = self.rpc.call("suix_getReferenceGasPrice", NO_PARAMS)?;
        Ok(price)
    }

    /// SUI coins owned by `owner` (first page, up to 50 - plenty for gas
    /// selection).
    pub fn sui_coins(&self, owner: &str) -> color_eyre::eyre::Result<Vec<SuiCoin>> {
        let page: CoinPage = self
            .rpc
            .call("suix_getCoins", (owner, "0x2::sui::SUI", (), 50))?;
        Ok(page.data)
    }

    /// Broadcasts a signed transaction; returns the transaction digest.
    pub fn execute_transaction(
        &self,
        tx_bytes_base64: &str,
        signature_base64: &str,
    ) -> color_eyre::eyre::Result<String> {
        let response: ExecuteResponse = self.rpc.call(
            "sui_executeTransactionBlock",
            (tx_bytes_base64, [signature_base64]),
        )?;
        Ok(response.digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sui encodes u64s (version, balance) as decimal strings.
    #[test]
    fn parses_a_coin_page() {
        let page: CoinPage = serde_json::from_str(
            r#"{"data":[{"coinType":"0x2::sui::SUI",
                "coinObjectId":"0xd0c1...abc","version":"1735",
                "digest":"8qCvNyoWJUZ9F6iK1kA9J4mDW2r5S3PqXhZbTnE7Vgud",
                "balance":"5000000000","previousTransaction":"..."}],
                "nextCursor":"0xd0c1...abc","hasNextPage":false}"#,
        )
        .unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].object_id, "0xd0c1...abc");
        assert_eq!(page.data[0].version, 1735);
        assert_eq!(page.data[0].balance, 5_000_000_000);
    }
}
