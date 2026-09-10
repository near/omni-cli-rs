//! Blocking JSON-RPC client for Sui fullnodes, typed: requests and
//! responses are serde structs.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::chains::http::{FlexU64, JsonRpcClient, NO_PARAMS, u64_from_number_or_string};
use crate::chains::sui::abi::NormalizedModule;

/// Sui has no dedicated not-found response for packages/modules: the RPC
/// error text says so. Everything else stays an error.
fn not_found_as_none<T>(
    result: color_eyre::eyre::Result<T>,
) -> color_eyre::eyre::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            let text = err.to_string().to_ascii_lowercase();
            if [
                "not found",
                "does not exist",
                "cannot find",
                "no module found",
            ]
            .iter()
            .any(|needle| text.contains(needle))
            {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

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

/// An object's current reference and ownership (`sui_getObject`).
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub version: u64,
    pub digest_base58: String,
    /// `Some(initial_shared_version)` for shared objects.
    pub shared_initial_version: Option<u64>,
}

#[derive(Deserialize)]
struct GetObjectResponse {
    data: Option<ObjectData>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ObjectData {
    #[serde(deserialize_with = "u64_from_number_or_string")]
    version: u64,
    digest: String,
    #[serde(default)]
    owner: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ObjectDataOptions {
    show_owner: bool,
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

    /// Reference + ownership of an object, for passing it to a Move call.
    pub fn get_object(&self, object_id_hex: &str) -> color_eyre::eyre::Result<ObjectInfo> {
        let response: GetObjectResponse = self.rpc.call(
            "sui_getObject",
            (object_id_hex, ObjectDataOptions { show_owner: true }),
        )?;
        let data = response.data.ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "Object {object_id_hex} not found: {}",
                response.error.unwrap_or_default()
            )
        })?;
        let shared_initial_version = data
            .owner
            .as_ref()
            .and_then(|owner| owner.get("Shared"))
            .and_then(|shared| shared.get("initial_shared_version"))
            .and_then(|version| {
                version
                    .as_u64()
                    .or_else(|| version.as_str().and_then(|text| text.parse().ok()))
            });
        Ok(ObjectInfo {
            version: data.version,
            digest_base58: data.digest,
            shared_initial_version,
        })
    }

    /// The interface of one module; `None` when the package or module does
    /// not exist (the node reports that as an RPC error).
    pub fn normalized_module(
        &self,
        package_hex: &str,
        module: &str,
    ) -> color_eyre::eyre::Result<Option<NormalizedModule>> {
        not_found_as_none(
            self.rpc
                .call("sui_getNormalizedMoveModule", (package_hex, module)),
        )
    }

    /// The interfaces of every module in a package, keyed by module name.
    pub fn normalized_modules(
        &self,
        package_hex: &str,
    ) -> color_eyre::eyre::Result<BTreeMap<String, NormalizedModule>> {
        Ok(not_found_as_none(
            self.rpc
                .call("sui_getNormalizedMoveModulesByPackage", (package_hex,)),
        )?
        .unwrap_or_default())
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
