//! `omni construct svm <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::svm::{InstructionAccount, InstructionAccountKey, SvmActionSpec, SvmAdapter};
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
        message = "setup-nonce   -   One-time durable nonce account setup (enables the DAO route)"
    ))]
    /// One-time durable nonce account setup (enables the DAO route)
    SetupNonce(SetupNonce),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SvmChainContext)]
#[interactive_clap(output_context = RawInstructionContext)]
pub struct RawInstruction {
    /// Program id (base58):
    program_id: SolanaAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Accounts: JSON array of "<pubkey>[:flags]" with flags s (signer) / w (writable); "payer" = the derived address, e.g. '["payer:sw", "So11...:w"]'
    accounts: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Instruction data as hex (0x-prefixed or bare; empty for none):
    data: HexBytes,
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
                nonce_account_override: None,
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

impl RawInstruction {
    fn input_accounts(_context: &SvmChainContext) -> color_eyre::eyre::Result<Option<String>> {
        Ok(Some(
            inquire::Text::new(
                "Accounts (JSON array of \"<pubkey>[:flags]\", flags s/w; \"payer\" = derived address):",
            )
            .with_initial_value("[\"payer:sw\"]")
            .prompt()?,
        ))
    }

    fn input_data(_context: &SvmChainContext) -> color_eyre::eyre::Result<Option<HexBytes>> {
        Ok(Some(
            CustomType::new("Instruction data (hex; empty for none):")
                .with_default(HexBytes::default())
                .prompt()?,
        ))
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
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account (direct route only)
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPathAccountOnly,
}

#[derive(Clone)]
pub struct SetupNonceContext(SpecContext);

impl SetupNonceContext {
    pub fn from_previous_context(
        previous_context: SvmChainContext,
        _scope: &<SetupNonce as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SvmAdapter {
                spec: SvmActionSpec::SetupNonce,
                nonce_account_override: None,
            }),
        }))
    }
}

impl From<SetupNonceContext> for SpecContext {
    fn from(item: SetupNonceContext) -> Self {
        item.0
    }
}
