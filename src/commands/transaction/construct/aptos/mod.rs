//! `omni transaction construct aptos <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::aptos::{AptosActionSpec, AptosAdapter, rpc};
use crate::chains::move_call::{self, SelectedMoveFunction};
use crate::commands::transaction::construct::{SelectedChain, SpecContext, guided};
use crate::config::ChainDef;
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
        message = "transfer        -   Transfer APT (creates the recipient account if needed)"
    ))]
    /// Transfer APT (creates the recipient account if needed)
    Transfer(Transfer),
    #[strum_discriminants(strum(
        message = "contract-call   -   Call an entry function (<address>::<module>::<function>) with typed args"
    ))]
    /// Call an entry function (<address>::<module>::<function>) with typed args
    ContractCall(ContractCall),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = AptosChainContext)]
#[interactive_clap(output_context = ContractCallContext)]
pub struct ContractCall {
    #[interactive_clap(skip_default_input_arg)]
    /// Entry function (<address>::<module>::<function>, e.g. 0x1::aptos_account::transfer):
    function: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Type arguments: JSON array of Move types, e.g. '["0x1::aptos_coin::AptosCoin"]' ('[]' for none)
    type_args: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Arguments: JSON array of type:value strings, e.g. '["address:0x1", "u64:100"]' ('[]' for none)
    args: String,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct ContractCallContext(SpecContext);

impl ContractCallContext {
    pub fn from_previous_context(
        previous_context: AptosChainContext,
        scope: &<ContractCall as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        use crate::chains::move_call::MoveArg;

        let (address, module, function) = move_call::parse_function_path(&scope.function)?;
        let ty_args = move_call::parse_string_list(&scope.type_args)?
            .iter()
            .map(|ty| move_call::parse_type(ty))
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;
        let mut args = Vec::new();
        let mut arg_displays = Vec::new();
        for text in move_call::parse_string_list(&scope.args)? {
            match move_call::parse_arg(&text)? {
                MoveArg::Pure { bytes, display } => {
                    args.push(bytes);
                    arg_displays.push(display);
                }
                MoveArg::Object { .. } => {
                    return Err(color_eyre::eyre::eyre!(
                        "'{text}': object arguments are a Sui concept; Aptos entry functions \
                         take addresses (address:0x...)"
                    ));
                }
            }
        }
        let generics = if scope.type_args.trim().is_empty() || ty_args.is_empty() {
            String::new()
        } else {
            format!(
                "<{}>",
                move_call::parse_string_list(&scope.type_args)?.join(", ")
            )
        };
        let spec = AptosActionSpec::EntryFunction {
            module_address: omni_transaction::aptos::types::AccountAddress(address),
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
            adapter: Arc::new(AptosAdapter { spec }),
        }))
    }
}

impl From<ContractCallContext> for SpecContext {
    fn from(item: ContractCallContext) -> Self {
        item.0
    }
}

impl ContractCall {
    /// Guided: the module interface is fetched from the fullnode, entry
    /// functions are listed, and the chosen one's parameters drive the next
    /// two prompts. Typing a full path skips the browsing (the interface is
    /// still looked up for the prompts); lookup failures fall back to plain
    /// text entry.
    fn input_function(context: &AptosChainContext) -> color_eyre::eyre::Result<Option<String>> {
        let text = inquire::Text::new(
            "Function (0xaddr::module::function), or 0xaddr::module / 0xaddr to browse:",
        )
        .prompt()?;
        match browse(&context.selected.chain_def, text.trim()) {
            Ok(Browsed::Selected(function)) => {
                let path = function.path.clone();
                guided::stash(function);
                Ok(Some(path))
            }
            Ok(Browsed::Manual) => Ok(Some(
                inquire::Text::new("Entry function (<address>::<module>::<function>):").prompt()?,
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
                        inquire::Text::new("Entry function (<address>::<module>::<function>):")
                            .prompt()?,
                    ))
                }
            }
        }
    }

    fn input_type_args(_context: &AptosChainContext) -> color_eyre::eyre::Result<Option<String>> {
        if let Some(function) = guided::peek_stashed::<SelectedMoveFunction>() {
            return Ok(Some(move_call::prompt_type_args(
                &function,
                "0x1::aptos_coin::AptosCoin",
            )?));
        }
        Ok(Some(
            inquire::Text::new("Type arguments (JSON array of Move types; [] for none):")
                .with_initial_value("[]")
                .prompt()?,
        ))
    }

    fn input_args(_context: &AptosChainContext) -> color_eyre::eyre::Result<Option<String>> {
        if let Some(function) = guided::take_stashed::<SelectedMoveFunction>() {
            return Ok(Some(move_call::prompt_args(&function)?));
        }
        Ok(Some(
            inquire::Text::new(
                "Arguments (JSON array of type:value, e.g. [\"address:0x1\", \"u64:100\"]):",
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

/// Resolves what the user typed (`0xaddr`, `0xaddr::module`, or a full
/// function path) against the first configured network where the account
/// publishes modules.
fn browse(chain_def: &ChainDef, text: &str) -> color_eyre::eyre::Result<Browsed> {
    let parts: Vec<&str> = text.split("::").collect();
    let (address, module, function) = match parts.as_slice() {
        [address] => (*address, None, None),
        [address, module] => (*address, Some(*module), None),
        [address, module, function] => (*address, Some(*module), Some(*function)),
        _ => {
            return Err(color_eyre::eyre::eyre!(
                "expected <address>, <address>::<module>, or <address>::<module>::<function>"
            ));
        }
    };
    let address = format!("0x{}", hex::encode(move_call::parse_address(address)?))
        .trim_start_matches("0x0")
        .to_string();
    let address = if address.starts_with("0x") {
        address
    } else {
        format!("0x{address}")
    };

    let mut last_error = None;
    for (network, variant) in guided::lookup_networks(chain_def) {
        let rpc = rpc::Client::new(&variant.rpc_url)?;
        let found = match module {
            Some(module) => rpc
                .module_abi(&address, module)
                .map(|abi| abi.map(|abi| vec![abi])),
            None => rpc
                .modules(&address)
                .map(|abis| (!abis.is_empty()).then_some(abis)),
        };
        match found {
            Ok(Some(modules)) => {
                guided::note(&format!(
                    "Loaded the module interface from {} ({network}).",
                    variant.rpc_url
                ));
                return pick(&address, modules, function);
            }
            Ok(None) => {
                guided::note(&format!("No such module at {address} on {network}."));
            }
            Err(err) => {
                guided::note(&format!("{network}: {err}"));
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        color_eyre::eyre::eyre!("{address} publishes no such module on any configured network")
    }))
}

fn pick(
    address: &str,
    mut modules: Vec<crate::chains::aptos::abi::ModuleAbi>,
    function: Option<&str>,
) -> color_eyre::eyre::Result<Browsed> {
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let module = if modules.len() == 1 {
        modules.remove(0)
    } else {
        let names: Vec<String> = modules.iter().map(|module| module.name.clone()).collect();
        match guided::select_or_manual("Which module?", names)? {
            Some(index) => modules.remove(index),
            None => return Ok(Browsed::Manual),
        }
    };
    let entries: Vec<&crate::chains::aptos::abi::ExposedFunction> =
        module.entry_functions().collect();
    if let Some(name) = function {
        return match entries.iter().find(|entry| entry.name == name) {
            Some(entry) => Ok(Browsed::Selected(entry.selected(address, &module.name))),
            None => Err(color_eyre::eyre::eyre!(
                "{address}::{}::{name} is not an entry function (entry functions: {})",
                module.name,
                entries
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    if entries.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "{address}::{} has no entry functions (nothing a transaction can call)",
            module.name
        ));
    }
    let labels: Vec<String> = entries
        .iter()
        .map(|entry| {
            move_call::render_signature(
                &entry.name,
                entry.generic_type_params.len(),
                &entry.guided_params(),
            )
        })
        .collect();
    match guided::select_or_manual("Which entry function?", labels)? {
        Some(index) => Ok(Browsed::Selected(
            entries[index].selected(address, &module.name),
        )),
        None => Ok(Browsed::Manual),
    }
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
