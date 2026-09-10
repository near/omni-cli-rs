//! Anchor IDL support for the guided `instruction` flow: parse a program's
//! published interface (the on-chain IDL account, a file, or a URL), list
//! its instructions, Borsh-encode arguments from typed prompts, and derive
//! every account the IDL fully describes (fixed addresses, PDAs whose seeds
//! are constants, other accounts, or arguments) so the user only supplies
//! what the program genuinely cannot know.
//!
//! Both IDL generations are read into one model: the Anchor >= 0.30 format
//! (`writable`/`signer`, explicit `discriminator`, composite account
//! groups) and the legacy one (`isMut`/`isSigner`, camelCase names, no
//! discriminator).

use super::SYSTEM_PROGRAM;
use std::collections::BTreeMap;
use std::io::Read;

use alloy_primitives::U256;
use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use omni_transaction::solana::types::SolanaAddress;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::chains::svm::InstructionAccountKey;

// ------------------------------------------------------------------ model

/// A parsed IDL (either format), instruction accounts flattened.
#[derive(Debug, Clone)]
pub struct Idl {
    pub name: String,
    pub address: Option<SolanaAddress>,
    pub instructions: Vec<IdlInstruction>,
    pub types: BTreeMap<String, IdlTypeDef>,
}

#[derive(Debug, Clone)]
pub struct IdlInstruction {
    pub name: String,
    pub docs: Vec<String>,
    pub discriminator: [u8; 8],
    /// Depth-first flattened: composite groups contribute their leaves with
    /// dotted paths (`common.config`).
    pub accounts: Vec<IdlAccount>,
    pub args: Vec<IdlArg>,
}

#[derive(Debug, Clone)]
pub struct IdlAccount {
    /// Dotted path inside the instruction's account struct.
    pub path: String,
    /// The leaf name (last path segment).
    pub name: String,
    pub docs: Vec<String>,
    pub writable: bool,
    pub signer: bool,
    pub optional: bool,
    pub address: Option<SolanaAddress>,
    pub pda: Option<IdlPda>,
}

#[derive(Debug, Clone)]
pub struct IdlPda {
    pub seeds: Vec<IdlSeed>,
    /// The program the PDA belongs to; the instruction's program when absent.
    pub program: Option<IdlSeed>,
}

#[derive(Debug, Clone)]
pub enum IdlSeed {
    Const(Vec<u8>),
    /// Another account of the instruction (by name or dotted path). When
    /// the path does not name an account, Anchor means a field read out of
    /// an account's data (`user.owner`); `account_type` then names that
    /// account's type.
    Account {
        path: String,
        account_type: Option<String>,
    },
    /// An instruction argument (by name).
    Arg(String),
    /// A seed kind this CLI cannot evaluate, with the reason.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct IdlArg {
    pub name: String,
    pub ty: IdlType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdlType {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    U128,
    I128,
    U256,
    I256,
    F32,
    F64,
    Bytes,
    String,
    Pubkey,
    Vec(Box<IdlType>),
    Option(Box<IdlType>),
    COption(Box<IdlType>),
    Array(Box<IdlType>, usize),
    Defined(String),
    Unsupported(String),
}

impl std::fmt::Display for IdlType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::U8 => write!(f, "u8"),
            Self::I8 => write!(f, "i8"),
            Self::U16 => write!(f, "u16"),
            Self::I16 => write!(f, "i16"),
            Self::U32 => write!(f, "u32"),
            Self::I32 => write!(f, "i32"),
            Self::U64 => write!(f, "u64"),
            Self::I64 => write!(f, "i64"),
            Self::U128 => write!(f, "u128"),
            Self::I128 => write!(f, "i128"),
            Self::U256 => write!(f, "u256"),
            Self::I256 => write!(f, "i256"),
            Self::F32 => write!(f, "f32"),
            Self::F64 => write!(f, "f64"),
            Self::Bytes => write!(f, "bytes"),
            Self::String => write!(f, "string"),
            Self::Pubkey => write!(f, "pubkey"),
            Self::Vec(inner) => write!(f, "Vec<{inner}>"),
            Self::Option(inner) => write!(f, "Option<{inner}>"),
            Self::COption(inner) => write!(f, "COption<{inner}>"),
            Self::Array(inner, len) => write!(f, "[{inner}; {len}]"),
            Self::Defined(name) => write!(f, "{name}"),
            Self::Unsupported(what) => write!(f, "<unsupported: {what}>"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum IdlTypeDef {
    Struct(Vec<IdlField>),
    Enum(Vec<IdlVariant>),
}

#[derive(Debug, Clone)]
pub struct IdlField {
    pub name: String,
    pub ty: IdlType,
}

#[derive(Debug, Clone)]
pub struct IdlVariant {
    pub name: String,
    pub fields: VariantFields,
}

#[derive(Debug, Clone)]
pub enum VariantFields {
    Unit,
    Named(Vec<IdlField>),
    Tuple(Vec<IdlType>),
}

impl IdlInstruction {
    /// `name(arg: type, ...)`, the way the instruction is listed.
    pub fn signature(&self) -> String {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|arg| format!("{}: {}", arg.name, arg.ty))
            .collect();
        format!("{}({})", self.name, args.join(", "))
    }

    /// The instruction's account by leaf name or dotted path (name first,
    /// then exact path, then last path segment).
    pub fn account(&self, reference: &str) -> Option<&IdlAccount> {
        self.accounts
            .iter()
            .find(|account| account.name == reference)
            .or_else(|| {
                self.accounts
                    .iter()
                    .find(|account| account.path == reference)
            })
            .or_else(|| {
                let last = reference.rsplit('.').next()?;
                self.accounts.iter().find(|account| account.name == last)
            })
    }
}

// ---------------------------------------------------------------- parsing

/// The raw JSON shapes, tolerant of both IDL generations.
#[derive(Deserialize)]
struct RawIdl {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    metadata: Option<RawMetadata>,
    instructions: Vec<RawInstruction>,
    #[serde(default)]
    types: Vec<RawTypeDef>,
}

#[derive(Deserialize)]
struct RawMetadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<String>,
}

#[derive(Deserialize)]
struct RawInstruction {
    name: String,
    #[serde(default)]
    docs: Vec<String>,
    #[serde(default)]
    discriminator: Option<Vec<u8>>,
    #[serde(default)]
    accounts: Vec<RawAccountEntry>,
    #[serde(default)]
    args: Vec<RawField>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAccountEntry {
    Group {
        name: String,
        accounts: Vec<RawAccountEntry>,
    },
    Leaf(RawAccount),
}

#[derive(Deserialize)]
#[allow(clippy::struct_excessive_bools, reason = "mirrors the IDL's flags")]
struct RawAccount {
    name: String,
    #[serde(default)]
    docs: Vec<String>,
    #[serde(default)]
    writable: bool,
    #[serde(default)]
    signer: bool,
    #[serde(default)]
    optional: bool,
    #[serde(default, rename = "isMut")]
    is_mut: bool,
    #[serde(default, rename = "isSigner")]
    is_signer: bool,
    #[serde(default, rename = "isOptional")]
    is_optional: bool,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    pda: Option<RawPda>,
}

#[derive(Deserialize)]
struct RawPda {
    #[serde(default)]
    seeds: Vec<RawSeed>,
    #[serde(default)]
    program: Option<RawSeed>,
    #[serde(default, rename = "programId")]
    program_id: Option<RawSeed>,
}

#[derive(Deserialize)]
struct RawSeed {
    kind: String,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default, rename = "type")]
    ty: Option<RawType>,
}

#[derive(Deserialize)]
struct RawField {
    name: String,
    #[serde(rename = "type")]
    ty: RawType,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawType {
    Name(String),
    Vec { vec: Box<RawType> },
    Option { option: Box<RawType> },
    COption { coption: Box<RawType> },
    Array { array: (Box<RawType>, usize) },
    Defined { defined: RawDefined },
    Other(serde_json::Value),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawDefined {
    Name(String),
    Object { name: String },
}

#[derive(Deserialize)]
struct RawTypeDef {
    name: String,
    #[serde(rename = "type")]
    ty: RawTypeDefKind,
}

#[derive(Deserialize)]
struct RawTypeDefKind {
    kind: String,
    #[serde(default)]
    fields: Option<RawFields>,
    #[serde(default)]
    variants: Option<Vec<RawVariant>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawFields {
    Named(Vec<RawField>),
    Tuple(Vec<RawType>),
}

#[derive(Deserialize)]
struct RawVariant {
    name: String,
    #[serde(default)]
    fields: Option<RawFields>,
}

/// Parses IDL JSON of either format.
pub fn parse_idl_json(json: &str) -> color_eyre::eyre::Result<Idl> {
    let raw: RawIdl = serde_json::from_str(json).wrap_err("Not an Anchor IDL")?;
    let name = raw
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.name.clone())
        .or(raw.name)
        .unwrap_or_else(|| "program".to_string());
    let address = raw
        .address
        .or_else(|| raw.metadata.and_then(|metadata| metadata.address))
        .map(|text| SolanaAddress::from_base58(&text))
        .transpose()
        .map_err(|err| eyre!("Invalid program address in the IDL: {err}"))?;
    let instructions = raw
        .instructions
        .into_iter()
        .map(convert_instruction)
        .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
    let types = raw
        .types
        .into_iter()
        .map(|def| Ok((def.name, convert_type_def(def.ty)?)))
        .collect::<color_eyre::eyre::Result<BTreeMap<_, _>>>()?;
    Ok(Idl {
        name,
        address,
        instructions,
        types,
    })
}

/// Parses the on-chain IDL account: 8-byte discriminator, 32-byte
/// authority, u32 length, zlib-compressed JSON.
pub fn parse_idl_account(data: &[u8]) -> color_eyre::eyre::Result<Idl> {
    const HEADER: usize = 8 + 32 + 4;
    if data.len() < HEADER {
        return Err(eyre!(
            "The IDL account holds {} bytes, fewer than its header",
            data.len()
        ));
    }
    let len = u32::from_le_bytes(data[40..44].try_into().expect("4 bytes")) as usize;
    let compressed = data
        .get(HEADER..HEADER + len)
        .ok_or_else(|| eyre!("The IDL account's declared length exceeds its data"))?;
    let mut json = String::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_string(&mut json)
        .wrap_err("The IDL account is not zlib-compressed JSON")?;
    parse_idl_json(&json)
}

fn convert_instruction(raw: RawInstruction) -> color_eyre::eyre::Result<IdlInstruction> {
    let discriminator = match raw.discriminator {
        Some(bytes) => bytes
            .try_into()
            .map_err(|_| eyre!("Instruction '{}' has a non-8-byte discriminator", raw.name))?,
        None => anchor_discriminator(&raw.name),
    };
    let mut accounts = Vec::new();
    for entry in raw.accounts {
        flatten_account(entry, "", &mut accounts)?;
    }
    let args = raw
        .args
        .into_iter()
        .map(|field| IdlArg {
            name: field.name,
            ty: convert_type(field.ty),
        })
        .collect();
    Ok(IdlInstruction {
        name: raw.name,
        docs: raw.docs,
        discriminator,
        accounts,
        args,
    })
}

fn flatten_account(
    entry: RawAccountEntry,
    prefix: &str,
    out: &mut Vec<IdlAccount>,
) -> color_eyre::eyre::Result<()> {
    match entry {
        RawAccountEntry::Group { name, accounts } => {
            let path = join_path(prefix, &name);
            for inner in accounts {
                flatten_account(inner, &path, out)?;
            }
        }
        RawAccountEntry::Leaf(account) => {
            let path = join_path(prefix, &account.name);
            let address = account
                .address
                .map(|text| SolanaAddress::from_base58(&text))
                .transpose()
                .map_err(|err| eyre!("Account '{path}' has an invalid fixed address: {err}"))?;
            let pda = account.pda.map(|pda| convert_pda(pda, &path)).transpose()?;
            out.push(IdlAccount {
                path,
                name: account.name,
                docs: account.docs,
                writable: account.writable || account.is_mut,
                signer: account.signer || account.is_signer,
                optional: account.optional || account.is_optional,
                address,
                pda,
            });
        }
    }
    Ok(())
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

fn convert_pda(raw: RawPda, path: &str) -> color_eyre::eyre::Result<IdlPda> {
    let seeds = raw
        .seeds
        .into_iter()
        .map(|seed| convert_seed(seed, path))
        .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
    let program = raw
        .program
        .or(raw.program_id)
        .map(|seed| convert_seed(seed, path))
        .transpose()?;
    Ok(IdlPda { seeds, program })
}

fn convert_seed(raw: RawSeed, path: &str) -> color_eyre::eyre::Result<IdlSeed> {
    Ok(match raw.kind.as_str() {
        "const" => IdlSeed::Const(const_seed_bytes(raw.value, raw.ty.map(convert_type), path)?),
        "account" => IdlSeed::Account {
            path: raw
                .path
                .wrap_err_with(|| format!("Account '{path}' has a seed without a path"))?,
            account_type: raw.account,
        },
        "arg" => IdlSeed::Arg(
            raw.path
                .wrap_err_with(|| format!("Account '{path}' has an arg seed without a path"))?,
        ),
        other => IdlSeed::Unsupported(format!("seed kind '{other}'")),
    })
}

/// The bytes of a constant seed. New IDLs give a byte array; legacy ones a
/// typed value (a string, a base58 pubkey, an integer, or a byte array).
fn const_seed_bytes(
    value: Option<serde_json::Value>,
    ty: Option<IdlType>,
    path: &str,
) -> color_eyre::eyre::Result<Vec<u8>> {
    let value =
        value.wrap_err_with(|| format!("Account '{path}' has a const seed without a value"))?;
    match (&value, ty) {
        (serde_json::Value::Array(_), _) => bytes_from_json_array(&value),
        (serde_json::Value::String(text), Some(IdlType::Pubkey)) => {
            Ok(SolanaAddress::from_base58(text)
                .map_err(|err| eyre!("Account '{path}' has an invalid pubkey seed: {err}"))?
                .0
                .to_vec())
        }
        (serde_json::Value::String(text), Some(ty)) if ty.is_integer() => encode_integer(&ty, text),
        (serde_json::Value::String(text), _) => Ok(text.as_bytes().to_vec()),
        (serde_json::Value::Number(number), ty) => {
            encode_integer(&ty.unwrap_or(IdlType::U64), &number.to_string())
        }
        (other, _) => Err(eyre!(
            "Account '{path}' has an unsupported const seed: {other}"
        )),
    }
}

fn bytes_from_json_array(value: &serde_json::Value) -> color_eyre::eyre::Result<Vec<u8>> {
    value
        .as_array()
        .wrap_err("expected a JSON array of bytes")?
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .wrap_err_with(|| format!("'{item}' is not a byte"))
        })
        .collect()
}

fn convert_type(raw: RawType) -> IdlType {
    match raw {
        RawType::Name(name) => match name.as_str() {
            "bool" => IdlType::Bool,
            "u8" => IdlType::U8,
            "i8" => IdlType::I8,
            "u16" => IdlType::U16,
            "i16" => IdlType::I16,
            "u32" => IdlType::U32,
            "i32" => IdlType::I32,
            "u64" => IdlType::U64,
            "i64" => IdlType::I64,
            "u128" => IdlType::U128,
            "i128" => IdlType::I128,
            "u256" => IdlType::U256,
            "i256" => IdlType::I256,
            "f32" => IdlType::F32,
            "f64" => IdlType::F64,
            "bytes" => IdlType::Bytes,
            "string" => IdlType::String,
            "pubkey" | "publicKey" => IdlType::Pubkey,
            other => IdlType::Unsupported(other.to_string()),
        },
        RawType::Vec { vec } => IdlType::Vec(Box::new(convert_type(*vec))),
        RawType::Option { option } => IdlType::Option(Box::new(convert_type(*option))),
        RawType::COption { coption } => IdlType::COption(Box::new(convert_type(*coption))),
        RawType::Array {
            array: (inner, len),
        } => IdlType::Array(Box::new(convert_type(*inner)), len),
        RawType::Defined { defined } => IdlType::Defined(match defined {
            RawDefined::Name(name) | RawDefined::Object { name } => name,
        }),
        RawType::Other(value) => IdlType::Unsupported(value.to_string()),
    }
}

fn convert_type_def(raw: RawTypeDefKind) -> color_eyre::eyre::Result<IdlTypeDef> {
    match raw.kind.as_str() {
        "struct" => Ok(IdlTypeDef::Struct(match raw.fields {
            Some(RawFields::Named(fields)) => convert_fields(fields),
            Some(RawFields::Tuple(types)) => types
                .into_iter()
                .enumerate()
                .map(|(index, ty)| IdlField {
                    name: index.to_string(),
                    ty: convert_type(ty),
                })
                .collect(),
            None => Vec::new(),
        })),
        "enum" => Ok(IdlTypeDef::Enum(
            raw.variants
                .unwrap_or_default()
                .into_iter()
                .map(|variant| IdlVariant {
                    name: variant.name,
                    fields: match variant.fields {
                        None => VariantFields::Unit,
                        Some(RawFields::Named(fields)) => {
                            VariantFields::Named(convert_fields(fields))
                        }
                        Some(RawFields::Tuple(types)) => {
                            VariantFields::Tuple(types.into_iter().map(convert_type).collect())
                        }
                    },
                })
                .collect(),
        )),
        other => Err(eyre!("Unsupported IDL type kind '{other}'")),
    }
}

fn convert_fields(fields: Vec<RawField>) -> Vec<IdlField> {
    fields
        .into_iter()
        .map(|field| IdlField {
            name: field.name,
            ty: convert_type(field.ty),
        })
        .collect()
}

/// Anchor's instruction discriminator: `sha256("global:<snake_name>")[..8]`.
pub fn anchor_discriminator(name: &str) -> [u8; 8] {
    let hash = Sha256::digest(format!("global:{}", to_snake_case(name)).as_bytes());
    hash[..8].try_into().expect("8 bytes")
}

fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------- address math

/// `Pubkey::create_with_seed`: `sha256(base || seed || owner)`.
pub fn create_with_seed(base: SolanaAddress, seed: &str, owner: SolanaAddress) -> SolanaAddress {
    let mut hasher = Sha256::new();
    hasher.update(base.0);
    hasher.update(seed.as_bytes());
    hasher.update(owner.0);
    SolanaAddress(hasher.finalize().into())
}

/// `Pubkey::find_program_address`: the first bump (from 255 down) whose
/// hash is not a valid ed25519 point.
pub fn find_program_address(seeds: &[&[u8]], program: SolanaAddress) -> (SolanaAddress, u8) {
    for bump in (0..=255u8).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program.0);
        hasher.update(b"ProgramDerivedAddress");
        let candidate: [u8; 32] = hasher.finalize().into();
        if !is_on_curve(&candidate) {
            return (SolanaAddress(candidate), bump);
        }
    }
    unreachable!("some bump always yields an off-curve address")
}

/// Whether 32 bytes decompress to a point on ed25519 (the check
/// `find_program_address` uses to reject candidates): with `y` the low 255
/// bits, `x^2 = (y^2 - 1) / (d y^2 + 1)` must have a square root mod
/// `2^255 - 19`.
#[allow(
    clippy::many_single_char_names,
    reason = "the curve equation's own letters"
)]
fn is_on_curve(bytes: &[u8; 32]) -> bool {
    // p = 2^255 - 19
    let p = (U256::from(1u8) << 255) - U256::from(19u8);
    let mut y_bytes = *bytes;
    y_bytes[31] &= 0x7f;
    let y = U256::from_le_bytes(y_bytes).reduce_mod(p);
    // d = -121665 / 121666
    let d = (p - U256::from(121_665u32))
        .mul_mod(U256::from(121_666u32).pow_mod(p - U256::from(2u8), p), p);
    let y2 = y.mul_mod(y, p);
    let u = y2.add_mod(p - U256::from(1u8), p);
    let v = d.mul_mod(y2, p).add_mod(U256::from(1u8), p);
    let v_inv = v.pow_mod(p - U256::from(2u8), p);
    let x2 = u.mul_mod(v_inv, p);
    // Euler's criterion: 0 and quadratic residues are squares.
    x2.is_zero() || x2.pow_mod((p - U256::from(1u8)) >> 1, p) == U256::from(1u8)
}

/// Where Anchor stores a program's IDL:
/// `create_with_seed(find_program_address([], program), "anchor:idl", program)`.
pub fn idl_account_address(program: SolanaAddress) -> SolanaAddress {
    let (base, _) = find_program_address(&[], program);
    create_with_seed(base, "anchor:idl", program)
}

// ----------------------------------------------------------- arg encoding

impl IdlType {
    fn is_integer(&self) -> bool {
        matches!(
            self,
            Self::U8
                | Self::I8
                | Self::U16
                | Self::I16
                | Self::U32
                | Self::I32
                | Self::U64
                | Self::I64
                | Self::U128
                | Self::I128
                | Self::U256
                | Self::I256
        )
    }
}

/// An instruction argument the user has entered, Borsh-encoded, with the
/// bytes a PDA seed referencing it would use.
#[derive(Debug, Clone)]
pub struct EncodedArg {
    pub name: String,
    pub bytes: Vec<u8>,
    pub seed_bytes: Vec<u8>,
}

/// Borsh-encodes one argument typed as text: integers in decimal, `true`/
/// `false`, strings verbatim, pubkeys in base58, bytes as `0x` hex or a JSON
/// array, `Vec`/arrays as JSON arrays, `Option` as `null` or the value,
/// structs as JSON objects, enums as the variant name (or `{"Variant":
/// {...}}` with fields).
pub fn encode_arg(
    ty: &IdlType,
    text: &str,
    types: &BTreeMap<String, IdlTypeDef>,
) -> color_eyre::eyre::Result<EncodedArg> {
    let value = text_to_value(text);
    let bytes = encode_value(ty, &value, types)?;
    // Seeds use the raw content of strings/bytes, not the Borsh length
    // prefix (`seeds = [name.as_bytes()]`).
    let seed_bytes = match ty {
        IdlType::String | IdlType::Bytes => bytes[4..].to_vec(),
        _ => bytes.clone(),
    };
    Ok(EncodedArg {
        name: String::new(),
        bytes,
        seed_bytes,
    })
}

/// The instruction data: discriminator followed by the Borsh-encoded args.
pub fn encode_instruction_data(discriminator: [u8; 8], args: &[EncodedArg]) -> Vec<u8> {
    let mut data = discriminator.to_vec();
    for arg in args {
        data.extend_from_slice(&arg.bytes);
    }
    data
}

/// Structured input (`[..]`, `{..}`, `"..."`, `null`) is JSON; anything
/// else is taken literally, so a base58 key of digits is not read as a
/// number.
fn text_to_value(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    let structured = trimmed == "null"
        || trimmed.starts_with('[')
        || trimmed.starts_with('{')
        || trimmed.starts_with('"');
    match serde_json::from_str(trimmed) {
        Ok(value) if structured => value,
        _ => serde_json::Value::String(trimmed.to_string()),
    }
}

#[allow(clippy::too_many_lines, reason = "one arm per IDL type")]
fn encode_value(
    ty: &IdlType,
    value: &serde_json::Value,
    types: &BTreeMap<String, IdlTypeDef>,
) -> color_eyre::eyre::Result<Vec<u8>> {
    use serde_json::Value;
    Ok(match ty {
        IdlType::Bool => {
            let flag = match value {
                Value::Bool(flag) => *flag,
                Value::String(text) => text
                    .parse::<bool>()
                    .map_err(|_| eyre!("'{text}' is not a bool (true/false)"))?,
                other => return Err(eyre!("'{other}' is not a bool")),
            };
            vec![u8::from(flag)]
        }
        ty if ty.is_integer() => encode_integer(ty, &scalar_text(value)?)?,
        IdlType::F32 => scalar_text(value)?
            .parse::<f32>()
            .map_err(|err| eyre!("not an f32: {err}"))?
            .to_le_bytes()
            .to_vec(),
        IdlType::F64 => scalar_text(value)?
            .parse::<f64>()
            .map_err(|err| eyre!("not an f64: {err}"))?
            .to_le_bytes()
            .to_vec(),
        IdlType::String => {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            borsh_bytes(text.as_bytes())
        }
        IdlType::Bytes => borsh_bytes(&bytes_value(value)?),
        IdlType::Pubkey => {
            let text = scalar_text(value)?;
            SolanaAddress::from_base58(&text)
                .map_err(|err| eyre!("'{text}' is not a base58 pubkey: {err}"))?
                .0
                .to_vec()
        }
        IdlType::Vec(inner) => {
            if **inner == IdlType::U8
                && let Ok(bytes) = bytes_value(value)
            {
                return Ok(borsh_bytes(&bytes));
            }
            let items = value
                .as_array()
                .wrap_err_with(|| format!("expected a JSON array for Vec<{inner}>"))?;
            let mut out = u32::try_from(items.len())?.to_le_bytes().to_vec();
            for item in items {
                out.extend(encode_value(inner, item, types)?);
            }
            out
        }
        IdlType::Array(inner, len) => {
            if **inner == IdlType::U8
                && let Ok(bytes) = bytes_value(value)
            {
                if bytes.len() != *len {
                    return Err(eyre!("expected {len} bytes, got {}", bytes.len()));
                }
                return Ok(bytes);
            }
            let items = value
                .as_array()
                .wrap_err_with(|| format!("expected a JSON array for [{inner}; {len}]"))?;
            if items.len() != *len {
                return Err(eyre!("expected {len} elements, got {}", items.len()));
            }
            let mut out = Vec::new();
            for item in items {
                out.extend(encode_value(inner, item, types)?);
            }
            out
        }
        IdlType::Option(inner) => {
            if value.is_null() {
                vec![0]
            } else {
                let mut out = vec![1];
                out.extend(encode_value(inner, value, types)?);
                out
            }
        }
        IdlType::COption(inner) => {
            if value.is_null() {
                0u32.to_le_bytes().to_vec()
            } else {
                let mut out = 1u32.to_le_bytes().to_vec();
                out.extend(encode_value(inner, value, types)?);
                out
            }
        }
        IdlType::Defined(name) => {
            let def = types
                .get(name)
                .wrap_err_with(|| format!("the IDL does not define type '{name}'"))?;
            encode_defined(name, def, value, types)?
        }
        IdlType::Unsupported(what) => {
            return Err(eyre!(
                "argument type '{what}' is not supported by this CLI - use the manual hex data \
                 entry instead"
            ));
        }
        IdlType::U8
        | IdlType::I8
        | IdlType::U16
        | IdlType::I16
        | IdlType::U32
        | IdlType::I32
        | IdlType::U64
        | IdlType::I64
        | IdlType::U128
        | IdlType::I128
        | IdlType::U256
        | IdlType::I256 => unreachable!("handled by the guards above"),
    })
}

fn encode_defined(
    name: &str,
    def: &IdlTypeDef,
    value: &serde_json::Value,
    types: &BTreeMap<String, IdlTypeDef>,
) -> color_eyre::eyre::Result<Vec<u8>> {
    match def {
        IdlTypeDef::Struct(fields) => {
            let object = value
                .as_object()
                .wrap_err_with(|| format!("expected a JSON object for struct {name}"))?;
            let mut out = Vec::new();
            for field in fields {
                let field_value = object.get(&field.name).wrap_err_with(|| {
                    format!("struct {name} needs field '{}' ({})", field.name, field.ty)
                })?;
                out.extend(encode_value(&field.ty, field_value, types)?);
            }
            Ok(out)
        }
        IdlTypeDef::Enum(variants) => {
            let (variant_name, payload) = match value {
                serde_json::Value::String(text) => (text.as_str(), None),
                serde_json::Value::Object(object) if object.len() == 1 => {
                    let (key, payload) = object.iter().next().expect("one entry");
                    (key.as_str(), Some(payload))
                }
                other => {
                    return Err(eyre!(
                        "enum {name} takes a variant name or {{\"Variant\": fields}}, not {other}"
                    ));
                }
            };
            let (index, variant) = variants
                .iter()
                .enumerate()
                .find(|(_, variant)| variant.name.eq_ignore_ascii_case(variant_name))
                .wrap_err_with(|| {
                    format!(
                        "enum {name} has no variant '{variant_name}' (variants: {})",
                        variants
                            .iter()
                            .map(|variant| variant.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            let mut out = vec![u8::try_from(index)?];
            match &variant.fields {
                VariantFields::Unit => {}
                VariantFields::Named(fields) => {
                    let payload = payload.wrap_err_with(|| {
                        format!("variant {variant_name} of {name} carries fields")
                    })?;
                    out.extend(encode_defined(
                        variant_name,
                        &IdlTypeDef::Struct(fields.clone()),
                        payload,
                        types,
                    )?);
                }
                VariantFields::Tuple(field_types) => {
                    let items = payload
                        .and_then(|payload| payload.as_array())
                        .wrap_err_with(|| {
                            format!("variant {variant_name} of {name} takes a JSON array of values")
                        })?;
                    if items.len() != field_types.len() {
                        return Err(eyre!(
                            "variant {variant_name} takes {} value(s), got {}",
                            field_types.len(),
                            items.len()
                        ));
                    }
                    for (ty, item) in field_types.iter().zip(items) {
                        out.extend(encode_value(ty, item, types)?);
                    }
                }
            }
            Ok(out)
        }
    }
}

fn scalar_text(value: &serde_json::Value) -> color_eyre::eyre::Result<String> {
    match value {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Number(number) => Ok(number.to_string()),
        serde_json::Value::Bool(flag) => Ok(flag.to_string()),
        other => Err(eyre!("expected a scalar value, got {other}")),
    }
}

/// Bytes written as `0x` hex, bare hex, or a JSON array of numbers.
fn bytes_value(value: &serde_json::Value) -> color_eyre::eyre::Result<Vec<u8>> {
    match value {
        serde_json::Value::Array(_) => bytes_from_json_array(value),
        serde_json::Value::String(text) => {
            let hex_str = text.trim();
            let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
            hex::decode(hex_str).wrap_err_with(|| format!("'{text}' is not hex"))
        }
        other => Err(eyre!("expected hex or a byte array, got {other}")),
    }
}

fn borsh_bytes(bytes: &[u8]) -> Vec<u8> {
    let len = u32::try_from(bytes.len()).expect("argument shorter than 4 GiB");
    let mut out = len.to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

fn encode_integer(ty: &IdlType, text: &str) -> color_eyre::eyre::Result<Vec<u8>> {
    fn parse<T: std::str::FromStr>(text: &str, ty: &IdlType) -> color_eyre::eyre::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        text.parse::<T>()
            .map_err(|err| eyre!("'{text}' is not a valid {ty}: {err}"))
    }
    let text = text.trim().replace('_', "");
    Ok(match ty {
        IdlType::U8 => parse::<u8>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::I8 => parse::<i8>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::U16 => parse::<u16>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::I16 => parse::<i16>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::U32 => parse::<u32>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::I32 => parse::<i32>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::U64 => parse::<u64>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::I64 => parse::<i64>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::U128 => parse::<u128>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::I128 => parse::<i128>(&text, ty)?.to_le_bytes().to_vec(),
        IdlType::U256 => U256::from_str_radix(&text, 10)
            .map_err(|err| eyre!("'{text}' is not a valid u256: {err}"))?
            .to_le_bytes::<32>()
            .to_vec(),
        IdlType::I256 => alloy_primitives::I256::from_dec_str(&text)
            .map_err(|err| eyre!("'{text}' is not a valid i256: {err}"))?
            .to_le_bytes::<32>()
            .to_vec(),
        other => return Err(eyre!("{other} is not an integer type")),
    })
}

// ------------------------------------------------------ account resolution

/// What the user (or a heuristic) supplied for an account the IDL does not
/// pin down.
#[derive(Debug, Clone, Copy)]
pub enum ProvidedAccount {
    /// The derived address (fee payer / only possible signer).
    Payer,
    Address(SolanaAddress),
}

/// How an account's address was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSource {
    /// Pinned in the IDL.
    Fixed,
    /// A PDA derived from constants, other accounts, or arguments.
    Derived,
    /// The derived address, because the account must sign.
    Signer,
    /// Entered by the user.
    Provided,
}

impl std::fmt::Display for AccountSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fixed => "fixed (IDL)",
            Self::Derived => "derived (PDA)",
            Self::Signer => "you (signer)",
            Self::Provided => "you",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedAccount {
    pub path: String,
    pub key: InstructionAccountKey,
    pub writable: bool,
    pub signer: bool,
    pub source: AccountSource,
}

/// An account the resolver could not determine, with everything a prompt
/// needs.
#[derive(Debug, Clone)]
pub struct AccountNeed {
    pub path: String,
    pub docs: Vec<String>,
    pub writable: bool,
    pub optional: bool,
    /// A well-known default for this name, if any.
    pub default: Option<SolanaAddress>,
    /// Alternatives when the name suggests a small set (token programs).
    pub choices: Vec<(&'static str, SolanaAddress)>,
    /// Why a PDA could not be derived (yet).
    pub blocked_on: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AccountSlot {
    Resolved(ResolvedAccount),
    Unresolved(AccountNeed),
}

#[derive(Debug, Clone)]
pub struct Resolution {
    /// One slot per IDL account, in instruction order.
    pub slots: Vec<AccountSlot>,
}

impl Resolution {
    pub fn unresolved(&self) -> impl Iterator<Item = &AccountNeed> {
        self.slots.iter().filter_map(|slot| match slot {
            AccountSlot::Unresolved(need) => Some(need),
            AccountSlot::Resolved(_) => None,
        })
    }

    /// The accounts in the `"<pubkey>[:flags]"` grammar of the
    /// non-interactive command. Fails while anything is unresolved.
    pub fn account_specs(&self) -> color_eyre::eyre::Result<Vec<String>> {
        self.slots
            .iter()
            .map(|slot| match slot {
                AccountSlot::Resolved(account) => {
                    let key = match &account.key {
                        InstructionAccountKey::Payer => "payer".to_string(),
                        InstructionAccountKey::Address(address) => address.to_base58(),
                    };
                    let mut flags = String::new();
                    if account.signer {
                        flags.push('s');
                    }
                    if account.writable {
                        flags.push('w');
                    }
                    Ok(if flags.is_empty() {
                        key
                    } else {
                        format!("{key}:{flags}")
                    })
                }
                AccountSlot::Unresolved(need) => {
                    Err(eyre!("account '{}' is still unresolved", need.path))
                }
            })
            .collect()
    }
}

/// Resolves the accounts of `instruction` as far as the IDL allows: fixed
/// addresses, the signer (always the derived address), user-provided
/// accounts, and PDAs whose seeds and program are all known - iterating so
/// a PDA seeded with another account resolves once that account does.
#[allow(
    clippy::too_many_lines,
    reason = "the fixed-point loop and slot assembly belong together"
)]
pub fn resolve_accounts(
    instruction: &IdlInstruction,
    program_id: SolanaAddress,
    args: &[EncodedArg],
    provided: &BTreeMap<String, ProvidedAccount>,
) -> color_eyre::eyre::Result<Resolution> {
    let signers: Vec<&IdlAccount> = instruction
        .accounts
        .iter()
        .filter(|account| account.signer)
        .collect();
    if signers.len() > 1 {
        return Err(eyre!(
            "Instruction '{}' needs {} signers ({}), but the MPC can only sign for the derived \
             address - it cannot be sent through omni.",
            instruction.name,
            signers.len(),
            signers
                .iter()
                .map(|account| account.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // Known addresses by path (and leaf name), grown as passes resolve more.
    let mut known: BTreeMap<String, (InstructionAccountKey, AccountSource)> = BTreeMap::new();
    for account in &instruction.accounts {
        if let Some(address) = account.address {
            known.insert(
                account.path.clone(),
                (
                    InstructionAccountKey::Address(address),
                    AccountSource::Fixed,
                ),
            );
        } else if account.signer {
            known.insert(
                account.path.clone(),
                (InstructionAccountKey::Payer, AccountSource::Signer),
            );
        }
        if let Some(given) = provided.get(&account.path) {
            let key = match given {
                ProvidedAccount::Payer => InstructionAccountKey::Payer,
                ProvidedAccount::Address(address) => InstructionAccountKey::Address(*address),
            };
            known.insert(account.path.clone(), (key, AccountSource::Provided));
        }
    }

    let mut blocked: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let mut progress = false;
        for account in &instruction.accounts {
            if known.contains_key(&account.path) {
                continue;
            }
            let Some(pda) = &account.pda else {
                continue;
            };
            match derive_pda(instruction, pda, program_id, args, &known) {
                Ok(address) => {
                    known.insert(
                        account.path.clone(),
                        (
                            InstructionAccountKey::Address(address),
                            AccountSource::Derived,
                        ),
                    );
                    blocked.remove(&account.path);
                    progress = true;
                }
                Err(reason) => {
                    blocked.insert(account.path.clone(), reason);
                }
            }
        }
        if !progress {
            break;
        }
    }

    let slots = instruction
        .accounts
        .iter()
        .map(|account| {
            if let Some((key, source)) = known.get(&account.path) {
                AccountSlot::Resolved(ResolvedAccount {
                    path: account.path.clone(),
                    key: key.clone(),
                    writable: account.writable,
                    signer: account.signer,
                    source: *source,
                })
            } else {
                let (mut default, choices) = well_known_default(&account.name, program_id);
                if default.is_none() {
                    default = metaplex_metadata_default(&account.name, &known);
                }
                AccountSlot::Unresolved(AccountNeed {
                    path: account.path.clone(),
                    docs: account.docs.clone(),
                    writable: account.writable,
                    optional: account.optional,
                    default,
                    choices,
                    blocked_on: blocked.get(&account.path).cloned(),
                })
            }
        })
        .collect();
    Ok(Resolution { slots })
}

/// Derives one PDA, or explains what is still missing.
fn derive_pda(
    instruction: &IdlInstruction,
    pda: &IdlPda,
    program_id: SolanaAddress,
    args: &[EncodedArg],
    known: &BTreeMap<String, (InstructionAccountKey, AccountSource)>,
) -> Result<SolanaAddress, String> {
    let lookup_account = |reference: &str,
                          account_type: Option<&str>|
     -> Result<SolanaAddress, String> {
        let account = instruction.account(reference).ok_or_else(|| {
            if account_type.is_some() || reference.contains('.') {
                // `seeds = [user.owner.as_ref()]`: a field read from another
                // account's data, which needs that account fetched and
                // deserialized - not something this CLI does.
                format!(
                    "seeded with '{reference}', a field inside another account's data - paste the \
                     address"
                )
            } else {
                format!("seed refers to unknown account '{reference}'")
            }
        })?;
        match known.get(&account.path) {
            Some((InstructionAccountKey::Address(address), _)) => Ok(*address),
            Some((InstructionAccountKey::Payer, _)) => Err(format!(
                "seeded with the signer '{}' - the derived address is only known at the network \
                 step, so paste it here (see `omni account show`)",
                account.path
            )),
            None => Err(format!("waits for account '{}'", account.path)),
        }
    };
    let mut seeds: Vec<Vec<u8>> = Vec::with_capacity(pda.seeds.len());
    for seed in &pda.seeds {
        seeds.push(match seed {
            IdlSeed::Const(bytes) => bytes.clone(),
            IdlSeed::Account { path, account_type } => {
                lookup_account(path, account_type.as_deref())?.0.to_vec()
            }
            IdlSeed::Arg(name) => args
                .iter()
                .find(|arg| &arg.name == name)
                .map(|arg| arg.seed_bytes.clone())
                .ok_or_else(|| format!("seeded with argument '{name}'"))?,
            IdlSeed::Unsupported(what) => return Err(format!("seeded with {what}")),
        });
    }
    let program = match &pda.program {
        None => program_id,
        Some(IdlSeed::Const(bytes)) => SolanaAddress(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| "PDA program constant is not 32 bytes".to_string())?,
        ),
        Some(IdlSeed::Account { path, account_type }) => {
            lookup_account(path, account_type.as_deref())?
        }
        Some(IdlSeed::Arg(name)) => {
            let bytes = args
                .iter()
                .find(|arg| &arg.name == name)
                .map(|arg| arg.seed_bytes.clone())
                .ok_or_else(|| format!("PDA program comes from argument '{name}'"))?;
            SolanaAddress(
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| format!("argument '{name}' is not a pubkey"))?,
            )
        }
        Some(IdlSeed::Unsupported(what)) => return Err(format!("PDA program is {what}")),
    };
    let seed_refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    Ok(find_program_address(&seed_refs, program).0)
}

const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

fn address(base58: &str) -> SolanaAddress {
    SolanaAddress::from_base58(base58).expect("static address is valid")
}

const METAPLEX_TOKEN_METADATA_PROGRAM: &str = "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s";

/// The Metaplex metadata PDA of an already-known `mint` account, offered as
/// the default for an account named like the token metadata. IDLs declare
/// it as a plain (often optional) account because it lives under another
/// program, so seeds alone never derive it.
fn metaplex_metadata_default(
    name: &str,
    known: &BTreeMap<String, (InstructionAccountKey, AccountSource)>,
) -> Option<SolanaAddress> {
    let lower = name.to_ascii_lowercase();
    if !(lower == "metadata" || lower == "metadata_account" || lower == "token_metadata") {
        return None;
    }
    let mint = known.iter().find_map(|(path, (key, _))| {
        let leaf = path.rsplit('.').next().unwrap_or(path);
        match key {
            InstructionAccountKey::Address(address) if leaf == "mint" => Some(*address),
            _ => None,
        }
    })?;
    let metaplex = address(METAPLEX_TOKEN_METADATA_PROGRAM);
    Some(find_program_address(&[b"metadata", &metaplex.0, &mint.0], metaplex).0)
}

/// Conventional defaults for accounts Anchor programs name predictably.
/// Defaults only - the user confirms each.
fn well_known_default(
    name: &str,
    program_id: SolanaAddress,
) -> (Option<SolanaAddress>, Vec<(&'static str, SolanaAddress)>) {
    let lower = name.to_ascii_lowercase();
    if lower.contains("associated_token_program") {
        return (
            Some(address("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")),
            Vec::new(),
        );
    }
    if lower.contains("token_program") {
        return (
            Some(address(TOKEN_PROGRAM)),
            vec![
                ("SPL Token", address(TOKEN_PROGRAM)),
                ("Token-2022", address(TOKEN_2022_PROGRAM)),
            ],
        );
    }
    if lower.contains("system_program") {
        return (Some(SYSTEM_PROGRAM), Vec::new());
    }
    if lower.contains("metadata_program") || lower.contains("mpl_token_metadata") {
        return (Some(address(METAPLEX_TOKEN_METADATA_PROGRAM)), Vec::new());
    }
    if lower == "rent" || lower.ends_with("rent_sysvar") || lower == "sysvar_rent" {
        return (
            Some(address("SysvarRent111111111111111111111111111111111")),
            Vec::new(),
        );
    }
    if lower == "clock" || lower.ends_with("clock_sysvar") || lower == "sysvar_clock" {
        return (
            Some(address("SysvarC1ock11111111111111111111111111111111")),
            Vec::new(),
        );
    }
    if lower == "instructions"
        || lower.contains("sysvar_instructions")
        || lower == "instructions_sysvar"
    {
        return (
            Some(address("Sysvar1nstructions1111111111111111111111111")),
            Vec::new(),
        );
    }
    if lower == "event_authority" {
        // Anchor's event-CPI convention.
        return (
            Some(find_program_address(&[b"__event_authority"], program_id).0),
            Vec::new(),
        );
    }
    if lower == "program" {
        return (Some(program_id), Vec::new());
    }
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_default_is_the_metaplex_pda_of_the_known_mint() {
        let mut known = BTreeMap::new();
        known.insert(
            "mint".to_string(),
            (
                InstructionAccountKey::Address(address(
                    "ARS1ayzvH2xtaRo1BLW95FsthhqN3NAA1e9Pcm186Fmw",
                )),
                AccountSource::Provided,
            ),
        );
        assert_eq!(
            metaplex_metadata_default("metadata", &known).map(|a| a.to_base58()),
            Some("ADd4gv9ds4XZrL5q4sCFX9qWLuBmrfY6BSatH9RV2DG".to_string())
        );
        assert!(metaplex_metadata_default("vault", &known).is_none());
        assert!(metaplex_metadata_default("metadata", &BTreeMap::new()).is_none());
    }

    fn addr(base58: &str) -> SolanaAddress {
        SolanaAddress::from_base58(base58).unwrap()
    }

    const WORMHOLE: &str = "worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth";
    const BRIDGE_PROGRAM: &str = "dahPEoZGXfyV58JqqH85okdHmpN8U2q8owgPUXSCPxe";
    const MINT: &str = "ARS1ayzvH2xtaRo1BLW95FsthhqN3NAA1e9Pcm186Fmw";
    const SHIM: &str = "EtZMZM22ViKMo4r5y4Anovs3wKQ2owUmDpjygnMMcdEX";

    #[test]
    fn derives_known_mainnet_pdas() {
        assert_eq!(
            find_program_address(&[b"Bridge"], addr(WORMHOLE)).0,
            addr("2yVjuQwpsvdsrywzsJJVs9Ueh4zayyo5DYJbBNc3DDpn")
        );
        assert_eq!(
            find_program_address(&[b"fee_collector"], addr(WORMHOLE)).0,
            addr("9bFNrXNb2WTx8fMHXCheaZqkLZ3YCCaiqTftHxeintHy")
        );
        assert_eq!(
            find_program_address(&[b"config"], addr(BRIDGE_PROGRAM)).0,
            addr("2iANpDh96GgithLPTVMZjZFtnbrYBUCLqnEvaytbwTQq")
        );
        assert_eq!(
            find_program_address(&[b"authority"], addr(BRIDGE_PROGRAM)).0,
            addr("FvULawNPGBbuwYus74ECaQoV1oH9Tk6XPN7VPN51NYds")
        );
        assert_eq!(
            find_program_address(&[b"vault", &addr(MINT).0], addr(BRIDGE_PROGRAM)).0,
            addr("GRKJxYRWifS3Hde6TmhgiuwtzqXEamPSKWLHDV12TAJ4")
        );
        let metaplex = addr("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
        assert_eq!(
            find_program_address(&[b"metadata", &metaplex.0, &addr(MINT).0], metaplex).0,
            addr("ADd4gv9ds4XZrL5q4sCFX9qWLuBmrfY6BSatH9RV2DG")
        );
        assert_eq!(
            find_program_address(&[b"__event_authority"], addr(SHIM)).0,
            addr("HQS31aApX3DDkuXgSpV9XyDUNtFgQ31pUn5BNWHG2PSp")
        );
    }

    #[test]
    fn on_curve_check_accepts_real_keys_and_rejects_pdas() {
        // A real ed25519 public key (the Wormhole program id) is on the curve.
        assert!(is_on_curve(&addr(WORMHOLE).0));
        assert!(is_on_curve(&addr(MINT).0));
        // PDAs are, by construction, not.
        assert!(!is_on_curve(
            &addr("2yVjuQwpsvdsrywzsJJVs9Ueh4zayyo5DYJbBNc3DDpn").0
        ));
    }

    #[test]
    fn discriminator_matches_anchor() {
        assert_eq!(
            hex::encode(anchor_discriminator("log_metadata")),
            "a89dc34f60d2d002"
        );
        // Legacy IDLs use camelCase names; the hash uses snake_case.
        assert_eq!(
            anchor_discriminator("logMetadata"),
            anchor_discriminator("log_metadata")
        );
        assert_eq!(
            create_with_seed(
                addr("FvULawNPGBbuwYus74ECaQoV1oH9Tk6XPN7VPN51NYds"),
                "omni-nonce",
                SYSTEM_PROGRAM
            ),
            crate::chains::svm::nonce_account_address(addr(
                "FvULawNPGBbuwYus74ECaQoV1oH9Tk6XPN7VPN51NYds"
            ))
        );
    }

    /// A new-format IDL modelled on the omni-bridge `log_metadata`
    /// instruction: a composite `common` group, PDAs seeded with constants,
    /// other accounts, and a program taken from an account.
    fn bridge_idl() -> Idl {
        let json = serde_json::json!({
            "address": BRIDGE_PROGRAM,
            "metadata": { "name": "bridge_token_factory", "version": "0.1.0", "spec": "0.1.0" },
            "instructions": [{
                "name": "log_metadata",
                "docs": ["Publish a token's metadata to NEAR"],
                "discriminator": [168, 157, 195, 79, 96, 210, 208, 2],
                "accounts": [
                    { "name": "authority", "pda": { "seeds": [{ "kind": "const", "value": [97,117,116,104,111,114,105,116,121] }] } },
                    { "name": "mint" },
                    { "name": "metadata", "optional": true, "docs": ["may be uninitialized"] },
                    { "name": "vault", "writable": true, "pda": { "seeds": [
                        { "kind": "const", "value": [118,97,117,108,116] },
                        { "kind": "account", "path": "mint" }
                    ] } },
                    { "name": "common", "accounts": [
                        { "name": "config", "pda": { "seeds": [{ "kind": "const", "value": [99,111,110,102,105,103] }] } },
                        { "name": "bridge", "writable": true, "pda": {
                            "seeds": [{ "kind": "const", "value": [66,114,105,100,103,101] }],
                            "program": { "kind": "account", "path": "wormhole_program" }
                        } },
                        { "name": "sequence", "writable": true, "pda": {
                            "seeds": [
                                { "kind": "const", "value": [83,101,113,117,101,110,99,101] },
                                { "kind": "account", "path": "common.config" }
                            ],
                            "program": { "kind": "account", "path": "wormhole_program" }
                        } },
                        { "name": "payer", "writable": true, "signer": true },
                        { "name": "clock", "address": "SysvarC1ock11111111111111111111111111111111" },
                        { "name": "wormhole_program" },
                        { "name": "system_program", "address": "11111111111111111111111111111111" }
                    ] },
                    { "name": "token_program" }
                ],
                "args": []
            }, {
                "name": "init_transfer",
                "discriminator": [1,2,3,4,5,6,7,8],
                "accounts": [
                    { "name": "used_nonces", "writable": true, "pda": { "seeds": [
                        { "kind": "const", "value": [110,111,110,99,101] },
                        { "kind": "arg", "path": "payload.nonce" }
                    ] } }
                ],
                "args": [
                    { "name": "payload", "type": { "defined": { "name": "InitTransferPayload" } } }
                ]
            }],
            "types": [{
                "name": "InitTransferPayload",
                "type": { "kind": "struct", "fields": [
                    { "name": "amount", "type": "u128" },
                    { "name": "recipient", "type": "string" },
                    { "name": "fee", "type": "u128" },
                    { "name": "native_fee", "type": "u64" },
                    { "name": "message", "type": { "option": "string" } }
                ] }
            }, {
                "name": "Finality",
                "type": { "kind": "enum", "variants": [{ "name": "Confirmed" }, { "name": "Finalized" }] }
            }]
        });
        parse_idl_json(&json.to_string()).unwrap()
    }

    #[test]
    fn flattens_groups_and_resolves_log_metadata_incrementally() {
        let idl = bridge_idl();
        let ix = &idl.instructions[0];
        assert_eq!(ix.signature(), "log_metadata()");
        let paths: Vec<&str> = ix.accounts.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "authority",
                "mint",
                "metadata",
                "vault",
                "common.config",
                "common.bridge",
                "common.sequence",
                "common.payer",
                "common.clock",
                "common.wormhole_program",
                "common.system_program",
                "token_program"
            ]
        );

        let program = idl.address.unwrap();
        let mut provided = BTreeMap::new();
        let first = resolve_accounts(ix, program, &[], &provided).unwrap();
        let unresolved: Vec<&str> = first.unresolved().map(|n| n.path.as_str()).collect();
        // Constants derive at once; `vault` waits for the mint, the Wormhole
        // PDAs for their program.
        assert_eq!(
            unresolved,
            [
                "mint",
                "metadata",
                "vault",
                "common.bridge",
                "common.sequence",
                "common.wormhole_program",
                "token_program"
            ]
        );
        let vault_need = first.unresolved().find(|n| n.path == "vault").unwrap();
        assert_eq!(
            vault_need.blocked_on.as_deref(),
            Some("waits for account 'mint'")
        );
        let token_need = first
            .unresolved()
            .find(|n| n.path == "token_program")
            .unwrap();
        assert_eq!(token_need.choices.len(), 2);
        assert_eq!(token_need.default, Some(addr(TOKEN_PROGRAM)));

        provided.insert("mint".into(), ProvidedAccount::Address(addr(MINT)));
        provided.insert(
            "common.wormhole_program".into(),
            ProvidedAccount::Address(addr(WORMHOLE)),
        );
        provided.insert(
            "token_program".into(),
            ProvidedAccount::Address(addr(TOKEN_PROGRAM)),
        );
        provided.insert(
            "metadata".into(),
            ProvidedAccount::Address(addr("ADd4gv9ds4XZrL5q4sCFX9qWLuBmrfY6BSatH9RV2DG")),
        );
        let done = resolve_accounts(ix, program, &[], &provided).unwrap();
        assert!(done.unresolved().next().is_none());
        assert_eq!(
            done.account_specs().unwrap(),
            [
                "FvULawNPGBbuwYus74ECaQoV1oH9Tk6XPN7VPN51NYds",
                MINT,
                "ADd4gv9ds4XZrL5q4sCFX9qWLuBmrfY6BSatH9RV2DG",
                "GRKJxYRWifS3Hde6TmhgiuwtzqXEamPSKWLHDV12TAJ4:w",
                "2iANpDh96GgithLPTVMZjZFtnbrYBUCLqnEvaytbwTQq",
                "2yVjuQwpsvdsrywzsJJVs9Ueh4zayyo5DYJbBNc3DDpn:w",
                "79nePUUxtfJSdrkbririgL5YMaoEh5g4HNQaar2EuJ7:w",
                "payer:sw",
                "SysvarC1ock11111111111111111111111111111111",
                WORMHOLE,
                "11111111111111111111111111111111",
                TOKEN_PROGRAM,
            ]
        );
        let sources: Vec<AccountSource> = done
            .slots
            .iter()
            .map(|slot| match slot {
                AccountSlot::Resolved(a) => a.source,
                AccountSlot::Unresolved(_) => unreachable!(),
            })
            .collect();
        assert_eq!(sources[0], AccountSource::Derived);
        assert_eq!(sources[7], AccountSource::Signer);
        assert_eq!(sources[8], AccountSource::Fixed);
        assert_eq!(sources[9], AccountSource::Provided);
        assert_eq!(
            encode_instruction_data(ix.discriminator, &[]),
            hex::decode("a89dc34f60d2d002").unwrap()
        );
    }

    #[test]
    fn rejects_instructions_with_two_signers() {
        let json = serde_json::json!({
            "address": BRIDGE_PROGRAM,
            "instructions": [{
                "name": "two_signers",
                "accounts": [
                    { "name": "a", "signer": true },
                    { "name": "b", "signer": true }
                ]
            }]
        });
        let idl = parse_idl_json(&json.to_string()).unwrap();
        let err = resolve_accounts(
            &idl.instructions[0],
            idl.address.unwrap(),
            &[],
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("2 signers"), "{err}");
    }

    #[test]
    fn encodes_borsh_args_and_struct_payloads() {
        let idl = bridge_idl();
        let types = &idl.types;
        assert_eq!(
            encode_arg(&IdlType::U64, "1_200", types).unwrap().bytes,
            1200u64.to_le_bytes()
        );
        assert_eq!(
            encode_arg(&IdlType::Bool, "true", types).unwrap().bytes,
            [1]
        );
        let s = encode_arg(&IdlType::String, "hi", types).unwrap();
        assert_eq!(s.bytes, [2, 0, 0, 0, b'h', b'i']);
        assert_eq!(s.seed_bytes, b"hi");
        assert_eq!(
            encode_arg(&IdlType::Pubkey, "11111111111111111111111111111111", types)
                .unwrap()
                .bytes,
            [0u8; 32]
        );
        assert_eq!(
            encode_arg(&IdlType::Bytes, "0xdead", types).unwrap().bytes,
            [2, 0, 0, 0, 0xde, 0xad]
        );
        assert_eq!(
            encode_arg(&IdlType::Vec(Box::new(IdlType::U16)), "[1, 2]", types)
                .unwrap()
                .bytes,
            [2, 0, 0, 0, 1, 0, 2, 0]
        );
        assert_eq!(
            encode_arg(&IdlType::Option(Box::new(IdlType::U8)), "null", types)
                .unwrap()
                .bytes,
            [0]
        );
        assert_eq!(
            encode_arg(&IdlType::Option(Box::new(IdlType::U8)), "7", types)
                .unwrap()
                .bytes,
            [1, 7]
        );
        assert_eq!(
            encode_arg(&IdlType::Array(Box::new(IdlType::U8), 2), "0x0102", types)
                .unwrap()
                .bytes,
            [1, 2]
        );
        let payload = encode_arg(
            &IdlType::Defined("InitTransferPayload".into()),
            r#"{"amount": "5", "recipient": "alice.near", "fee": 0, "native_fee": 1, "message": null}"#,
            types,
        )
        .unwrap();
        let mut expected = 5u128.to_le_bytes().to_vec();
        expected.extend([10, 0, 0, 0]);
        expected.extend(b"alice.near");
        expected.extend(0u128.to_le_bytes());
        expected.extend(1u64.to_le_bytes());
        expected.push(0);
        assert_eq!(payload.bytes, expected);
        assert_eq!(
            encode_arg(&IdlType::Defined("Finality".into()), "Finalized", types)
                .unwrap()
                .bytes,
            [1]
        );
        assert!(encode_arg(&IdlType::U8, "300", types).is_err());
        assert!(encode_arg(&IdlType::Defined("Finality".into()), "Nope", types).is_err());
    }

    #[test]
    fn arg_seeded_pdas_use_raw_bytes() {
        let idl = bridge_idl();
        let ix = &idl.instructions[1];
        let mut arg = encode_arg(&IdlType::U64, "42", &idl.types).unwrap();
        arg.name = "payload.nonce".into();
        let resolution =
            resolve_accounts(ix, idl.address.unwrap(), &[arg], &BTreeMap::new()).unwrap();
        let expected =
            find_program_address(&[b"nonce", &42u64.to_le_bytes()], idl.address.unwrap()).0;
        assert_eq!(
            resolution.account_specs().unwrap(),
            [format!("{}:w", expected.to_base58())]
        );
        // Without the argument the PDA reports what it waits for.
        let pending = resolve_accounts(ix, idl.address.unwrap(), &[], &BTreeMap::new()).unwrap();
        assert_eq!(
            pending.unresolved().next().unwrap().blocked_on.as_deref(),
            Some("seeded with argument 'payload.nonce'")
        );
    }

    #[test]
    fn reads_legacy_idls() {
        let json = serde_json::json!({
            "version": "0.1.0",
            "name": "legacy_program",
            "instructions": [{
                "name": "logMetadata",
                "accounts": [
                    { "name": "config", "isMut": false, "isSigner": false, "pda": {
                        "seeds": [{ "kind": "const", "type": "string", "value": "config" }]
                    } },
                    { "name": "payer", "isMut": true, "isSigner": true },
                    { "name": "extra", "isMut": false, "isSigner": false, "isOptional": true }
                ],
                "args": [{ "name": "owner", "type": "publicKey" }, { "name": "kind", "type": { "defined": "Kind" } }]
            }],
            "types": [{ "name": "Kind", "type": { "kind": "enum", "variants": [{ "name": "A" }, { "name": "B", "fields": ["u8"] }] } }],
            "metadata": { "address": BRIDGE_PROGRAM }
        });
        let idl = parse_idl_json(&json.to_string()).unwrap();
        assert_eq!(idl.name, "legacy_program");
        let ix = &idl.instructions[0];
        assert_eq!(ix.discriminator, anchor_discriminator("log_metadata"));
        assert_eq!(ix.args[0].ty, IdlType::Pubkey);
        assert!(ix.accounts[2].optional);
        let resolution = resolve_accounts(ix, idl.address.unwrap(), &[], &BTreeMap::new()).unwrap();
        assert_eq!(
            resolution
                .unresolved()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>(),
            ["extra"]
        );
        assert_eq!(
            encode_arg(
                &IdlType::Defined("Kind".into()),
                r#"{"B": [9]}"#,
                &idl.types
            )
            .unwrap()
            .bytes,
            [1, 9]
        );
    }

    #[test]
    fn parses_the_on_chain_idl_account_layout() {
        use std::io::Write;
        let json = serde_json::json!({
            "address": BRIDGE_PROGRAM,
            "instructions": [{ "name": "pause", "discriminator": [1,2,3,4,5,6,7,8], "accounts": [], "args": [] }]
        })
        .to_string();
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(json.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut data = vec![0u8; 8];
        data.extend([7u8; 32]);
        data.extend(u32::try_from(compressed.len()).unwrap().to_le_bytes());
        data.extend(&compressed);
        data.extend([0u8; 100]); // trailing capacity, as on chain
        let idl = parse_idl_account(&data).unwrap();
        assert_eq!(idl.instructions[0].name, "pause");
        assert!(parse_idl_account(&data[..20]).is_err());
    }

    #[test]
    fn idl_address_is_deterministic() {
        // Same derivation `anchor idl fetch` uses; stable across runs.
        let program = addr(BRIDGE_PROGRAM);
        assert_eq!(idl_account_address(program), idl_account_address(program));
        assert_ne!(idl_account_address(program), program);
    }
}
