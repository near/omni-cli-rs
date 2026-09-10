//! Family-agnostic pieces of a Move entry-function call, shared by the Aptos
//! and Sui adapters: parsing a Move type from text (`u64`, `address`,
//! `vector<u8>`, `0x1::aptos_coin::AptosCoin<...>`) and BCS-encoding a typed
//! argument written Aptos-CLI style as `type:value` (`u64:100`,
//! `address:0x1`, `string:hello`, `hex:0xdead`, `bool:true`).
//!
//! Both families use BCS for pure values and the same type grammar, so the
//! adapters only map [`MoveType`] onto their own `TypeTag` enums.

use color_eyre::eyre::{WrapErr, eyre};

/// A parsed Move type. Struct addresses stay as 32-byte arrays so each
/// family can wrap them in its own address type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    U256,
    Address,
    Signer,
    Vector(Box<MoveType>),
    Struct {
        address: [u8; 32],
        module: String,
        name: String,
        type_args: Vec<MoveType>,
    },
}

/// One argument of a call: a BCS-encoded pure value, or (Sui only) an
/// on-chain object passed by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveArg {
    Pure { bytes: Vec<u8>, display: String },
    Object { id: [u8; 32], display: String },
}

/// Parses a 32-byte Move address from `0x`-prefixed hex, left-padded (so
/// `0x1` is the framework address).
pub fn parse_address(text: &str) -> color_eyre::eyre::Result<[u8; 32]> {
    let hex_str = text.trim();
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if hex_str.is_empty() || hex_str.len() > 64 {
        return Err(eyre!(
            "Invalid Move address '{text}': expected up to 64 hex characters"
        ));
    }
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(format!("{hex_str:0>64}"), &mut bytes)
        .wrap_err_with(|| format!("Invalid Move address '{text}'"))?;
    Ok(bytes)
}

/// Splits `0xaddr::module::name` into its three parts.
pub fn parse_function_path(text: &str) -> color_eyre::eyre::Result<([u8; 32], String, String)> {
    let parts: Vec<&str> = text.trim().split("::").collect();
    let [address, module, name] = parts.as_slice() else {
        return Err(eyre!("Expected <address>::<module>::<name>, got '{text}'"));
    };
    for ident in [module, name] {
        if ident.is_empty() || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(eyre!("Invalid Move identifier '{ident}' in '{text}'"));
        }
    }
    Ok((
        parse_address(address)?,
        (*module).to_string(),
        (*name).to_string(),
    ))
}

/// Parses a Move type. Generics use `<...>`; nesting is supported.
pub fn parse_type(text: &str) -> color_eyre::eyre::Result<MoveType> {
    let text = text.trim();
    let primitive = match text {
        "bool" => Some(MoveType::Bool),
        "u8" => Some(MoveType::U8),
        "u16" => Some(MoveType::U16),
        "u32" => Some(MoveType::U32),
        "u64" => Some(MoveType::U64),
        "u128" => Some(MoveType::U128),
        "u256" => Some(MoveType::U256),
        "address" => Some(MoveType::Address),
        "signer" => Some(MoveType::Signer),
        _ => None,
    };
    if let Some(primitive) = primitive {
        return Ok(primitive);
    }
    if let Some(inner) = text
        .strip_prefix("vector<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return Ok(MoveType::Vector(Box::new(parse_type(inner)?)));
    }
    // Struct: path, optionally followed by <T1, T2<...>, ...>
    let (path, generics) = match text.find('<') {
        Some(open) => {
            let close = text
                .strip_suffix('>')
                .map(|_| text.len() - 1)
                .ok_or_else(|| eyre!("Unbalanced '<' in Move type '{text}'"))?;
            (&text[..open], Some(&text[open + 1..close]))
        }
        None => (text, None),
    };
    let (address, module, name) =
        parse_function_path(path).wrap_err_with(|| format!("Invalid Move type '{text}'"))?;
    let type_args = match generics {
        Some(list) => split_top_level(list)
            .into_iter()
            .map(parse_type)
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    Ok(MoveType::Struct {
        address,
        module,
        name,
        type_args,
    })
}

/// Splits on commas that are not nested inside `<...>`.
fn split_top_level(list: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in list.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&list[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if !list[start..].trim().is_empty() {
        parts.push(&list[start..]);
    }
    parts
}

/// BCS ULEB128 length prefix.
fn uleb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn bcs_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = uleb128(bytes.len());
    out.extend_from_slice(bytes);
    out
}

/// Encodes one `type:value` argument. Supported types: `bool`, `u8`..`u256`,
/// `address`, `string`, `hex` (raw bytes, `vector<u8>`), `vector<address>`
/// (comma-separated), `object` (Sui object id, resolved on-chain later).
pub fn parse_arg(text: &str) -> color_eyre::eyre::Result<MoveArg> {
    let (kind, value) = text
        .split_once(':')
        .ok_or_else(|| eyre!("Argument '{text}' must be written as type:value (e.g. u64:100)"))?;
    let (kind, value) = (kind.trim(), value.trim());
    let display = format!("{kind}:{value}");
    let bytes = match kind {
        "bool" => vec![u8::from(
            value
                .parse::<bool>()
                .wrap_err_with(|| format!("Invalid bool '{value}'"))?,
        )],
        "u8" => parse_uint::<u8>(value)?.to_le_bytes().to_vec(),
        "u16" => parse_uint::<u16>(value)?.to_le_bytes().to_vec(),
        "u32" => parse_uint::<u32>(value)?.to_le_bytes().to_vec(),
        "u64" => parse_uint::<u64>(value)?.to_le_bytes().to_vec(),
        "u128" => parse_uint::<u128>(value)?.to_le_bytes().to_vec(),
        "u256" => {
            let big = alloy_primitives::U256::from_str_radix(value, 10)
                .wrap_err_with(|| format!("Invalid u256 '{value}'"))?;
            big.to_le_bytes::<32>().to_vec()
        }
        "address" => parse_address(value)?.to_vec(),
        "string" => bcs_bytes(value.as_bytes()),
        "hex" | "vector<u8>" => {
            let raw = hex::decode(value.strip_prefix("0x").unwrap_or(value))
                .wrap_err_with(|| format!("Invalid hex '{value}'"))?;
            bcs_bytes(&raw)
        }
        "vector<address>" => {
            let items: Vec<[u8; 32]> = value
                .split(',')
                .filter(|item| !item.trim().is_empty())
                .map(parse_address)
                .collect::<color_eyre::eyre::Result<_>>()?;
            let mut out = uleb128(items.len());
            for item in items {
                out.extend_from_slice(&item);
            }
            out
        }
        "object" => {
            return Ok(MoveArg::Object {
                id: parse_address(value)?,
                display,
            });
        }
        other => {
            return Err(eyre!(
                "Unsupported argument type '{other}' (supported: bool, u8, u16, u32, u64, \
                 u128, u256, address, string, hex, vector<address>, object)"
            ));
        }
    };
    Ok(MoveArg::Pure { bytes, display })
}

fn parse_uint<T: std::str::FromStr>(value: &str) -> color_eyre::eyre::Result<T>
where
    T::Err: std::fmt::Display,
{
    value
        .replace('_', "")
        .parse::<T>()
        .map_err(|err| eyre!("Invalid integer '{value}': {err}"))
}

/// Parses the JSON-array-of-strings form used on the command line for type
/// arguments and call arguments (`'["u64:1", "address:0x1"]'`).
pub fn parse_string_list(json: &str) -> color_eyre::eyre::Result<Vec<String>> {
    let json = json.trim();
    if json.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<String>>(json).wrap_err_with(|| {
        format!("Expected a JSON array of strings like '[\"u64:100\"]', got: {json}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_types_including_nested_generics() {
        assert_eq!(parse_type("u64").unwrap(), MoveType::U64);
        assert_eq!(
            parse_type("vector<vector<u8>>").unwrap(),
            MoveType::Vector(Box::new(MoveType::Vector(Box::new(MoveType::U8))))
        );
        let coin = parse_type("0x1::coin::Coin<0x1::aptos_coin::AptosCoin>").unwrap();
        match coin {
            MoveType::Struct {
                address,
                module,
                name,
                type_args,
            } => {
                assert_eq!(address[31], 1);
                assert_eq!((module.as_str(), name.as_str()), ("coin", "Coin"));
                assert_eq!(type_args.len(), 1);
            }
            other => panic!("unexpected {other:?}"),
        }
        let pair = parse_type("0x2::pair::Pair<u8, vector<u64>>").unwrap();
        let MoveType::Struct { type_args, .. } = pair else {
            panic!()
        };
        assert_eq!(type_args.len(), 2);
        assert!(parse_type("0x1::coin").is_err());
        assert!(parse_type("0x1::coin::Coin<u8").is_err());
    }

    /// BCS vectors: reference encodings from the Move/BCS spec.
    #[test]
    fn encodes_args_as_bcs() {
        let bytes = |text: &str| match parse_arg(text).unwrap() {
            MoveArg::Pure { bytes, .. } => bytes,
            MoveArg::Object { .. } => panic!("expected pure"),
        };
        assert_eq!(bytes("u64:100"), 100u64.to_le_bytes());
        assert_eq!(bytes("u8:255"), [255]);
        assert_eq!(bytes("bool:true"), [1]);
        assert_eq!(bytes("string:hi"), [2, b'h', b'i']);
        assert_eq!(bytes("hex:0xdead"), [2, 0xde, 0xad]);
        assert_eq!(bytes("address:0x1")[31], 1);
        assert_eq!(bytes("address:0x1").len(), 32);
        assert_eq!(bytes("u256:1")[0], 1);
        assert_eq!(bytes("u256:1").len(), 32);
        let two = bytes("vector<address>:0x1,0x2");
        assert_eq!(two[0], 2);
        assert_eq!(two.len(), 65);
        assert!(matches!(
            parse_arg("object:0xabc").unwrap(),
            MoveArg::Object { .. }
        ));
        assert!(parse_arg("u64:-1").is_err());
        assert!(parse_arg("nonsense").is_err());
        assert!(parse_arg("float:1.5").is_err());
    }

    #[test]
    fn uleb128_matches_bcs() {
        assert_eq!(uleb128(0), [0]);
        assert_eq!(uleb128(127), [127]);
        assert_eq!(uleb128(128), [0x80, 0x01]);
        assert_eq!(uleb128(300), [0xac, 0x02]);
    }
}
