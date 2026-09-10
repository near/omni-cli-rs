//! The exposed-function interface of an Aptos module, as the fullnode REST
//! API publishes it (`/v1/accounts/{address}/module/{name}`), and the
//! mapping from declared parameter types to guided `type:value` input.

use serde::Deserialize;

use crate::chains::move_call::{ArgKind, GuidedParam, SelectedMoveFunction};

/// `abi` of a module response.
#[derive(Debug, Clone, Deserialize)]
pub struct ModuleAbi {
    pub name: String,
    #[serde(default)]
    pub exposed_functions: Vec<ExposedFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExposedFunction {
    pub name: String,
    pub is_entry: bool,
    /// Ability constraints per type parameter; only the count matters here.
    #[serde(default)]
    pub generic_type_params: Vec<serde_json::Value>,
    /// Move types as text (`&signer`, `u64`, `0x1::string::String`, `T0`).
    #[serde(default)]
    pub params: Vec<String>,
}

/// A module as the REST API returns it (`bytecode` + `abi`).
#[derive(Debug, Clone, Deserialize)]
pub struct MoveModule {
    pub abi: Option<ModuleAbi>,
}

impl ModuleAbi {
    /// The functions a transaction can call.
    pub fn entry_functions(&self) -> impl Iterator<Item = &ExposedFunction> {
        self.exposed_functions
            .iter()
            .filter(|function| function.is_entry)
    }
}

impl ExposedFunction {
    pub fn guided_params(&self) -> Vec<GuidedParam> {
        self.params
            .iter()
            .map(|param| GuidedParam {
                type_text: param.clone(),
                kind: arg_kind_for_aptos(param),
            })
            .collect()
    }

    pub fn selected(&self, module_address: &str, module_name: &str) -> SelectedMoveFunction {
        SelectedMoveFunction {
            path: format!("{module_address}::{module_name}::{}", self.name),
            generic_count: self.generic_type_params.len(),
            params: self.guided_params(),
        }
    }
}

/// How a declared Aptos entry-function parameter is collected. Signers are
/// supplied by the transaction; `Object<T>` handles are passed as their
/// address; `String` and `vector<u8>` map to `string:` and `hex:`.
pub fn arg_kind_for_aptos(param: &str) -> ArgKind {
    let param = param.trim();
    let param = param
        .strip_prefix("&mut ")
        .or_else(|| param.strip_prefix('&'))
        .unwrap_or(param)
        .trim();
    match param {
        "signer" => ArgKind::Skip,
        "bool" => ArgKind::Typed("bool"),
        "u8" => ArgKind::Typed("u8"),
        "u16" => ArgKind::Typed("u16"),
        "u32" => ArgKind::Typed("u32"),
        "u64" => ArgKind::Typed("u64"),
        "u128" => ArgKind::Typed("u128"),
        "u256" => ArgKind::Typed("u256"),
        "address" => ArgKind::Typed("address"),
        "0x1::string::String" => ArgKind::Typed("string"),
        "vector<u8>" => ArgKind::Typed("hex"),
        "vector<address>" => ArgKind::Typed("vector<address>"),
        other if other.starts_with("0x1::object::Object<") => ArgKind::Typed("address"),
        _ => ArgKind::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = r#"{"bytecode":"0x00","abi":{"address":"0xe4ec","name":"omni_bridge",
        "friends":[],"exposed_functions":[
        {"name":"log_metadata","visibility":"public","is_entry":true,"is_view":false,
         "generic_type_params":[],"params":["&signer","0x1::object::Object<0x1::fungible_asset::Metadata>"],"return":[]},
        {"name":"transfer","visibility":"public","is_entry":true,"is_view":false,
         "generic_type_params":[{"constraints":[]}],
         "params":["&signer","address","u64","0x1::string::String","vector<u8>","0x1::option::Option<u64>"],"return":[]},
        {"name":"get_config","visibility":"public","is_entry":false,"is_view":true,
         "generic_type_params":[],"params":[],"return":["u64"]}],
        "structs":[]}}"#;

    #[test]
    fn parses_a_module_and_lists_entry_functions() {
        let module: MoveModule = serde_json::from_str(MODULE).unwrap();
        let abi = module.abi.unwrap();
        assert_eq!(abi.name, "omni_bridge");
        let entries: Vec<&str> = abi
            .entry_functions()
            .map(|function| function.name.as_str())
            .collect();
        assert_eq!(entries, ["log_metadata", "transfer"]);

        let selected = abi.exposed_functions[1].selected("0xe4ec", "omni_bridge");
        assert_eq!(selected.path, "0xe4ec::omni_bridge::transfer");
        assert_eq!(selected.generic_count, 1);
        let kinds: Vec<ArgKind> = selected.params.into_iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ArgKind::Skip,
                ArgKind::Typed("address"),
                ArgKind::Typed("u64"),
                ArgKind::Typed("string"),
                ArgKind::Typed("hex"),
                ArgKind::Manual,
            ]
        );
    }

    #[test]
    fn maps_object_handles_and_references() {
        assert_eq!(
            arg_kind_for_aptos("0x1::object::Object<0x1::fungible_asset::Metadata>"),
            ArgKind::Typed("address")
        );
        assert_eq!(arg_kind_for_aptos("&signer"), ArgKind::Skip);
        assert_eq!(arg_kind_for_aptos("signer"), ArgKind::Skip);
        assert_eq!(arg_kind_for_aptos("&mut u64"), ArgKind::Typed("u64"));
        assert_eq!(arg_kind_for_aptos("T0"), ArgKind::Manual);
        assert_eq!(
            arg_kind_for_aptos("vector<address>"),
            ArgKind::Typed("vector<address>")
        );
    }
}
