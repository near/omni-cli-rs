//! `omni transaction construct sui <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use std::collections::BTreeMap;

use crate::chains::move_call::{self, SelectedMoveFunction};
use crate::chains::sui::abi::{NormalizedFunction, NormalizedModule};
use crate::chains::sui::{SuiActionSpec, SuiAdapter, rpc};
use crate::commands::transaction::construct::{SelectedChain, SpecContext, guided};
use crate::config::ChainDef;
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
    #[interactive_clap(skip_default_input_arg)]
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
    /// Guided: the package's normalized modules are fetched from the
    /// fullnode, callable functions are listed, and the chosen one's
    /// parameters drive the next two prompts. Typing a full path skips the
    /// browsing (the interface is still looked up for the prompts); lookup
    /// failures fall back to plain text entry.
    fn input_function(context: &SuiChainContext) -> color_eyre::eyre::Result<Option<String>> {
        let text = inquire::Text::new(
            "Function (0xpkg::module::function), or 0xpkg::module / 0xpkg to browse:",
        )
        .prompt()?;
        match browse(&context.selected.chain_def, text.trim()) {
            Ok(Browsed::Selected(function)) => {
                let path = function.path.clone();
                guided::stash(function);
                Ok(Some(path))
            }
            Ok(Browsed::Manual) => Ok(Some(
                inquire::Text::new("Move function (<package>::<module>::<function>):").prompt()?,
            )),
            Err(err) => {
                guided::note(&format!(
                    "Could not load the module interface ({err}); type arguments and \
                     arguments are entered as JSON."
                ));
                if text.trim().split("::").count() == 3 {
                    Ok(Some(text.trim().to_string()))
                } else {
                    Ok(Some(
                        inquire::Text::new("Move function (<package>::<module>::<function>):")
                            .prompt()?,
                    ))
                }
            }
        }
    }

    fn input_type_args(_context: &SuiChainContext) -> color_eyre::eyre::Result<Option<String>> {
        if let Some(function) = guided::peek_stashed::<SelectedMoveFunction>() {
            return Ok(Some(move_call::prompt_type_args(
                &function,
                "0x2::sui::SUI",
            )?));
        }
        Ok(Some(
            inquire::Text::new("Type arguments (JSON array of Move types; [] for none):")
                .with_initial_value("[]")
                .prompt()?,
        ))
    }

    fn input_args(_context: &SuiChainContext) -> color_eyre::eyre::Result<Option<String>> {
        if let Some(function) = guided::take_stashed::<SelectedMoveFunction>() {
            return Ok(Some(move_call::prompt_args(&function)?));
        }
        Ok(Some(
            inquire::Text::new(
                "Arguments (JSON array of type:value, e.g. [\"object:0x...\", \"u64:100\"]):",
            )
            .with_initial_value("[]")
            .prompt()?,
        ))
    }
}

enum Browsed {
    Selected(SelectedMoveFunction),
    /// The user chose manual entry from a list.
    Manual,
}

/// Resolves what the user typed (`0xpkg`, `0xpkg::module`, or a full
/// function path) against the first configured network where the package
/// exists.
fn browse(chain_def: &ChainDef, text: &str) -> color_eyre::eyre::Result<Browsed> {
    let parts: Vec<&str> = text.split("::").collect();
    let (package, module, function) = match parts.as_slice() {
        [package] => (*package, None, None),
        [package, module] => (*package, Some(*module), None),
        [package, module, function] => (*package, Some(*module), Some(*function)),
        _ => {
            return Err(color_eyre::eyre::eyre!(
                "expected <package>, <package>::<module>, or <package>::<module>::<function>"
            ));
        }
    };
    // Sui wants the full 32-byte form.
    let package = format!("0x{}", hex::encode(move_call::parse_address(package)?));

    let mut last_error = None;
    for (network, variant) in guided::lookup_networks(chain_def) {
        let rpc = rpc::Client::new(&variant.rpc_url)?;
        let found = match module {
            Some(module) => rpc
                .normalized_module(&package, module)
                .map(|found| found.map(|found| BTreeMap::from([(module.to_string(), found)]))),
            None => rpc
                .normalized_modules(&package)
                .map(|modules| (!modules.is_empty()).then_some(modules)),
        };
        match found {
            Ok(Some(modules)) => {
                guided::note(&format!(
                    "Loaded the module interface from {} ({network}).",
                    variant.rpc_url
                ));
                return pick(&package, modules, function);
            }
            Ok(None) => {
                guided::note(&format!(
                    "No such package/module at {package} on {network}."
                ));
            }
            Err(err) => {
                guided::note(&format!("{network}: {err}"));
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        color_eyre::eyre::eyre!("{package} has no such module on any configured network")
    }))
}

fn pick(
    package: &str,
    mut modules: BTreeMap<String, NormalizedModule>,
    function: Option<&str>,
) -> color_eyre::eyre::Result<Browsed> {
    let (module_name, module) = if modules.len() == 1 {
        modules.pop_first().expect("one module")
    } else {
        let names: Vec<String> = modules.keys().cloned().collect();
        match guided::select_or_manual("Which module?", names.clone())? {
            Some(index) => {
                let name = names[index].clone();
                let module = modules.remove(&name).expect("selected from keys");
                (name, module)
            }
            None => return Ok(Browsed::Manual),
        }
    };
    let callable: Vec<(&String, &NormalizedFunction)> = module.callable_functions().collect();
    if let Some(name) = function {
        return match callable.iter().find(|(found, _)| found.as_str() == name) {
            Some((_, found)) => Ok(Browsed::Selected(found.selected(
                package,
                &module_name,
                name,
            ))),
            None => Err(color_eyre::eyre::eyre!(
                "{package}::{module_name}::{name} is not a public or entry function (callable: {})",
                callable
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    if callable.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "{package}::{module_name} exposes no public or entry functions"
        ));
    }
    let labels: Vec<String> = callable
        .iter()
        .map(|(name, found)| {
            let signature = move_call::render_signature(
                name,
                found.type_parameters.len(),
                &found.guided_params(),
            );
            if found.is_entry {
                format!("{signature}  [entry]")
            } else {
                signature
            }
        })
        .collect();
    match guided::select_or_manual("Which function?", labels)? {
        Some(index) => {
            let (name, found) = callable[index];
            Ok(Browsed::Selected(found.selected(
                package,
                &module_name,
                name,
            )))
        }
        None => Ok(Browsed::Manual),
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
