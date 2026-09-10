//! Local-only, cast-style ABI encoding: the user types a function signature
//! like `transfer(address,uint256)` plus a JSON array of argument values,
//! and the calldata is encoded without any external ABI registry.

use alloy_dyn_abi::{DynSolType, DynSolValue, Specifier};
use alloy_json_abi::Function;
use color_eyre::eyre::{WrapErr, eyre};

/// Parses a human-readable function signature: `transfer(address,uint256)`,
/// `transfer(address to, uint256 amount)`, or the same with a leading
/// `function` keyword (the form ABIs are usually quoted in).
pub fn parse_function(signature: &str) -> color_eyre::eyre::Result<Function> {
    let signature = signature.trim();
    let signature = signature
        .strip_prefix("function ")
        .map_or(signature, str::trim_start);
    if signature.is_empty() {
        return Err(eyre!(
            "Function signature must look like `name(type1,type2)` or \
             `name(type1 param1, type2 param2)`"
        ));
    }
    Function::parse(signature).map_err(|err| {
        eyre!(
            "Invalid function signature `{signature}`: {err} (expected e.g. \
             `transfer(address to, uint256 amount)` or `pause()`)"
        )
    })
}

/// `name(type name, type name)` - the signature with parameter names kept,
/// the way the guided flow writes the chosen function back into the command.
pub fn human_signature(function: &Function) -> String {
    let params: Vec<String> = function
        .inputs
        .iter()
        .map(|param| {
            let ty = param.selector_type();
            if param.name.is_empty() {
                ty.into_owned()
            } else {
                format!("{ty} {}", param.name)
            }
        })
        .collect();
    format!("{}({})", function.name, params.join(", "))
}

/// Encodes calldata from a human-readable function signature and a JSON array
/// of argument values. Returns `(calldata, canonical_signature)`, the latter
/// being the types-only form (`transfer(address,uint256)`) that determines
/// the selector.
pub fn encode_calldata(
    signature: &str,
    args_json: &str,
) -> color_eyre::eyre::Result<(Vec<u8>, String)> {
    let function = parse_function(signature)?;
    let canonical_signature = function.signature();
    let component_types = function
        .inputs
        .iter()
        .map(|param| {
            param.resolve().map_err(|err| {
                eyre!(
                    "Failed to resolve parameter type `{}` in `{canonical_signature}`: {err}",
                    param.selector_type()
                )
            })
        })
        .collect::<color_eyre::eyre::Result<Vec<DynSolType>>>()?;

    let args_json = args_json.trim();
    let args: Vec<serde_json::Value> = if args_json.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(args_json).wrap_err(
            "Function arguments must be a JSON array, e.g. [\"0xabc...\", \"1000\"] ([] for none)",
        )?
    };
    if args.len() != component_types.len() {
        return Err(eyre!(
            "`{canonical_signature}` expects {} argument(s), got {}",
            component_types.len(),
            args.len()
        ));
    }

    let mut values = Vec::with_capacity(args.len());
    for (index, (arg, ty)) in args.iter().zip(&component_types).enumerate() {
        let arg_str = match arg {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let value = ty
            .coerce_str(&arg_str)
            .wrap_err_with(|| format!("Argument #{index} ('{arg_str}') is not a valid `{ty}`"))?;
        values.push(value);
    }

    let encoded_args = DynSolValue::Tuple(values).abi_encode_params();
    let mut calldata = function.selector().to_vec();
    calldata.extend_from_slice(&encoded_args);
    Ok((calldata, canonical_signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_no_arg_function() {
        let (calldata, canonical) = encode_calldata("pause()", "[]").unwrap();
        assert_eq!(canonical, "pause()");
        // selector of pause() is 0x8456cb59
        assert_eq!(calldata, vec![0x84, 0x56, 0xcb, 0x59]);
    }

    #[test]
    fn encodes_erc20_transfer() {
        let (calldata, canonical) = encode_calldata(
            "transfer(address,uint256)",
            r#"["0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", "1000"]"#,
        )
        .unwrap();
        assert_eq!(canonical, "transfer(address,uint256)");
        // selector of transfer(address,uint256) is 0xa9059cbb
        assert_eq!(&calldata[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(calldata.len(), 4 + 32 + 32);
        assert_eq!(calldata[4 + 32 + 31], 0xe8); // 1000 = 0x3e8
    }

    #[test]
    fn encodes_single_arg_function() {
        // Regression: a one-element tuple must not render as `(address,)`.
        let (calldata, canonical) = encode_calldata(
            "balanceOf(address)",
            r#"["0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"]"#,
        )
        .unwrap();
        assert_eq!(canonical, "balanceOf(address)");
        // selector of balanceOf(address) is 0x70a08231
        assert_eq!(&calldata[..4], &[0x70, 0xa0, 0x82, 0x31]);
        assert_eq!(calldata.len(), 4 + 32);
    }

    #[test]
    fn normalizes_uint_alias_for_selector() {
        let (a, _) = encode_calldata("f(uint)", "[\"1\"]").unwrap();
        let (b, _) = encode_calldata("f(uint256)", "[\"1\"]").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn accepts_named_parameters_and_function_keyword() {
        let (bare, canonical) = encode_calldata(
            "transfer(address,uint256)",
            r#"["0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", "1000"]"#,
        )
        .unwrap();
        let (named, canonical_named) = encode_calldata(
            "function transfer(address to, uint256 amount)",
            r#"["0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", "1000"]"#,
        )
        .unwrap();
        assert_eq!(bare, named);
        assert_eq!(canonical, canonical_named);
        assert_eq!(canonical_named, "transfer(address,uint256)");
    }

    #[test]
    fn named_single_arg_selector() {
        let (calldata, _) = encode_calldata(
            "balanceOf(address account)",
            r#"["0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"]"#,
        )
        .unwrap();
        assert_eq!(&calldata[..4], &[0x70, 0xa0, 0x82, 0x31]);
    }

    #[test]
    fn human_signature_keeps_names_and_tuples() {
        let function =
            parse_function("swap((address,uint256) order, bytes sig, uint8[] flags)").unwrap();
        assert_eq!(
            human_signature(&function),
            "swap((address,uint256) order, bytes sig, uint8[] flags)"
        );
        assert_eq!(
            human_signature(&parse_function("pause()").unwrap()),
            "pause()"
        );
        assert_eq!(
            human_signature(&parse_function("f(address,uint256 b)").unwrap()),
            "f(address, uint256 b)"
        );
    }

    #[test]
    fn rejects_wrong_arity() {
        assert!(encode_calldata("transfer(address,uint256)", "[]").is_err());
    }
}
