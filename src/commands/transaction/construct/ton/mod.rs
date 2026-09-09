//! `omni transaction construct ton <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::ton::{TonActionSpec, TonAdapter};
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::ton_address::TonAddressArg;
use crate::types::ton_amount::TonAmount;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = TonChainContext)]
pub struct TonChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which TON chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: TonAction,
}

#[derive(Clone)]
pub struct TonChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl TonChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<TonChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::ton::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl TonChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::ton::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = TonChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum TonAction {
    #[strum_discriminants(strum(
        message = "transfer   -   Transfer TON (deploys the derived wallet on first use)"
    ))]
    /// Transfer TON (deploys the derived wallet on first use)
    Transfer(Transfer),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = TonChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (friendly base64, e.g. UQ.../EQ...):
    receiver: TonAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 TON):
    amount: TonAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: TonChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = TonActionSpec::Transfer {
            to: scope.receiver.0,
            nanotons: scope.amount.nanotons,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(TonAdapter { spec }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &TonChainContext) -> color_eyre::eyre::Result<Option<TonAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 TON, 1000 nanotons):").prompt()?,
        ))
    }
}
