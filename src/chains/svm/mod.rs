//! SVM chain family (Solana, Fogo, ...): ed25519 payloads built with
//! omni-transaction-rs Solana builders. The MPC signs the full serialized
//! message (never a hash) via its ed25519 key domain.
//!
//! The DAO route uses a durable nonce account (a recent blockhash dies in
//! ~60-90 s, long before a DAO can vote): each derived address gets one
//! deterministic nonce account, created once with the `setup-nonce` action
//! and consumed by prepending `AdvanceNonceAccount` to governance
//! transactions.

pub mod idl;
pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use color_eyre::owo_colors::OwoColorize;
use omni_transaction::TxBuilder;
use omni_transaction::solana::types::{
    AccountMeta, Blockhash, Instruction, SolanaAddress, SolanaSignature,
};
use omni_transaction::solana::{SolanaTransaction, SolanaTransactionBuilder, utils};
use sha2::{Digest, Sha256};

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "svm";

/// Cost of one signature at the base fee rate.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

/// Seed for the deterministic durable nonce account of a derived address
/// (`create_with_seed(base = derived, seed, owner = system program)`).
const NONCE_SEED: &str = "omni-nonce";

/// Byte size of an initialized nonce account's state.
const NONCE_ACCOUNT_SIZE: u64 = 80;

pub(crate) const SYSTEM_PROGRAM: SolanaAddress = SolanaAddress([0u8; 32]);

fn recent_blockhashes_sysvar() -> SolanaAddress {
    SolanaAddress::from_base58("SysvarRecentB1ockHashes11111111111111111111")
        .expect("static sysvar address is valid")
}

fn rent_sysvar() -> SolanaAddress {
    SolanaAddress::from_base58("SysvarRent111111111111111111111111111111111")
        .expect("static sysvar address is valid")
}

#[derive(Debug, Clone)]
pub enum SvmActionSpec {
    /// Native SOL transfer via the system program.
    Transfer { to: SolanaAddress, lamports: u64 },
    /// Create + initialize a durable nonce account - the one-time
    /// prerequisite for the DAO route on SVM chains. `authority` is the
    /// address that will use it: `None` for the derived address itself, or
    /// another derived address (a DAO's) that cannot create its own because
    /// creation needs a signature within a recent-blockhash window.
    SetupNonce { authority: Option<SolanaAddress> },
    /// One arbitrary program instruction (program call) signed by the
    /// derived address as fee payer.
    Instruction {
        program_id: SolanaAddress,
        accounts: Vec<InstructionAccount>,
        data: Vec<u8>,
        summary: String,
    },
}

/// An account of a custom instruction. The derived address is the only key
/// the MPC can sign for, so it is the only account that may be a signer;
/// `Payer` names it before the derivation is known.
#[derive(Debug, Clone)]
pub struct InstructionAccount {
    pub key: InstructionAccountKey,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(Debug, Clone)]
pub enum InstructionAccountKey {
    Payer,
    Address(SolanaAddress),
}

pub struct SvmAdapter {
    pub spec: SvmActionSpec,
    /// Externally created nonce account (authority must be the derived
    /// address). When absent, the deterministic seed account is used.
    pub nonce_account_override: Option<SolanaAddress>,
}

/// The deterministic durable nonce account of a derived address:
/// `sha256(base || "omni-nonce" || system_program)`, the
/// `create_with_seed` address rule.
pub fn nonce_account_address(base: SolanaAddress) -> SolanaAddress {
    create_with_seed(base, NONCE_SEED)
}

/// The `create_with_seed` address rule: `sha256(base || seed || owner)`
/// with the system program as owner.
fn create_with_seed(base: SolanaAddress, seed: &str) -> SolanaAddress {
    let mut hasher = Sha256::new();
    hasher.update(base.0);
    hasher.update(seed.as_bytes());
    hasher.update(SYSTEM_PROGRAM.0);
    SolanaAddress(hasher.finalize().into())
}

/// The nonce account `payer` creates for `authority`: the payer's own
/// deterministic one when they coincide, otherwise a seed account of the
/// payer keyed by the authority (seeds are capped at 32 bytes, so the
/// authority's base58 prefix is used). Deterministic, so re-running
/// `setup-nonce` finds the existing account instead of making another.
pub fn nonce_account_for(
    payer: SolanaAddress,
    authority: SolanaAddress,
) -> (SolanaAddress, String) {
    if authority == payer {
        return (nonce_account_address(payer), NONCE_SEED.to_string());
    }
    let authority_base58 = authority.to_base58();
    let seed = format!("nonce:{}", &authority_base58[..26]);
    (create_with_seed(payer, &seed), seed)
}

/// How DAO-route commands reference a nonce account: the deterministic one
/// of the derived address is found automatically; one created by someone
/// else must be passed explicitly.
fn nonce_account_flag_hint(
    authority: SolanaAddress,
    payer: SolanaAddress,
    nonce_base58: &str,
) -> String {
    if authority == payer {
        String::new()
    } else {
        format!(
            "\nDAO-route commands for that derived address must pass it explicitly: \
             --nonce-account {nonce_base58}"
        )
    }
}

/// `SystemInstruction::AdvanceNonceAccount` (bincode enum index 4).
fn advance_nonce_account(nonce_account: SolanaAddress, authority: SolanaAddress) -> Instruction {
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![
            AccountMeta {
                pubkey: nonce_account,
                is_signer: false,
                is_writable: true,
            },
            AccountMeta {
                pubkey: recent_blockhashes_sysvar(),
                is_signer: false,
                is_writable: false,
            },
            AccountMeta {
                pubkey: authority,
                is_signer: true,
                is_writable: false,
            },
        ],
        data: 4u32.to_le_bytes().to_vec(),
    }
}

/// `SystemInstruction::CreateAccountWithSeed` (bincode enum index 3):
/// `base || seed (u64-length-prefixed) || lamports || space || owner`.
fn create_account_with_seed(
    funder_and_base: SolanaAddress,
    new_account: SolanaAddress,
    seed: &str,
    lamports: u64,
    space: u64,
    owner: SolanaAddress,
) -> Instruction {
    let mut data = 3u32.to_le_bytes().to_vec();
    data.extend_from_slice(&funder_and_base.0);
    data.extend_from_slice(&(seed.len() as u64).to_le_bytes());
    data.extend_from_slice(seed.as_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(&owner.0);
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![
            AccountMeta {
                pubkey: funder_and_base,
                is_signer: true,
                is_writable: true,
            },
            AccountMeta {
                pubkey: new_account,
                is_signer: false,
                is_writable: true,
            },
        ],
        data,
    }
}

/// `SystemInstruction::InitializeNonceAccount` (bincode enum index 6).
fn initialize_nonce_account(nonce_account: SolanaAddress, authority: SolanaAddress) -> Instruction {
    let mut data = 6u32.to_le_bytes().to_vec();
    data.extend_from_slice(&authority.0);
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![
            AccountMeta {
                pubkey: nonce_account,
                is_signer: false,
                is_writable: true,
            },
            AccountMeta {
                pubkey: recent_blockhashes_sysvar(),
                is_signer: false,
                is_writable: false,
            },
            AccountMeta {
                pubkey: rent_sysvar(),
                is_signer: false,
                is_writable: false,
            },
        ],
        data,
    }
}

impl ChainAdapter for SvmAdapter {
    fn family(&self) -> &'static str {
        FAMILY
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::Ed25519
    }

    fn derived_address(
        &self,
        public_key: &near_crypto::PublicKey,
    ) -> color_eyre::eyre::Result<String> {
        // A Solana address IS the ed25519 public key, base58-encoded.
        let bytes = crate::mpc::ed25519_bytes(public_key)?;
        Ok(SolanaAddress(bytes).to_base58())
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        let payer = SolanaAddress(crate::mpc::ed25519_bytes(derived_public_key)?);
        let payer_base58 = payer.to_base58();
        let nonce_account = nonce_account_address(payer);
        let rpc = rpc::Client::new(&chain.rpc_url)?;

        let mut after_broadcast = None;
        // Resolve blockhash + instruction prefix per action and latency.
        let (instructions, blockhash_base58, validity_note, summary, required) = match &self.spec {
            SvmActionSpec::SetupNonce { authority } => {
                if latency == ExecutionLatency::Governance {
                    return Err(eyre!(
                        "setup-nonce must run via sign-as-account (it is itself the \
                             prerequisite for the DAO route). To set up a nonce for a DAO's \
                             derived address, run it from your own account with \
                             --nonce-authority <the DAO's derived address>."
                    ));
                }
                let authority = authority.unwrap_or(payer);
                let authority_base58 = authority.to_base58();
                let (nonce_account, seed) = nonce_account_for(payer, authority);
                let nonce_base58 = nonce_account.to_base58();
                if rpc.nonce_account(&nonce_base58)?.is_some() {
                    return Err(eyre!(
                        "The durable nonce account {nonce_base58} for {authority_base58} already \
                         exists - the DAO route is ready to use.{}",
                        nonce_account_flag_hint(authority, payer, &nonce_base58)
                    ));
                }
                let rent = rpc.minimum_rent(NONCE_ACCOUNT_SIZE)?;
                let recent = rpc.latest_blockhash()?.blockhash;
                let for_whom = if authority == payer {
                    String::new()
                } else {
                    format!(" with authority {authority_base58}")
                };
                after_broadcast = Some(format!(
                    "Durable nonce account {nonce_base58} is ready for {authority_base58}.{}",
                    nonce_account_flag_hint(authority, payer, &nonce_base58)
                ));
                (
                    vec![
                        create_account_with_seed(
                            payer,
                            nonce_account,
                            &seed,
                            rent,
                            NONCE_ACCOUNT_SIZE,
                            SYSTEM_PROGRAM,
                        ),
                        initialize_nonce_account(nonce_account, authority),
                    ],
                    recent,
                    "expires in ~60-90 seconds (recent blockhash)".to_string(),
                    format!(
                        "set up the durable nonce account {nonce_base58}{for_whom} (one-time, \
                         enables the DAO route; locks {} for rent exemption)",
                        format_native(rent, chain),
                    ),
                    rent + LAMPORTS_PER_SIGNATURE,
                )
            }
            SvmActionSpec::Transfer { .. } | SvmActionSpec::Instruction { .. } => {
                let (base_instructions, summary, required) = match &self.spec {
                    SvmActionSpec::Transfer { to, lamports } => (
                        vec![utils::system_transfer(payer, *to, *lamports)],
                        format!("transfer {} to {to}", format_native(*lamports, chain)),
                        lamports + LAMPORTS_PER_SIGNATURE,
                    ),
                    SvmActionSpec::Instruction {
                        program_id,
                        accounts,
                        data,
                        summary,
                    } => {
                        let accounts = accounts
                            .iter()
                            .map(|account| {
                                let pubkey = match &account.key {
                                    InstructionAccountKey::Payer => payer,
                                    InstructionAccountKey::Address(address) => *address,
                                };
                                if account.is_signer && pubkey != payer {
                                    return Err(eyre!(
                                        "Account {pubkey} is marked as a signer, but the MPC \
                                             can only sign for the derived address {payer_base58} \
                                             (use `payer` for it)."
                                    ));
                                }
                                Ok(AccountMeta {
                                    pubkey,
                                    is_signer: account.is_signer,
                                    is_writable: account.is_writable,
                                })
                            })
                            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
                        (
                            vec![Instruction {
                                program_id: *program_id,
                                accounts,
                                data: data.clone(),
                            }],
                            summary.clone(),
                            LAMPORTS_PER_SIGNATURE,
                        )
                    }
                    SvmActionSpec::SetupNonce { .. } => unreachable!("handled above"),
                };
                match latency {
                    ExecutionLatency::Immediate => {
                        let recent = rpc
                            .latest_blockhash()
                            .wrap_err("Failed to fetch a recent blockhash")?
                            .blockhash;
                        (
                            base_instructions,
                            recent,
                            "expires in ~60-90 seconds (recent blockhash)".to_string(),
                            summary,
                            required,
                        )
                    }
                    ExecutionLatency::Governance => {
                        let nonce_account = self.nonce_account_override.unwrap_or(nonce_account);
                        let nonce_info =
                                rpc.nonce_account(&nonce_account.to_base58())?
                                    .wrap_err_with(|| {
                                        format!(
                                            "The DAO route on SVM needs a durable nonce account with \
                                             authority {payer_base58}, and none was found at {}.\n\
                                             - For an account-owned derived address, create the \
                                             deterministic one (signed by the derived key itself):\n  \
                                             omni transaction construct svm {chain_key} setup-nonce \
                                             derivation-path {derivation_path} sign-as-account {owner} \
                                             network-config {near_network} sign-with-keychain send\n\
                                             - For a DAO-owned derived address, the DAO cannot create it \
                                             (creation needs a signature within a recent-blockhash \
                                             window - exactly what the nonce unblocks). Create it from \
                                             your own funded derived address instead:\n  \
                                             omni transaction construct svm {chain_key} setup-nonce \
                                             --nonce-authority {payer_base58} derivation-path <your-path> \
                                             sign-as-account <you.near> network-config {near_network} \
                                             sign-with-keychain send\n  \
                                             ... then repeat this command with --nonce-account <the \
                                             printed address>.",
                                            nonce_account.to_base58(),
                                            chain_key = chain.chain_key,
                                            near_network = chain.near_network,
                                        )
                                    })?;
                        if nonce_info.authority != payer_base58 {
                            return Err(eyre!(
                                "The nonce account {} has authority {}, not the derived \
                                     address {payer_base58}.",
                                nonce_account.to_base58(),
                                nonce_info.authority
                            ));
                        }
                        let mut instructions = vec![advance_nonce_account(nonce_account, payer)];
                        instructions.extend(base_instructions);
                        (
                            instructions,
                            nonce_info.durable_nonce_blockhash,
                            format!(
                                "durable nonce (account {}) - valid until this nonce advances; \
                                     one governance transaction per nonce at a time",
                                nonce_account.to_base58()
                            ),
                            summary,
                            required,
                        )
                    }
                }
            }
        };

        let blockhash = Blockhash::from_base58(&blockhash_base58)
            .map_err(|err| eyre!("Invalid blockhash: {err}"))?;

        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(instructions)
            .recent_blockhash(blockhash)
            .build();

        let payload = tx.build_for_signing();

        let balance = rpc.balance(&payer_base58).unwrap_or(0);
        let balance_note = if balance < required {
            format!(
                "\n   {warning} balance {} is below the required {} (amount + fee/rent) - \
                 fund the derived address first",
                format_native(balance, chain),
                format_native(required, chain),
                warning = "WARNING:".yellow(),
            )
        } else {
            String::new()
        };

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:            {summary}\n\
             fee payer (from):  {payer_base58} (derived: {owner} / \"{derivation_path}\")\n\
             balance:           {balance}{balance_note}\n\
             blockhash:         {blockhash_base58}\n\
             validity:          {validity_note}\n\
             base fee:          {fee}\n\
             signing payload:   {payload_len} bytes (full message, signed as-is by the MPC)\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            balance = format_native(balance, chain),
            fee = format_native(LAMPORTS_PER_SIGNATURE, chain),
            payload_len = payload.len(),
        );

        let mut unsigned_tx = serde_json::to_value(&tx)?;
        prettify_instruction_data(&mut unsigned_tx);

        Ok(BuiltTransaction {
            unsigned_tx,
            payloads: vec![payload],
            display,
            after_broadcast,
        })
    }
}

/// Recomputes the MPC signing payload from an envelope's unsigned tx -
/// the byte-equality half of `proposal review`.
pub(crate) fn signing_payloads_from_envelope(
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<Vec<Vec<u8>>> {
    let mut unsigned_tx = unsigned_tx.clone();
    unprettify_instruction_data(&mut unsigned_tx)?;
    let tx: SolanaTransaction = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Solana transaction")?;
    Ok(vec![tx.build_for_signing()])
}

/// Combines the unsigned Solana transaction with the MPC signature and
/// broadcasts it; returns the transaction signature (its id).
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let mut unsigned_tx = unsigned_tx.clone();
    unprettify_instruction_data(&mut unsigned_tx)?;
    let tx: SolanaTransaction = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Solana transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let signature = signature_from_mpc(response)?;
    let wire_bytes = tx.build_with_signature(&[signature]);
    let tx_base64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&wire_bytes)
    };
    rpc::Client::new(&chain.rpc_url)?.send_transaction(&tx_base64)
}

/// Envelope prettification: instruction data blobs are emitted as number
/// arrays by the upstream serde; show them as hex.
fn prettify_instruction_data(unsigned_tx: &mut serde_json::Value) {
    for variant in ["Legacy", "V0"] {
        if let Some(instructions) = unsigned_tx
            .pointer_mut(&format!("/message/{variant}/instructions"))
            .and_then(serde_json::Value::as_array_mut)
        {
            for instruction in instructions {
                if let Some(data) = instruction.get_mut("data") {
                    crate::chains::bytes_array_to_hex(data);
                }
            }
        }
    }
}

fn unprettify_instruction_data(
    unsigned_tx: &mut serde_json::Value,
) -> color_eyre::eyre::Result<()> {
    for variant in ["Legacy", "V0"] {
        if let Some(instructions) = unsigned_tx
            .pointer_mut(&format!("/message/{variant}/instructions"))
            .and_then(serde_json::Value::as_array_mut)
        {
            for instruction in instructions {
                if let Some(data) = instruction.get_mut("data") {
                    crate::chains::hex_to_bytes_array(data)?;
                }
            }
        }
    }
    Ok(())
}

pub fn signature_from_mpc(
    response: &MpcSignatureResponse,
) -> color_eyre::eyre::Result<SolanaSignature> {
    let MpcSignatureResponse::Ed25519 { signature } = response else {
        return Err(eyre!(
            "Expected an ed25519 MPC signature for an SVM chain, got a secp256k1 one"
        ));
    };
    let bytes: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        eyre!(
            "MPC ed25519 signature must be 64 bytes, got {}",
            signature.len()
        )
    })?;
    Ok(SolanaSignature(bytes))
}

fn format_native(lamports: u64, chain: &ResolvedChain) -> String {
    crate::types::format_move_style_amount(
        lamports,
        &chain.symbol,
        "lamports",
        10u64.pow(u32::from(chain.decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_account_for_another_authority_is_deterministic_and_seed_fits() {
        let payer = SolanaAddress([1u8; 32]);
        let dao = SolanaAddress([2u8; 32]);
        let (own, own_seed) = nonce_account_for(payer, payer);
        assert_eq!(own, nonce_account_address(payer));
        assert_eq!(own_seed, NONCE_SEED);
        let (for_dao, seed) = nonce_account_for(payer, dao);
        assert!(seed.len() <= 32, "create_with_seed caps seeds at 32 bytes");
        assert_ne!(for_dao, own);
        assert_eq!(nonce_account_for(payer, dao).0, for_dao);
        // A different payer creating for the same DAO gets a different account.
        assert_ne!(nonce_account_for(SolanaAddress([3u8; 32]), dao).0, for_dao);
    }

    /// Round-trip through a real ed25519 key: build a transfer, sign the full
    /// message payload the way the MPC does, assemble, and verify both the
    /// wire layout and the signature against the derived address.
    #[test]
    fn ed25519_signature_assembles_and_verifies() {
        use ed25519_dalek::{Signer, SigningKey, Verifier};

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let payer_bytes = signing_key.verifying_key().to_bytes();
        let payer = SolanaAddress(payer_bytes);
        let to = SolanaAddress([9u8; 32]);

        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(vec![utils::system_transfer(payer, to, 1_000_000)])
            .recent_blockhash(Blockhash([3u8; 32]))
            .build();
        let payload = tx.build_for_signing();

        // The MPC signs the full message bytes with ed25519 (no pre-hash)
        let signature = signing_key.sign(&payload);
        let response = MpcSignatureResponse::Ed25519 {
            signature: signature.to_bytes().to_vec(),
        };

        let solana_signature = signature_from_mpc(&response).unwrap();
        let wire_bytes = tx.build_with_signature(&[solana_signature]);

        // Wire layout: compact-u16 count (1), 64 signature bytes, then the message
        assert_eq!(wire_bytes[0], 1);
        assert_eq!(&wire_bytes[1..65], signature.to_bytes().as_slice());
        assert_eq!(&wire_bytes[65..], payload.as_slice());

        // And the signature must verify against the derived address (= pubkey)
        signing_key
            .verifying_key()
            .verify(&payload, &signature)
            .unwrap();

        // Adapter address derivation matches the payer
        let adapter = SvmAdapter {
            spec: SvmActionSpec::Transfer {
                to,
                lamports: 1_000_000,
            },
            nonce_account_override: None,
        };
        let near_pk = near_crypto::PublicKey::ED25519(near_crypto::ED25519PublicKey(payer_bytes));
        assert_eq!(
            adapter.derived_address(&near_pk).unwrap(),
            payer.to_base58()
        );
    }

    /// The durable-nonce system instructions must match the bincode layout of
    /// `SystemInstruction` (enum index u32 LE + fields).
    #[test]
    fn nonce_instructions_have_correct_layout() {
        let base = SolanaAddress([7u8; 32]);
        let nonce = nonce_account_address(base);

        // Deterministic: same base -> same nonce account, different from base
        assert_eq!(nonce, nonce_account_address(base));
        assert_ne!(nonce.0, base.0);

        let advance = advance_nonce_account(nonce, base);
        assert_eq!(advance.data, vec![4, 0, 0, 0]);
        assert_eq!(advance.program_id, SYSTEM_PROGRAM);
        assert_eq!(advance.accounts.len(), 3);
        assert!(advance.accounts[0].is_writable && !advance.accounts[0].is_signer);
        assert!(advance.accounts[2].is_signer && !advance.accounts[2].is_writable);

        let create = create_account_with_seed(
            base,
            nonce,
            NONCE_SEED,
            1_500_000,
            NONCE_ACCOUNT_SIZE,
            SYSTEM_PROGRAM,
        );
        // u32 index + base(32) + u64 seed len + seed + lamports u64 + space u64 + owner(32)
        assert_eq!(
            create.data.len(),
            4 + 32 + 8 + NONCE_SEED.len() + 8 + 8 + 32
        );
        assert_eq!(&create.data[..4], &[3, 0, 0, 0]);
        assert_eq!(&create.data[4..36], &base.0);
        assert_eq!(
            &create.data[36..44],
            &(NONCE_SEED.len() as u64).to_le_bytes()
        );

        let init = initialize_nonce_account(nonce, base);
        assert_eq!(&init.data[..4], &[6, 0, 0, 0]);
        assert_eq!(&init.data[4..36], &base.0);
        assert_eq!(init.accounts.len(), 3);
    }

    #[test]
    fn governance_transfer_uses_durable_nonce_shape() {
        // Building the instruction list by hand the way the adapter does:
        // advance must come first, authority = payer.
        let payer = SolanaAddress([7u8; 32]);
        let nonce = nonce_account_address(payer);
        let instructions = vec![
            advance_nonce_account(nonce, payer),
            utils::system_transfer(payer, SolanaAddress([9u8; 32]), 1),
        ];
        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(instructions)
            .recent_blockhash(Blockhash([3u8; 32])) // = the durable nonce value
            .build();
        // One required signature (the payer/authority), advance compiled first
        let payload = tx.build_for_signing();
        assert!(!payload.is_empty());
        assert_eq!(tx.message.num_required_signatures(), 1);
    }

    /// The envelope form shows instruction data as hex and round-trips exactly.
    #[test]
    fn envelope_instruction_data_prettifies_and_round_trips() {
        let payer = SolanaAddress([7u8; 32]);
        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(vec![utils::system_transfer(
                payer,
                SolanaAddress([9u8; 32]),
                1_000_000,
            )])
            .recent_blockhash(Blockhash([3u8; 32]))
            .build();
        let expected_payload = tx.build_for_signing();

        let mut unsigned_tx = serde_json::to_value(&tx).unwrap();
        prettify_instruction_data(&mut unsigned_tx);
        let data = &unsigned_tx["message"]["Legacy"]["instructions"][0]["data"];
        assert_eq!(data, "0x0200000040420f0000000000");

        unprettify_instruction_data(&mut unsigned_tx).unwrap();
        let restored: SolanaTransaction = serde_json::from_value(unsigned_tx).unwrap();
        assert_eq!(restored.build_for_signing(), expected_payload);
    }
}
