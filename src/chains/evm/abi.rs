//! Local-only, cast-style ABI encoding: the user types a function signature
//! like `transfer(address,uint256)` plus a JSON array of argument values,
//! and the calldata is encoded without any external ABI registry.

use alloy_dyn_abi::{DynSolType, DynSolValue};
use color_eyre::eyre::{WrapErr, eyre};

/// Encodes calldata from a human-readable function signature and a JSON array
/// of argument values. Returns `(calldata, canonical_signature)`.
pub fn encode_calldata(
    signature: &str,
    args_json: &str,
) -> color_eyre::eyre::Result<(Vec<u8>, String)> {
    let signature = signature.trim();
    let open = signature
        .find('(')
        .ok_or_else(|| eyre!("Function signature must look like `name(type1,type2)`"))?;
    if !signature.ends_with(')') {
        return Err(eyre!("Function signature must end with `)`"));
    }
    let name = signature[..open].trim();
    if name.is_empty() {
        return Err(eyre!("Function signature is missing the function name"));
    }
    let params = &signature[open..];

    let tuple_type = DynSolType::parse(params)
        .wrap_err_with(|| format!("Failed to parse parameter types in `{signature}`"))?;
    let DynSolType::Tuple(component_types) = &tuple_type else {
        return Err(eyre!("Failed to parse `{params}` as a parameter list"));
    };

    // Canonical signature (aliases like `uint` normalized to `uint256`)
    // determines the 4-byte selector.
    let canonical_signature = format!("{name}{}", tuple_type.sol_type_name());
    let selector = &alloy_primitives::keccak256(canonical_signature.as_bytes())[..4];

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
    for (index, (arg, ty)) in args.iter().zip(component_types).enumerate() {
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
    let mut calldata = selector.to_vec();
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
    fn normalizes_uint_alias_for_selector() {
        let (a, _) = encode_calldata("f(uint)", "[\"1\"]").unwrap();
        let (b, _) = encode_calldata("f(uint256)", "[\"1\"]").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_wrong_arity() {
        assert!(encode_calldata("transfer(address,uint256)", "[]").is_err());
    }
}
