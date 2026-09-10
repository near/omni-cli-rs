//! The exposed-function interface of a Sui Move module, as fullnodes
//! publish it (`sui_getNormalizedMoveModule`), and the mapping from
//! declared parameter types to guided `type:value` input.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::chains::move_call::{ArgKind, GuidedParam, SelectedMoveFunction};

/// A normalized module: `exposedFunctions` keyed by name.
#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedModule {
    #[serde(rename = "exposedFunctions", default)]
    pub exposed_functions: BTreeMap<String, NormalizedFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedFunction {
    /// `Public`, `Private`, or `Friend`.
    pub visibility: String,
    #[serde(rename = "isEntry")]
    pub is_entry: bool,
    /// Ability constraints per type parameter; only the count matters here.
    #[serde(rename = "typeParameters", default)]
    pub type_parameters: Vec<serde_json::Value>,
    #[serde(default)]
    pub parameters: Vec<NormalizedType>,
}

/// A Move type as the normalized-module API encodes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum NormalizedType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    U256,
    Address,
    Signer,
    Vector(Box<NormalizedType>),
    Struct {
        address: String,
        module: String,
        name: String,
        #[serde(rename = "typeArguments", default)]
        type_arguments: Vec<NormalizedType>,
    },
    Reference(Box<NormalizedType>),
    MutableReference(Box<NormalizedType>),
    TypeParameter(u16),
}

impl fmt::Display for NormalizedType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::U8 => write!(f, "u8"),
            Self::U16 => write!(f, "u16"),
            Self::U32 => write!(f, "u32"),
            Self::U64 => write!(f, "u64"),
            Self::U128 => write!(f, "u128"),
            Self::U256 => write!(f, "u256"),
            Self::Address => write!(f, "address"),
            Self::Signer => write!(f, "signer"),
            Self::Vector(inner) => write!(f, "vector<{inner}>"),
            Self::Struct {
                address,
                module,
                name,
                type_arguments,
            } => {
                write!(f, "{address}::{module}::{name}")?;
                if !type_arguments.is_empty() {
                    let args: Vec<String> =
                        type_arguments.iter().map(ToString::to_string).collect();
                    write!(f, "<{}>", args.join(", "))?;
                }
                Ok(())
            }
            Self::Reference(inner) => write!(f, "&{inner}"),
            Self::MutableReference(inner) => write!(f, "&mut {inner}"),
            Self::TypeParameter(index) => write!(f, "T{index}"),
        }
    }
}

impl NormalizedModule {
    /// The functions a programmable transaction can call: entry functions
    /// and public ones.
    pub fn callable_functions(&self) -> impl Iterator<Item = (&String, &NormalizedFunction)> {
        self.exposed_functions
            .iter()
            .filter(|(_, function)| function.is_entry || function.visibility == "Public")
    }
}

impl NormalizedFunction {
    pub fn guided_params(&self) -> Vec<GuidedParam> {
        self.parameters
            .iter()
            .map(|param| GuidedParam {
                type_text: param.to_string(),
                kind: arg_kind_for_sui(param),
            })
            .collect()
    }

    pub fn selected(&self, package: &str, module: &str, name: &str) -> SelectedMoveFunction {
        SelectedMoveFunction {
            path: format!("{package}::{module}::{name}"),
            generic_count: self.type_parameters.len(),
            params: self.guided_params(),
        }
    }
}

fn is_framework_struct(ty: &NormalizedType, framework: u64, module: &str, name: &str) -> bool {
    let NormalizedType::Struct {
        address,
        module: got_module,
        name: got_name,
        ..
    } = ty
    else {
        return false;
    };
    got_module == module
        && got_name == name
        && crate::chains::move_call::parse_address(address)
            .is_ok_and(|bytes| bytes == u256_address(framework))
}

fn u256_address(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    bytes
}

/// How a declared Sui parameter is collected. `TxContext` is supplied by
/// the runtime; every other struct (by value or reference) is an on-chain
/// object passed by id, except the pure-value structs `String` and `ID`.
pub fn arg_kind_for_sui(ty: &NormalizedType) -> ArgKind {
    let inner = match ty {
        NormalizedType::Reference(inner) | NormalizedType::MutableReference(inner) => inner,
        other => other,
    };
    match inner {
        NormalizedType::Bool => ArgKind::Typed("bool"),
        NormalizedType::U8 => ArgKind::Typed("u8"),
        NormalizedType::U16 => ArgKind::Typed("u16"),
        NormalizedType::U32 => ArgKind::Typed("u32"),
        NormalizedType::U64 => ArgKind::Typed("u64"),
        NormalizedType::U128 => ArgKind::Typed("u128"),
        NormalizedType::U256 => ArgKind::Typed("u256"),
        NormalizedType::Address => ArgKind::Typed("address"),
        NormalizedType::Vector(item) if **item == NormalizedType::U8 => ArgKind::Typed("hex"),
        NormalizedType::Vector(item) if **item == NormalizedType::Address => {
            ArgKind::Typed("vector<address>")
        }
        NormalizedType::Struct { .. } => {
            if is_framework_struct(inner, 2, "tx_context", "TxContext") {
                ArgKind::Skip
            } else if is_framework_struct(inner, 1, "string", "String")
                || is_framework_struct(inner, 1, "ascii", "String")
            {
                ArgKind::Typed("string")
            } else if is_framework_struct(inner, 2, "object", "ID") {
                ArgKind::Typed("address")
            } else {
                ArgKind::Typed("object")
            }
        }
        NormalizedType::Signer
        | NormalizedType::Vector(_)
        | NormalizedType::Reference(_)
        | NormalizedType::MutableReference(_)
        | NormalizedType::TypeParameter(_) => ArgKind::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = r#"{"fileFormatVersion":6,"address":"0x2","name":"pay","friends":[],"structs":{},
        "exposedFunctions":{
          "split_and_transfer":{"visibility":"Public","isEntry":true,"typeParameters":[{"abilities":[]}],
            "parameters":[
              {"MutableReference":{"Struct":{"address":"0x2","module":"coin","name":"Coin","typeArguments":[{"TypeParameter":0}]}}},
              "U64","Address",
              {"MutableReference":{"Struct":{"address":"0x2","module":"tx_context","name":"TxContext","typeArguments":[]}}}],
            "return":[]},
          "helper":{"visibility":"Private","isEntry":false,"typeParameters":[],"parameters":["U8"],"return":[]},
          "with_name":{"visibility":"Public","isEntry":false,"typeParameters":[],
            "parameters":[{"Struct":{"address":"0x1","module":"string","name":"String","typeArguments":[]}},
                          {"Struct":{"address":"0x2","module":"object","name":"ID","typeArguments":[]}},
                          {"Vector":"U8"},{"Vector":"Address"},{"Vector":"U64"}],
            "return":[]}}}"#;

    #[test]
    fn parses_a_normalized_module_and_maps_parameter_kinds() {
        let module: NormalizedModule = serde_json::from_str(MODULE).unwrap();
        let callable: Vec<&str> = module
            .callable_functions()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(callable, ["split_and_transfer", "with_name"]);

        let split = &module.exposed_functions["split_and_transfer"];
        let selected = split.selected("0x2", "pay", "split_and_transfer");
        assert_eq!(selected.path, "0x2::pay::split_and_transfer");
        assert_eq!(selected.generic_count, 1);
        let texts: Vec<&str> = selected
            .params
            .iter()
            .map(|param| param.type_text.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                "&mut 0x2::coin::Coin<T0>",
                "u64",
                "address",
                "&mut 0x2::tx_context::TxContext"
            ]
        );
        let kinds: Vec<ArgKind> = selected.params.into_iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ArgKind::Typed("object"),
                ArgKind::Typed("u64"),
                ArgKind::Typed("address"),
                ArgKind::Skip
            ]
        );

        let kinds: Vec<ArgKind> = module.exposed_functions["with_name"]
            .guided_params()
            .into_iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                ArgKind::Typed("string"),
                ArgKind::Typed("address"),
                ArgKind::Typed("hex"),
                ArgKind::Typed("vector<address>"),
                ArgKind::Manual
            ]
        );
    }

    #[test]
    fn framework_addresses_match_padded_forms() {
        let ctx: NormalizedType = serde_json::from_str(
            r#"{"Reference":{"Struct":{"address":"0x0000000000000000000000000000000000000000000000000000000000000002","module":"tx_context","name":"TxContext","typeArguments":[]}}}"#,
        )
        .unwrap();
        assert_eq!(arg_kind_for_sui(&ctx), ArgKind::Skip);
    }
}
