//! `omni transaction construct aptos <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::aptos::{AptosActionSpec, AptosAdapter};
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::apt_amount::AptAmount;
use crate::types::move_address::MoveAddress;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = AptosChainContext)]
pub struct AptosChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which Aptos chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: AptosAction,
}

#[derive(Clone)]
pub struct AptosChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl AptosChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<AptosChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::aptos::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl AptosChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::aptos::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = AptosChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum AptosAction {
    #[strum_discriminants(strum(
        message = "transfer   -   Transfer APT (creates the recipient account if needed)"
    ))]
    /// Transfer APT (creates the recipient account if needed)
    Transfer(Transfer),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = AptosChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (0x...):
    receiver: MoveAddress,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 APT):
    amount: AptAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: AptosChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = AptosActionSpec::Transfer {
            to: omni_transaction::aptos::types::AccountAddress(scope.receiver.0),
            octas: scope.amount.octas,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(AptosAdapter { spec }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &AptosChainContext) -> color_eyre::eyre::Result<Option<AptAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 APT, 1000 octas):").prompt()?,
        ))
    }
}
