//! `omni transaction construct utxo <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::utxo::{UtxoActionSpec, UtxoAdapter};
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::btc_address::BtcAddressArg;
use crate::types::btc_amount::BtcAmount;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = UtxoChainContext)]
pub struct UtxoChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which UTXO chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: UtxoAction,
}

#[derive(Clone)]
pub struct UtxoChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl UtxoChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<UtxoChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::utxo::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl UtxoChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::utxo::FAMILY)
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = UtxoChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum UtxoAction {
    #[strum_discriminants(strum(
        message = "transfer   -   Transfer BTC (change returns to the derived address)"
    ))]
    /// Transfer BTC (change returns to the derived address)
    Transfer(Transfer),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = UtxoChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (bech32 or base58):
    receiver: BtcAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 BTC):
    amount: BtcAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: UtxoChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = UtxoActionSpec::Transfer {
            to: scope.receiver.0.clone(),
            sats: scope.amount.sats,
        };
        Ok(Self(SpecContext {
            global_context: previous_context.global_context,
            chain_key: previous_context.selected.chain_key.clone(),
            chain_def: previous_context.selected.chain_def.clone(),
            mpc_config: previous_context.selected.mpc_config.clone(),
            adapter: Arc::new(UtxoAdapter { spec }),
        }))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &UtxoChainContext) -> color_eyre::eyre::Result<Option<BtcAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 BTC, 1000 sats):").prompt()?,
        ))
    }
}
