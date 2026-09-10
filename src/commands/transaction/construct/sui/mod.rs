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
    #[strum_discriminants(strum(message = "transfer    -   Transfer SUI"))]
    /// Transfer SUI
    Transfer(Transfer),
    #[strum_discriminants(strum(
        message = "move-call   -   Call a Move function (<package>::<module>::<function>) with typed args"
    ))]
    /// Call a Move function (<package>::<module>::<function>) with typed args
    MoveCall(MoveCall),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = SuiChainContext)]
#[interactive_clap(output_context = MoveCallContext)]
pub struct MoveCall {
    /// Move function (<package>::<module>::<function>, e.g. 0x2::coin::zero):
    function: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Type arguments: JSON array of Move types, e.g. '["0x2::sui::SUI"]' ('[]' for none)
    type_args: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Arguments: JSON array of type:value strings, e.g. '["object:0x...", "u64:100", "address:0x..."]'
    args: String,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct MoveCallContext(SpecContext);

impl MoveCallContext {
    pub fn from_previous_context(
        previous_context: SuiChainContext,
        scope: &<MoveCall as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        use crate::chains::move_call;

        let (package, module, function) = move_call::parse_function_path(&scope.function)?;
        let type_arg_texts = move_call::parse_string_list(&scope.type_args)?;
        let ty_args = type_arg_texts
            .iter()
            .map(|ty| move_call::parse_type(ty))
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
        let args = move_call::parse_string_list(&scope.args)?
            .iter()
            .map(|arg| move_call::parse_arg(arg))
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
        let arg_displays: Vec<String> = args
            .iter()
            .map(|arg| match arg {
                move_call::MoveArg::Pure { display, .. }
                | move_call::MoveArg::Object { display, .. } => display.clone(),
            })
            .collect();
        let generics = if type_arg_texts.is_empty() {
            String::new()
        } else {
            format!("<{}>", type_arg_texts.join(", "))
        };
        let spec = SuiActionSpec::MoveCall {
            package: omni_transaction::sui::types::SuiAddress(package),
            module,
            function,
            ty_args,
            args,
            summary: format!(
                "call {}{generics}({})",
                scope.function.trim(),
                arg_displays.join(", ")
            ),
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

impl From<MoveCallContext> for SpecContext {
    fn from(item: MoveCallContext) -> Self {
        item.0
    }
}

impl MoveCall {
    fn input_type_args(_context: &SuiChainContext) -> color_eyre::eyre::Result<Option<String>> {
        Ok(Some(
            inquire::Text::new("Type arguments (JSON array of Move types; [] for none):")
                .with_initial_value("[]")
                .prompt()?,
        ))
    }

    fn input_args(_context: &SuiChainContext) -> color_eyre::eyre::Result<Option<String>> {
        Ok(Some(
            inquire::Text::new(
                "Arguments (JSON array of type:value, e.g. [\"object:0x...\", \"u64:100\"]):",
            )
            .with_initial_value("[]")
            .prompt()?,
        ))
    }
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
            to: omni_transaction::sui::types::SuiAddress(scope.receiver.0),
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
