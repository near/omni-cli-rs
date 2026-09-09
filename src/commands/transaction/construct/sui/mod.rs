//! `omni transaction construct sui <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::sui::{SuiActionSpec, SuiAdapter};
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::move_address::MoveAddress;
use crate::types::sui_amount::SuiAmount;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = SuiChainContext)]
pub struct SuiChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which Sui chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: SuiAction,
}

#[derive(Clone)]
pub struct SuiChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl SuiChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<SuiChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::sui::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl SuiChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::sui::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = SuiChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum SuiAction {
    #[strum_discriminants(strum(message = "transfer   -   Transfer SUI"))]
    /// Transfer SUI
    Transfer(Transfer),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SuiChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (0x...):
    receiver: MoveAddress,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 SUI):
    amount: SuiAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: SuiChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = SuiActionSpec::Transfer {
            to: scope.receiver.0,
            mist: scope.amount.mist,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(SuiAdapter { spec }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &SuiChainContext) -> color_eyre::eyre::Result<Option<SuiAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 SUI, 1000 mist):").prompt()?,
        ))
    }
}
