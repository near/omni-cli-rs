//! `omni construct svm <chain> <action> ... derivation-path <path> sign-as-...`

use std::collections::BTreeMap;
use std::sync::Arc;

use color_eyre::eyre::WrapErr;
use inquire::CustomType;
use omni_transaction::solana::types::SolanaAddress;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::svm::idl::{self, AccountNeed, AccountSlot, Idl, IdlType, ProvidedAccount};
use crate::chains::svm::{InstructionAccount, InstructionAccountKey, SvmActionSpec, SvmAdapter};
use crate::commands::transaction::construct::guided;
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::hex_bytes::HexBytes;
use crate::types::sol_amount::SolAmount;
use crate::types::solana_address::SolanaAddressArg;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = SvmChainContext)]
pub struct SvmChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which SVM chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: SvmAction,
}

#[derive(Clone)]
pub struct SvmChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl SvmChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<SvmChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::svm::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl SvmChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::svm::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = SvmChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum SvmAction {
    #[strum_discriminants(strum(
        message = "transfer      -   Transfer the native token (SOL, ...)"
    ))]
    /// Transfer the native token (SOL, ...)
    Transfer(Transfer),
    #[strum_discriminants(strum(
        message = "instruction   -   Call a program: one raw instruction (program id, accounts, data)"
    ))]
    /// Call a program: one raw instruction (program id, accounts, data)
    Instruction(RawInstruction),
    #[strum_discriminants(strum(
        message = "setup-nonce   -   One-time durable nonce account setup for this or a DAO's derived address (enables the DAO route)"
    ))]
    /// One-time durable nonce account setup for this or a DAO's derived address (enables the DAO route)
    SetupNonce(SetupNonce),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SvmChainContext)]
#[interactive_clap(output_context = RawInstructionContext)]
pub struct RawInstruction {
    #[interactive_clap(skip_default_input_arg)]
    /// Program id (base58):
    program_id: SolanaAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Accounts: JSON array of "<pubkey>[:flags]" with flags s (signer) / w (writable); "payer" = the derived address, e.g. '["payer:sw", "So11...:w"]'
    accounts: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Instruction data as hex (0x-prefixed or bare; empty for none):
    data: HexBytes,
    /// Externally created durable nonce account for the DAO route (authority must be the derived address)
    #[interactive_clap(long)]
    #[interactive_clap(skip_interactive_input)]
    nonce_account: Option<SolanaAddressArg>,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct RawInstructionContext(SpecContext);

impl RawInstructionContext {
    pub fn from_previous_context(
        previous_context: SvmChainContext,
        scope: &<RawInstruction as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let accounts = crate::chains::move_call::parse_string_list(&scope.accounts)?
            .iter()
            .map(|entry| parse_instruction_account(entry))
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
        let spec = SvmActionSpec::Instruction {
            program_id: scope.program_id.0,
            accounts: accounts.clone(),
            data: scope.data.0.clone(),
            summary: format!(
                "call program {} ({} account(s), {} data byte(s))",
                scope.program_id,
                accounts.len(),
                scope.data.0.len()
            ),
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SvmAdapter {
                spec,
                nonce_account_override: scope.nonce_account.map(|address| address.0),
            }),
        }))
    }
}

impl From<RawInstructionContext> for SpecContext {
    fn from(item: RawInstructionContext) -> Self {
        item.0
    }
}

/// `<pubkey>[:flags]` where flags are any of `s` (signer) and `w`
/// (writable); `payer` stands for the derived address.
fn parse_instruction_account(entry: &str) -> color_eyre::eyre::Result<InstructionAccount> {
    let (key, flags) = entry.trim().split_once(':').unwrap_or((entry.trim(), ""));
    for flag in flags.chars() {
        if flag != 's' && flag != 'w' {
            return Err(color_eyre::eyre::eyre!(
                "Unknown account flag '{flag}' in '{entry}' (use s = signer, w = writable)"
            ));
        }
    }
    let key = if key.eq_ignore_ascii_case("payer") {
        InstructionAccountKey::Payer
    } else {
        InstructionAccountKey::Address(
            key.parse::<SolanaAddressArg>()
                .map_err(|err| color_eyre::eyre::eyre!("{err}"))?
                .0,
        )
    };
    Ok(InstructionAccount {
        key,
        is_signer: flags.contains('s'),
        is_writable: flags.contains('w'),
    })
}

/// The program id typed in the previous prompt, parked for the accounts
/// prompt, which looks up the program's IDL (see [`guided`]).
#[derive(Clone, Copy)]
struct GuidedProgram(SolanaAddress);

/// Instruction data assembled by the guided accounts prompt, parked for the
/// data prompt.
struct GuidedData(Vec<u8>);

impl RawInstruction {
    fn input_program_id(
        _context: &SvmChainContext,
    ) -> color_eyre::eyre::Result<Option<SolanaAddressArg>> {
        let program = CustomType::<SolanaAddressArg>::new("Program id (base58):").prompt()?;
        guided::stash(GuidedProgram(program.0));
        Ok(Some(program))
    }

    fn input_accounts(context: &SvmChainContext) -> color_eyre::eyre::Result<Option<String>> {
        if let Some(GuidedProgram(program)) = guided::take_stashed::<GuidedProgram>()
            && let Some(accounts) = guided_accounts(context, program)?
        {
            return Ok(Some(accounts));
        }
        Ok(Some(
            inquire::Text::new(
                "Accounts (JSON array of \"<pubkey>[:flags]\", flags s/w; \"payer\" = derived address):",
            )
            .with_initial_value("[\"payer:sw\"]")
            .prompt()?,
        ))
    }

    fn input_data(_context: &SvmChainContext) -> color_eyre::eyre::Result<Option<HexBytes>> {
        if let Some(GuidedData(data)) = guided::take_stashed::<GuidedData>() {
            return Ok(Some(HexBytes(data)));
        }
        Ok(Some(
            CustomType::new("Instruction data (hex; empty for none):")
                .with_default(HexBytes::default())
                .prompt()?,
        ))
    }
}

// --------------------------------------------------------- guided (IDL) flow

/// Finds the program's Anchor IDL: the on-chain IDL account on each of the
/// chain's networks, else a file or URL the user points at. `None` when the
/// user prefers to type the accounts by hand.
fn load_idl(
    context: &SvmChainContext,
    program: SolanaAddress,
) -> color_eyre::eyre::Result<Option<Idl>> {
    const FROM_FILE: &str = "Load the IDL from a file (e.g. target/idl/<program>.json)";
    const FROM_URL: &str = "Load the IDL from a URL";
    const MANUAL: &str = "Type the accounts and data manually";

    let program_base58 = program.to_base58();
    guided::note(&format!(
        "Looking up the Anchor IDL of {program_base58} ..."
    ));
    let idl_account = idl::idl_account_address(program).to_base58();
    for (network, variant) in guided::lookup_networks(&context.selected.chain_def) {
        let client = match crate::chains::svm::rpc::Client::new(&variant.rpc_url) {
            Ok(client) => client,
            Err(err) => {
                guided::note(&format!("  {network}: {err}"));
                continue;
            }
        };
        match client.account_data(&idl_account) {
            Ok(Some(data)) => match idl::parse_idl_account(&data) {
                Ok(idl) => {
                    guided::note(&format!(
                        "  found on {network}: {} ({} instruction(s))",
                        idl.name,
                        idl.instructions.len()
                    ));
                    return Ok(Some(idl));
                }
                Err(err) => guided::note(&format!("  {network}: IDL account unreadable: {err}")),
            },
            Ok(None) => guided::note(&format!("  {network}: no IDL published on-chain")),
            Err(err) => guided::note(&format!("  {network}: {err}")),
        }
    }

    let choice = inquire::Select::new(
        "No published IDL found. How do you want to continue?",
        vec![FROM_FILE, FROM_URL, MANUAL],
    )
    .prompt()?;
    let json = match choice {
        FROM_FILE => {
            let path = inquire::Text::new("IDL file path:").prompt()?;
            std::fs::read_to_string(path.trim())
                .wrap_err_with(|| format!("Failed to read the IDL file '{path}'"))?
        }
        FROM_URL => {
            let url = inquire::Text::new("IDL URL:").prompt()?;
            reqwest::blocking::get(url.trim())
                .and_then(reqwest::blocking::Response::error_for_status)
                .and_then(reqwest::blocking::Response::text)
                .wrap_err_with(|| format!("Failed to fetch the IDL from '{url}'"))?
        }
        _ => return Ok(None),
    };
    let idl = idl::parse_idl_json(&json)?;
    if let Some(address) = idl.address
        && address != program
    {
        crate::output::warn(format!(
            "the IDL declares program {}, not {program_base58} - make sure it describes \
             the program you are calling.",
            address.to_base58()
        ));
    }
    Ok(Some(idl))
}

/// The guided accounts prompt: pick an instruction from the IDL, enter its
/// arguments by type, and let the IDL derive every account it can; the
/// user is asked only for the rest. Returns the accounts in the
/// non-interactive grammar and parks the encoded data for the next prompt.
/// `None` means the user opted for manual entry.
fn guided_accounts(
    context: &SvmChainContext,
    program: SolanaAddress,
) -> color_eyre::eyre::Result<Option<String>> {
    let Some(idl) = load_idl(context, program)? else {
        return Ok(None);
    };
    let options: Vec<String> = idl
        .instructions
        .iter()
        .map(|instruction| match instruction.docs.first() {
            Some(doc) => format!("{}  - {}", instruction.signature(), doc.trim()),
            None => instruction.signature(),
        })
        .collect();
    let Some(index) = guided::select_or_manual("Which instruction?", options)? else {
        return Ok(None);
    };
    let instruction = &idl.instructions[index];
    for line in &instruction.docs {
        guided::note(&format!("  {line}"));
    }
    if let Some(arg) = instruction
        .args
        .iter()
        .find(|arg| matches!(arg.ty, IdlType::Unsupported(_)))
    {
        guided::note(&format!(
            "Argument '{}' has type {}, which this CLI cannot encode - falling back to manual \
             entry.",
            arg.name, arg.ty
        ));
        return Ok(None);
    }

    // Arguments first: PDA seeds may depend on them.
    let mut args = Vec::with_capacity(instruction.args.len());
    for arg in &instruction.args {
        let encoded = loop {
            let text = inquire::Text::new(&format!("{} ({}):", arg.name, arg.ty))
                .with_help_message(&type_help(&arg.ty, &idl.types))
                .prompt()?;
            match idl::encode_arg(&arg.ty, &text, &idl.types) {
                Ok(mut encoded) => {
                    encoded.name.clone_from(&arg.name);
                    break encoded;
                }
                Err(err) => crate::output::warn(format!("{err:#}")),
            }
        };
        args.push(encoded);
    }

    // Accounts: derive what the IDL pins down, ask for the rest - plain
    // accounts before PDAs, since PDAs usually resolve once those are known.
    let mut provided: BTreeMap<String, ProvidedAccount> = BTreeMap::new();
    let resolution = loop {
        let resolution = idl::resolve_accounts(instruction, program, &args, &provided)?;
        let next = resolution
            .unresolved()
            .find(|need| need.blocked_on.is_none())
            .or_else(|| resolution.unresolved().next())
            .cloned();
        let Some(need) = next else {
            break resolution;
        };
        let value = prompt_account(&need, program)?;
        provided.insert(need.path.clone(), value);
    };

    let mut table = crate::commands::new_table();
    table.set_titles(crate::commands::title_row(&[
        "Account", "Address", "Flags", "Source",
    ]));
    for slot in &resolution.slots {
        let AccountSlot::Resolved(account) = slot else {
            unreachable!("the loop above resolves every account");
        };
        let address = match &account.key {
            InstructionAccountKey::Payer => "payer (the derived address)".to_string(),
            InstructionAccountKey::Address(address) => address.to_base58(),
        };
        let mut flags = Vec::new();
        if account.signer {
            flags.push("signer");
        }
        if account.writable {
            flags.push("writable");
        }
        table.add_row(prettytable::row![
            account.path,
            address,
            flags.join(", "),
            account.source.to_string()
        ]);
    }
    table.printstd();

    guided::stash(GuidedData(idl::encode_instruction_data(
        instruction.discriminator,
        &args,
    )));
    Ok(Some(serde_json::to_string(&resolution.account_specs()?)?))
}

/// Asks for one account the IDL does not determine: a select when the name
/// implies a small set (token programs), otherwise free text pre-filled
/// with the conventional default. `payer` names the derived address.
fn prompt_account(
    need: &AccountNeed,
    program: SolanaAddress,
) -> color_eyre::eyre::Result<ProvidedAccount> {
    let mut flags = Vec::new();
    flags.push(if need.writable {
        "writable"
    } else {
        "read-only"
    });
    if need.optional {
        flags.push("optional");
    }
    let mut help = need
        .docs
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(reason) = &need.blocked_on {
        if !help.is_empty() {
            help.push_str(" | ");
        }
        help.push_str("could not derive: ");
        help.push_str(reason);
    }
    let help = if help.is_empty() { None } else { Some(help) };

    if need.optional {
        let question = format!(
            "Provide the optional account '{}'? (no = leave it out)",
            need.path
        );
        let mut confirm = inquire::Confirm::new(&question).with_default(false);
        if let Some(help) = &help {
            confirm = confirm.with_help_message(help);
        }
        if !confirm.prompt()? {
            // Anchor's convention for an omitted optional account.
            return Ok(ProvidedAccount::Address(program));
        }
    }

    let label = format!("{} ({}):", need.path, flags.join(", "));
    if !need.choices.is_empty() {
        const OTHER: &str = "another address";
        let mut options: Vec<String> = need
            .choices
            .iter()
            .map(|(name, address)| format!("{name} ({})", address.to_base58()))
            .collect();
        options.push(OTHER.to_string());
        let choice = inquire::Select::new(&label, options).raw_prompt()?;
        if let Some((_, address)) = need.choices.get(choice.index) {
            return Ok(ProvidedAccount::Address(*address));
        }
    }
    let default = need.default.map(|address| address.to_base58());
    loop {
        let mut prompt = inquire::Text::new(&label);
        if let Some(default) = &default {
            prompt = prompt.with_initial_value(default);
        }
        if let Some(help) = &help {
            prompt = prompt.with_help_message(help);
        }
        let text = prompt.prompt()?;
        let text = text.trim();
        if text.eq_ignore_ascii_case("payer") {
            return Ok(ProvidedAccount::Payer);
        }
        match SolanaAddress::from_base58(text) {
            Ok(address) => return Ok(ProvidedAccount::Address(address)),
            Err(err) => crate::output::warn(format!(
                "'{text}' is not a base58 address ({err}); type an address or `payer`"
            )),
        }
    }
}

/// How to type a value of an IDL type.
fn type_help(ty: &IdlType, types: &BTreeMap<String, idl::IdlTypeDef>) -> String {
    match ty {
        IdlType::Bool => "true or false".to_string(),
        IdlType::String => "text, as is".to_string(),
        IdlType::Pubkey => "base58 address".to_string(),
        IdlType::Bytes => "hex (0x...) or a JSON array of bytes".to_string(),
        IdlType::F32 | IdlType::F64 => "decimal number".to_string(),
        IdlType::Vec(inner) => format!("JSON array of {inner}"),
        IdlType::Array(inner, len) => format!("JSON array of {len} x {inner}"),
        IdlType::Option(inner) | IdlType::COption(inner) => format!("null, or a {inner}"),
        IdlType::Defined(name) => match types.get(name) {
            Some(idl::IdlTypeDef::Struct(fields)) => format!(
                "JSON object with fields {}",
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, field.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(idl::IdlTypeDef::Enum(variants)) => format!(
                "one of {} (variants with fields as {{\"Variant\": ...}})",
                variants
                    .iter()
                    .map(|variant| variant.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("{name} (not defined in the IDL)"),
        },
        IdlType::Unsupported(what) => format!("unsupported: {what}"),
        _ => "integer in decimal".to_string(),
    }
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SvmChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (base58):
    receiver: SolanaAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 SOL, 0.5 FOGO, 5000 lamports):
    amount: SolAmount,
    /// Externally created durable nonce account for the DAO route (authority must be the derived address)
    #[interactive_clap(long)]
    #[interactive_clap(skip_interactive_input)]
    nonce_account: Option<SolanaAddressArg>,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: SvmChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = SvmActionSpec::Transfer {
            to: scope.receiver.0,
            lamports: scope.amount.lamports,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SvmAdapter {
                spec,
                nonce_account_override: scope.nonce_account.map(|address| address.0),
            }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(context: &SvmChainContext) -> color_eyre::eyre::Result<Option<SolAmount>> {
        // Show the chain's own symbol in the example (SOL, FOGO, ...).
        let symbol = context
            .selected
            .chain_def
            .networks
            .values()
            .find_map(|network| network.symbol.clone())
            .unwrap_or_else(|| "SOL".to_string());
        Ok(Some(
            CustomType::new(&format!(
                "Amount to transfer (e.g. 0.5 {symbol}, 5000 lamports):"
            ))
            .prompt()?,
        ))
    }
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SvmChainContext)]
#[interactive_clap(output_context = SetupNonceContext)]
pub struct SetupNonce {
    /// Create the nonce account for another derived address (e.g. a DAO's) instead of this one; that address becomes its authority
    #[interactive_clap(long)]
    #[interactive_clap(skip_default_input_arg)]
    nonce_authority: Option<SolanaAddressArg>,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the paying account (direct route only)
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPathAccountOnly,
}

#[derive(Clone)]
pub struct SetupNonceContext(SpecContext);

impl SetupNonceContext {
    pub fn from_previous_context(
        previous_context: SvmChainContext,
        scope: &<SetupNonce as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SvmAdapter {
                spec: SvmActionSpec::SetupNonce {
                    authority: scope.nonce_authority.map(|address| address.0),
                },
                nonce_account_override: None,
            }),
        }))
    }
}

impl SetupNonce {
    fn input_nonce_authority(
        _context: &SvmChainContext,
    ) -> color_eyre::eyre::Result<Option<SolanaAddressArg>> {
        const OWN: &str = "this derived address itself (it signs its own DAO-route transactions)";
        const OTHER: &str = "another derived address, e.g. a DAO's - it cannot create its own, so this account pays";
        let choice =
            inquire::Select::new("Who will use the nonce account?", vec![OWN, OTHER]).prompt()?;
        if choice == OWN {
            return Ok(None);
        }
        Ok(Some(
            CustomType::<SolanaAddressArg>::new(
                "Derived address that will own (be the authority of) the nonce account (base58):",
            )
            .prompt()?,
        ))
    }
}

impl From<SetupNonceContext> for SpecContext {
    fn from(item: SetupNonceContext) -> Self {
        item.0
    }
}
