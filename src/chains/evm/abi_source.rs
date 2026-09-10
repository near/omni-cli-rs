//! Where a contract's ABI comes from in the guided `contract-call` flow:
//! Sourcify (keyless, verified sources) first, Etherscan v2 when an API key
//! is configured, following EIP-1967 proxies to their implementation either
//! way. Lookups are best-effort - every failure becomes a note and the flow
//! falls back to a typed signature - and never affect the non-interactive
//! command, which carries the signature itself.

use alloy_json_abi::{Function, JsonAbi};
use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;

use crate::chains::http::RestClient;
use crate::commands::transaction::construct::guided::{lookup_networks, note};
use crate::config::ChainDef;

const SOURCIFY_URL: &str = "https://sourcify.dev/server";
const ETHERSCAN_V2_URL: &str = "https://api.etherscan.io";

/// `keccak256("eip1967.proxy.implementation") - 1`: where EIP-1967 proxies
/// keep the implementation address.
const EIP1967_IMPLEMENTATION_SLOT: [u8; 32] = [
    0x36, 0x08, 0x94, 0xa1, 0x3b, 0xa1, 0xa3, 0x21, 0x06, 0x67, 0xc8, 0x28, 0x49, 0x2d, 0xb9, 0x8d,
    0xca, 0x3e, 0x20, 0x76, 0xcc, 0x37, 0x35, 0xa9, 0x20, 0xa3, 0xca, 0x50, 0x5d, 0x38, 0x2b, 0xbc,
];

/// A contract interface found for the guided flow.
#[derive(Debug, Clone)]
pub struct FetchedAbi {
    pub abi: JsonAbi,
    /// Where it came from, e.g. `Sourcify (exact match)`.
    pub source: String,
    /// The NEAR network whose chain variant the contract was found on.
    pub network: String,
    /// The implementation the proxy was followed to, if it is one.
    pub implementation: Option<String>,
}

// ------------------------------------------------------------------ Sourcify

/// `GET /v2/contract/{chainId}/{address}?fields=abi,proxyResolution`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcifyContract {
    #[serde(rename = "match")]
    match_kind: Option<String>,
    abi: Option<JsonAbi>,
    proxy_resolution: Option<SourcifyProxyResolution>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcifyProxyResolution {
    #[serde(default)]
    is_proxy: bool,
    #[serde(default)]
    implementations: Vec<SourcifyImplementation>,
}

#[derive(Debug, Deserialize)]
struct SourcifyImplementation {
    address: String,
}

/// One source's answer for one address.
#[derive(Debug)]
struct VerifiedContract {
    abi: JsonAbi,
    source: String,
    implementation: Option<String>,
}

fn sourcify_lookup(
    client: &RestClient,
    chain_id: u64,
    address: &str,
) -> color_eyre::eyre::Result<Option<VerifiedContract>> {
    let (status, body) = client.send(
        client
            .get(&format!("/v2/contract/{chain_id}/{address}"))
            .query(&[("fields", "abi,proxyResolution")]),
    )?;
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(eyre!("Sourcify answered {status}"));
    }
    parse_sourcify(&body)
}

fn parse_sourcify(body: &str) -> color_eyre::eyre::Result<Option<VerifiedContract>> {
    let contract: SourcifyContract =
        serde_json::from_str(body).wrap_err("Unexpected Sourcify response")?;
    let Some(abi) = contract.abi else {
        return Ok(None);
    };
    let source = match contract.match_kind.as_deref() {
        Some("exact_match") => "Sourcify (exact match)".to_string(),
        Some(other) => format!("Sourcify ({})", other.replace('_', " ")),
        None => "Sourcify".to_string(),
    };
    let implementation = contract
        .proxy_resolution
        .filter(|resolution| resolution.is_proxy)
        .and_then(|resolution| resolution.implementations.into_iter().next())
        .map(|implementation| implementation.address);
    Ok(Some(VerifiedContract {
        abi,
        source,
        implementation,
    }))
}

// ----------------------------------------------------------------- Etherscan

/// `GET /v2/api?module=contract&action=getsourcecode`.
#[derive(Debug, Deserialize)]
struct EtherscanEnvelope {
    status: String,
    #[serde(default)]
    message: String,
    result: EtherscanResult,
}

/// `result` is an array of records on success and an error string otherwise.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EtherscanResult {
    Records(Vec<EtherscanSourceCode>),
    Message(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EtherscanSourceCode {
    #[serde(rename = "ABI")]
    abi: String,
    #[serde(default)]
    proxy: String,
    #[serde(default)]
    implementation: String,
}

fn etherscan_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("ETHERSCAN_API_KEY")
        && !key.trim().is_empty()
    {
        return Some(key);
    }
    crate::config::load_or_init()
        .ok()
        .and_then(|config| config.evm.etherscan_api_key)
        .filter(|key| !key.trim().is_empty())
}

fn etherscan_lookup(
    client: &RestClient,
    chain_id: u64,
    address: &str,
    api_key: &str,
) -> color_eyre::eyre::Result<Option<VerifiedContract>> {
    let (status, body) = client.send(client.get("/v2/api").query(&[
        ("chainid", chain_id.to_string().as_str()),
        ("module", "contract"),
        ("action", "getsourcecode"),
        ("address", address),
        ("apikey", api_key),
    ]))?;
    if !status.is_success() {
        return Err(eyre!("Etherscan answered {status}"));
    }
    parse_etherscan(&body)
}

fn parse_etherscan(body: &str) -> color_eyre::eyre::Result<Option<VerifiedContract>> {
    let envelope: EtherscanEnvelope =
        serde_json::from_str(body).wrap_err("Unexpected Etherscan response")?;
    let records = match envelope.result {
        EtherscanResult::Records(records) if envelope.status == "1" => records,
        EtherscanResult::Records(_) => return Ok(None),
        EtherscanResult::Message(message) => {
            return Err(eyre!("Etherscan: {} - {message}", envelope.message));
        }
    };
    let Some(record) = records.into_iter().next() else {
        return Ok(None);
    };
    if !record.abi.trim_start().starts_with('[') {
        // "Contract source code not verified"
        return Ok(None);
    }
    let abi: JsonAbi =
        serde_json::from_str(&record.abi).wrap_err("Etherscan returned an invalid ABI")?;
    let implementation =
        (record.proxy == "1" && !record.implementation.is_empty()).then_some(record.implementation);
    Ok(Some(VerifiedContract {
        abi,
        source: "Etherscan".to_string(),
        implementation,
    }))
}

// -------------------------------------------------------------------- driver

/// The verified-source lookups for one address, in order of preference.
struct Sources {
    sourcify: Option<RestClient>,
    etherscan: Option<(RestClient, String)>,
}

impl Sources {
    fn new() -> Self {
        let sourcify = RestClient::new(SOURCIFY_URL, "Sourcify").ok();
        let etherscan = etherscan_api_key().and_then(|key| {
            RestClient::new(ETHERSCAN_V2_URL, "Etherscan")
                .ok()
                .map(|client| (client, key))
        });
        Self {
            sourcify,
            etherscan,
        }
    }

    /// First source that knows `address`; failures are noted, not fatal.
    fn lookup(&self, chain_id: u64, address: &str) -> Option<VerifiedContract> {
        if let Some(client) = &self.sourcify {
            match sourcify_lookup(client, chain_id, address) {
                Ok(Some(contract)) => return Some(contract),
                Ok(None) => {}
                Err(err) => note(&format!("  Sourcify lookup failed: {err}")),
            }
        }
        if let Some((client, key)) = &self.etherscan {
            match etherscan_lookup(client, chain_id, address, key) {
                Ok(Some(contract)) => return Some(contract),
                Ok(None) => {}
                Err(err) => note(&format!("  Etherscan lookup failed: {err}")),
            }
        }
        None
    }
}

/// Looks the contract's ABI up on the chain's network variants (mainnet
/// first), following proxies to their implementation. `Ok(None)` means no
/// source knows it; the caller falls back to a typed signature.
pub fn fetch_abi(
    chain_def: &ChainDef,
    address: &str,
) -> color_eyre::eyre::Result<Option<FetchedAbi>> {
    let parsed: alloy_primitives::Address = address
        .parse()
        .map_err(|err| eyre!("Invalid EVM address '{address}': {err}"))?;
    let address = format!("{parsed:#x}");
    let sources = Sources::new();

    for (network, variant) in lookup_networks(chain_def) {
        let Some(chain_id) = variant.chain_id else {
            continue;
        };
        let rpc = super::rpc::Client::new(&variant.rpc_url).ok();
        // `has_code == None` means the RPC did not answer; still try the
        // sources, they may know the contract anyway.
        let has_code = rpc
            .as_ref()
            .and_then(|rpc| rpc.get_code(parsed.into_array()).ok())
            .map(|code| !code.is_empty());
        if has_code == Some(false) {
            continue;
        }

        let mut source_label = String::new();
        let mut implementation = None;
        let mut abi = match sources.lookup(chain_id, &address) {
            Some(contract) => {
                source_label = contract.source;
                implementation = contract.implementation;
                Some(contract.abi)
            }
            None => None,
        };

        // Unverified proxy: the implementation slot may still lead somewhere.
        if abi.is_none()
            && let Some(rpc) = &rpc
            && let Ok(word) = rpc.get_storage_at(parsed.into_array(), EIP1967_IMPLEMENTATION_SLOT)
            && word[12..].iter().any(|byte| *byte != 0)
        {
            let implementation_address =
                format!("{:#x}", alloy_primitives::Address::from_slice(&word[12..]));
            note(&format!(
                "  {address} is an EIP-1967 proxy pointing at {implementation_address}; \
                 looking that up instead ..."
            ));
            if let Some(contract) = sources.lookup(chain_id, &implementation_address) {
                source_label = format!("{} via the EIP-1967 proxy", contract.source);
                implementation = Some(implementation_address);
                abi = Some(contract.abi);
            }
        }

        let Some(mut abi) = abi else {
            continue;
        };

        // Verified proxy: merge the implementation's functions in front of
        // the proxy's own (admin) ones.
        if let Some(implementation_address) = &implementation
            && !source_label.contains("via the EIP-1967 proxy")
        {
            match sources.lookup(chain_id, implementation_address) {
                Some(contract) => {
                    abi = merge_abis(contract.abi, abi);
                    source_label =
                        format!("{source_label}, proxy resolved to {implementation_address}");
                }
                None => note(&format!(
                    "  The proxy's implementation {implementation_address} is not verified; \
                     only the proxy's own functions are listed."
                )),
            }
        }

        return Ok(Some(FetchedAbi {
            abi,
            source: source_label,
            network,
            implementation,
        }));
    }
    Ok(None)
}

/// Implementation first, then whatever the proxy adds; duplicate selectors
/// keep the first (implementation) definition.
fn merge_abis(implementation: JsonAbi, proxy: JsonAbi) -> JsonAbi {
    let mut merged = implementation;
    let mut seen: std::collections::HashSet<[u8; 4]> = merged
        .functions()
        .map(|function| function.selector().0)
        .collect();
    for function in proxy.functions() {
        if seen.insert(function.selector().0) {
            merged
                .functions
                .entry(function.name.clone())
                .or_default()
                .push(function.clone());
        }
    }
    merged
}

/// The functions a transaction can call (state-changing), sorted by name.
pub fn callable_functions(abi: &JsonAbi) -> Vec<&Function> {
    let mut functions: Vec<&Function> = abi
        .functions()
        .filter(|function| {
            matches!(
                function.state_mutability,
                alloy_json_abi::StateMutability::NonPayable
                    | alloy_json_abi::StateMutability::Payable
            )
        })
        .collect();
    functions.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.inputs.len().cmp(&b.inputs.len()))
    });
    functions
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERC20_ABI: &str = r#"[
        {"type":"function","name":"transfer","stateMutability":"nonpayable",
         "inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]},
        {"type":"function","name":"balanceOf","stateMutability":"view",
         "inputs":[{"name":"account","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
        {"type":"function","name":"deposit","stateMutability":"payable","inputs":[],"outputs":[]}
    ]"#;

    #[test]
    fn parses_sourcify_exact_match_with_proxy() {
        let body = format!(
            r#"{{"match":"exact_match","chainId":"42161","address":"0xd025b38762B4A4E36F0Cde483b86CB13ea00D989",
                "abi":{ERC20_ABI},
                "proxyResolution":{{"isProxy":true,"proxyType":"EIP1967Proxy",
                   "implementations":[{{"address":"0xB9dE9F72e81d1609E940Fb2217f7286602064881","name":"OmniBridgeWormhole"}}]}}}}"#
        );
        let contract = parse_sourcify(&body).unwrap().unwrap();
        assert_eq!(contract.source, "Sourcify (exact match)");
        assert_eq!(
            contract.implementation.as_deref(),
            Some("0xB9dE9F72e81d1609E940Fb2217f7286602064881")
        );
        assert_eq!(contract.abi.functions().count(), 3);
    }

    #[test]
    fn sourcify_non_proxy_and_missing_abi() {
        let body = format!(
            r#"{{"match":"match","chainId":"1","address":"0x00","abi":{ERC20_ABI},
                "proxyResolution":{{"isProxy":false,"implementations":[]}}}}"#
        );
        let contract = parse_sourcify(&body).unwrap().unwrap();
        assert_eq!(contract.source, "Sourcify (match)");
        assert!(contract.implementation.is_none());
        assert!(
            parse_sourcify(r#"{"match":"match","chainId":"1","address":"0x00"}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parses_etherscan_records_and_not_verified() {
        let abi_escaped = serde_json::to_string(ERC20_ABI).unwrap();
        let body = format!(
            r#"{{"status":"1","message":"OK","result":[{{"SourceCode":"...","ABI":{abi_escaped},
                "ContractName":"Token","Proxy":"1","Implementation":"0xabc0000000000000000000000000000000000001"}}]}}"#
        );
        let contract = parse_etherscan(&body).unwrap().unwrap();
        assert_eq!(contract.source, "Etherscan");
        assert_eq!(
            contract.implementation.as_deref(),
            Some("0xabc0000000000000000000000000000000000001")
        );

        let unverified = r#"{"status":"1","message":"OK","result":[{"ABI":"Contract source code not verified","Proxy":"0","Implementation":""}]}"#;
        assert!(parse_etherscan(unverified).unwrap().is_none());

        let error = r#"{"status":"0","message":"NOTOK","result":"Missing/Invalid API Key"}"#;
        assert!(parse_etherscan(error).is_err());
    }

    #[test]
    fn merges_implementation_first_and_dedupes_selectors() {
        let implementation: JsonAbi = serde_json::from_str(ERC20_ABI).unwrap();
        let proxy: JsonAbi = serde_json::from_str(
            r#"[
            {"type":"function","name":"upgradeTo","stateMutability":"nonpayable",
             "inputs":[{"name":"newImplementation","type":"address"}],"outputs":[]},
            {"type":"function","name":"transfer","stateMutability":"nonpayable",
             "inputs":[{"name":"x","type":"address"},{"name":"y","type":"uint256"}],"outputs":[]}
        ]"#,
        )
        .unwrap();
        let merged = merge_abis(implementation, proxy);
        assert_eq!(merged.functions().count(), 4);
        // The implementation's `transfer` (named to/amount) wins.
        let transfer = &merged.function("transfer").unwrap()[0];
        assert_eq!(transfer.inputs[0].name, "to");

        let callable: Vec<String> = callable_functions(&merged)
            .iter()
            .map(|function| function.name.clone())
            .collect();
        assert_eq!(callable, ["deposit", "transfer", "upgradeTo"]);
    }
}
