//! `omni construct svm <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::svm::{SvmActionSpec, SvmAdapter};
use crate::commands::construct::{SelectedChain, SpecContext};
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
            selected: Arc::new(crate::commands::construct::load_chain(
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
        crate::commands::construct::input_chain(crate::chains::svm::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = SvmChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum SvmAction {
    #[strum_discriminants(strum(
        message = "transfer   -   Transfer the native token (SOL, ...)"
    ))]
    /// Transfer the native token (SOL, ...)
    Transfer(Transfer),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SvmChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (base58):
    receiver: SolanaAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 SOL):
    amount: SolAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: SvmChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = SvmActionSpec::Transfer {
            to: scope.receiver.to_string(),
            lamports: scope.amount.lamports,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SvmAdapter { spec }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &SvmChainContext) -> color_eyre::eyre::Result<Option<SolAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 SOL, 5000 lamports):").prompt()?,
        ))
    }
}
