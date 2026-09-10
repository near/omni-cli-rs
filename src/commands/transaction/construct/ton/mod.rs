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
        message = "transfer       -   Transfer TON (deploys the derived wallet on first use)"
    ))]
    /// Transfer TON (deploys the derived wallet on first use)
    Transfer(Transfer),
    #[strum_discriminants(strum(
        message = "send-message   -   Send an internal message with a body (contract call / comment)"
    ))]
    /// Send an internal message with a body (contract call / comment)
    SendMessage(SendMessage),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = TonChainContext)]
#[interactive_clap(output_context = SendMessageContext)]
pub struct SendMessage {
    /// Destination address (a contract or wallet):
    receiver: TonAddressArg,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to attach (e.g. 0.05 TON - covers the callee's gas):
    amount: TonAmount,
    #[interactive_clap(skip_default_input_arg)]
    /// Message body: 'comment:<text>', 'boc:<hex of a body cell BOC>', or 'none':
    body: String,
    /// Send non-bounceable (default: bounceable, so a failed call refunds the value)
    #[interactive_clap(long)]
    non_bounceable: bool,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct SendMessageContext(SpecContext);

impl SendMessageContext {
    pub fn from_previous_context(
        previous_context: TonChainContext,
        scope: &<SendMessage as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        use crate::chains::ton::TonBody;

        let body_text = scope.body.trim();
        let body = match body_text.split_once(':') {
            _ if body_text.eq_ignore_ascii_case("none") || body_text.is_empty() => TonBody::None,
            Some(("comment", text)) => TonBody::Comment(text.to_string()),
            Some(("boc", hex_text)) => TonBody::Boc(
                hex::decode(hex_text.trim().trim_start_matches("0x"))
                    .map_err(|err| color_eyre::eyre::eyre!("Invalid body BOC hex: {err}"))?,
            ),
            _ => {
                return Err(color_eyre::eyre::eyre!(
                    "Body must be 'comment:<text>', 'boc:<hex>', or 'none', got '{body_text}'"
                ));
            }
        };
        let spec = TonActionSpec::Message {
            to: scope.receiver.0,
            nanotons: scope.amount.nanotons,
            body,
            bounce: !scope.non_bounceable,
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

impl From<SendMessageContext> for SpecContext {
    fn from(item: SendMessageContext) -> Self {
        item.0
    }
}

impl SendMessage {
    fn input_amount(_context: &TonChainContext) -> color_eyre::eyre::Result<Option<TonAmount>> {
        Ok(Some(
            CustomType::new("Amount to attach (e.g. 0.05 TON, 1000 nanotons):").prompt()?,
        ))
    }

    fn input_body(_context: &TonChainContext) -> color_eyre::eyre::Result<Option<String>> {
        Ok(Some(
            inquire::Text::new("Message body ('comment:<text>', 'boc:<hex>', or 'none'):")
                .with_initial_value("none")
                .prompt()?,
        ))
    }
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
