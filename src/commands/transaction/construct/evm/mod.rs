//! `omni construct evm <chain> <action> ... derivation-path <path> sign-as-...`

use std::sync::Arc;

use alloy_dyn_abi::Specifier;
use inquire::CustomType;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

use crate::chains::evm::{EvmActionSpec, EvmAdapter, abi, abi_source};
use crate::commands::transaction::construct::guided;
use crate::commands::transaction::construct::{SelectedChain, SpecContext};
use crate::types::eth_address::EthAddress;
use crate::types::eth_amount::EthAmount;
use crate::types::hex_bytes::HexBytes;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = EvmChainContext)]
pub struct EvmChain {
    #[interactive_clap(skip_default_input_arg)]
    /// Which EVM chain? (from the omni chain registry)
    chain: String,
    #[interactive_clap(subcommand)]
    action: EvmAction,
}

#[derive(Clone)]
pub struct EvmChainContext {
    pub global_context: near_cli_rs::GlobalContext,
    pub selected: Arc<SelectedChain>,
}

impl EvmChainContext {
    pub fn from_previous_context(
        previous_context: near_cli_rs::GlobalContext,
        scope: &<EvmChain as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            global_context: previous_context,
            selected: Arc::new(crate::commands::transaction::construct::load_chain(
                crate::chains::evm::FAMILY,
                &scope.chain,
            )?),
        })
    }
}

impl EvmChain {
    fn input_chain(
        _context: &near_cli_rs::GlobalContext,
    ) -> color_eyre::eyre::Result<Option<String>> {
        crate::commands::transaction::construct::input_chain(crate::chains::evm::FAMILY)
    }
}

impl EvmChainContext {
    fn into_spec_context(self, spec: EvmActionSpec) -> SpecContext {
        SpecContext {
            global_context: self.global_context,
            chain_key: self.selected.chain_key.clone(),
            chain_def: self.selected.chain_def.clone(),
            mpc_config: self.selected.mpc_config.clone(),
            adapter: Arc::new(EvmAdapter { spec }),
        }
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = EvmChainContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum EvmAction {
    #[strum_discriminants(strum(
        message = "transfer       -   Transfer the native token (ETH, ...)"
    ))]
    /// Transfer the native token (ETH, ...)
    Transfer(Transfer),
    #[strum_discriminants(strum(
        message = "contract-call  -   Call a contract function (typed signature or raw calldata)"
    ))]
    /// Call a contract function (typed signature or raw calldata)
    ContractCall(ContractCall),
    #[strum_discriminants(strum(
        message = "raw            -   Fully custom transaction: recipient, value, and raw calldata"
    ))]
    /// Fully custom transaction: recipient, value, and raw calldata
    Raw(Raw),
}

// ---------------------------------------------------------------- transfer

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = EvmChainContext)]
#[interactive_clap(output_context = TransferContext)]
pub struct Transfer {
    /// Recipient address (0x...):
    receiver: EthAddress,
    #[interactive_clap(skip_default_input_arg)]
    /// Amount to transfer (e.g. 0.5 ETH):
    amount: EthAmount,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct TransferContext(SpecContext);

impl TransferContext {
    pub fn from_previous_context(
        previous_context: EvmChainContext,
        scope: &<Transfer as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = EvmActionSpec {
            to: scope.receiver.as_bytes(),
            value_wei: scope.amount.wei,
            data: Vec::new(),
            summary: format!("transfer {} to {}", scope.amount, scope.receiver),
        };
        Ok(Self(previous_context.into_spec_context(spec)))
    }
}

impl From<TransferContext> for SpecContext {
    fn from(item: TransferContext) -> Self {
        item.0
    }
}

impl Transfer {
    fn input_amount(_context: &EvmChainContext) -> color_eyre::eyre::Result<Option<EthAmount>> {
        Ok(Some(
            CustomType::new("Amount to transfer (e.g. 0.5 ETH, 2 gwei, 1000 wei):").prompt()?,
        ))
    }
}

// ------------------------------------------------------------ contract-call

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = EvmChainContext)]
#[interactive_clap(output_context = ContractCallContext)]
pub struct ContractCall {
    /// Contract address (0x...):
    contract: EthAddress,
    #[interactive_clap(skip_default_input_arg)]
    /// Attached native value (usually 0 ETH):
    attached_value: EthAmount,
    #[interactive_clap(subcommand)]
    calldata: Calldata,
}

#[derive(Clone)]
pub struct ContractCallContext {
    chain_context: EvmChainContext,
    contract: EthAddress,
    attached_value: EthAmount,
}

impl ContractCallContext {
    pub fn from_previous_context(
        previous_context: EvmChainContext,
        scope: &<ContractCall as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            chain_context: previous_context,
            contract: scope.contract,
            attached_value: scope.attached_value,
        })
    }

    fn into_spec_context(self, calldata: Vec<u8>, call_description: &str) -> SpecContext {
        let summary = if self.attached_value.wei == 0 {
            format!("{call_description} on {}", self.contract)
        } else {
            format!(
                "{call_description} on {} with {}",
                self.contract, self.attached_value
            )
        };
        let spec = EvmActionSpec {
            to: self.contract.as_bytes(),
            value_wei: self.attached_value.wei,
            data: calldata,
            summary,
        };
        self.chain_context.into_spec_context(spec)
    }
}

impl ContractCall {
    fn input_attached_value(
        _context: &EvmChainContext,
    ) -> color_eyre::eyre::Result<Option<EthAmount>> {
        Ok(Some(
            CustomType::new("Attached native value (usually 0 ETH):")
                .with_starting_input("0 ETH")
                .prompt()?,
        ))
    }
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = ContractCallContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// How do you want to provide the calldata?
pub enum Calldata {
    #[strum_discriminants(strum(
        message = "function-signature  -   Pick a function from the contract's verified ABI (or type a signature) and fill in its arguments"
    ))]
    /// Pick a function from the contract's verified ABI (or type a signature) and fill in its arguments
    FunctionSignature(FunctionSignature),
    #[strum_discriminants(strum(
        message = "raw-calldata        -   Paste pre-encoded calldata hex (e.g. from cast/foundry)"
    ))]
    /// Paste pre-encoded calldata hex (e.g. from cast/foundry)
    RawCalldata(RawCalldata),
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = ContractCallContext)]
#[interactive_clap(output_context = FunctionSignatureContext)]
pub struct FunctionSignature {
    #[interactive_clap(skip_default_input_arg)]
    /// Function signature, parameter names optional (e.g. transfer(address to, uint256 amount) or pause()):
    signature: String,
    #[interactive_clap(skip_default_input_arg)]
    /// Function arguments as a JSON array (e.g. ["0xabc...", "1000"]; [] for none):
    args: String,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct FunctionSignatureContext(SpecContext);

impl FunctionSignatureContext {
    pub fn from_previous_context(
        previous_context: ContractCallContext,
        scope: &<FunctionSignature as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let (calldata, canonical_signature) =
            crate::chains::evm::abi::encode_calldata(&scope.signature, &scope.args)?;
        let call_description = format!("call {canonical_signature}");
        Ok(Self(
            previous_context.into_spec_context(calldata, &call_description),
        ))
    }
}

impl From<FunctionSignatureContext> for SpecContext {
    fn from(item: FunctionSignatureContext) -> Self {
        item.0
    }
}

/// The function picked from the ABI, parked between the signature prompt
/// and the per-argument prompts of the same flow.
#[derive(Clone)]
struct SelectedFunction(alloy_json_abi::Function);

impl FunctionSignature {
    fn input_signature(context: &ContractCallContext) -> color_eyre::eyre::Result<Option<String>> {
        let contract = context.contract.to_string();
        guided::note(&format!(
            "Looking up the interface of {contract} on {} ...",
            context.chain_context.selected.chain_key
        ));
        let fetched =
            match abi_source::fetch_abi(&context.chain_context.selected.chain_def, &contract) {
                Ok(fetched) => fetched,
                Err(err) => {
                    guided::note(&format!("  Interface lookup failed: {err}"));
                    None
                }
            };
        if let Some(fetched) = fetched {
            let implementation = fetched
                .implementation
                .as_ref()
                .map(|address| format!(" (proxy; functions from implementation {address})"))
                .unwrap_or_default();
            guided::note(&format!(
                "  Found on {} via {}{implementation}.",
                fetched.network, fetched.source
            ));
            let functions = abi_source::callable_functions(&fetched.abi);
            if functions.is_empty() {
                guided::note("  The ABI has no state-changing functions.");
            } else {
                let options: Vec<String> = functions
                    .iter()
                    .map(|function| {
                        let payable = matches!(
                            function.state_mutability,
                            alloy_json_abi::StateMutability::Payable
                        );
                        format!(
                            "{}{}",
                            abi::human_signature(function),
                            if payable { " [payable]" } else { "" }
                        )
                    })
                    .collect();
                if let Some(index) = guided::select_or_manual("Which function?", options)? {
                    let function = functions[index].clone();
                    if !matches!(
                        function.state_mutability,
                        alloy_json_abi::StateMutability::Payable
                    ) && context.attached_value.wei != 0
                    {
                        crate::output::warn(format!(
                            "{} is not payable, but {} is attached - the call will revert.",
                            function.name, context.attached_value
                        ));
                    }
                    let signature = abi::human_signature(&function);
                    guided::stash(SelectedFunction(function));
                    return Ok(Some(signature));
                }
            }
        } else {
            guided::note(
                "  No verified ABI found (Sourcify; Etherscan with an API key) - type the signature.",
            );
        }
        Ok(Some(
            inquire::Text::new(
                "Function signature (parameter names optional, e.g. transfer(address to, uint256 amount) or pause()):",
            )
            .prompt()?,
        ))
    }

    fn input_args(_context: &ContractCallContext) -> color_eyre::eyre::Result<Option<String>> {
        let function = guided::take_stashed::<SelectedFunction>().map(|selected| selected.0);
        if let Some(function) = function {
            return prompt_arguments(&function).map(Some);
        }
        let args = inquire::Text::new(
            "Function arguments as a JSON array (e.g. [\"0xabc...\", \"1000\"]):",
        )
        .with_initial_value("[]")
        .prompt()?;
        Ok(Some(args))
    }
}

/// One prompt per parameter, validated against its Solidity type; the
/// answers become the JSON array the command line takes.
fn prompt_arguments(function: &alloy_json_abi::Function) -> color_eyre::eyre::Result<String> {
    if function.inputs.is_empty() {
        return Ok("[]".to_string());
    }
    let mut values = Vec::with_capacity(function.inputs.len());
    for (index, param) in function.inputs.iter().enumerate() {
        let ty = param.resolve().map_err(|err| {
            color_eyre::eyre::eyre!(
                "Cannot resolve the type of parameter `{}`: {err}",
                param.selector_type()
            )
        })?;
        let label = if param.name.is_empty() {
            format!("Argument #{index} ({}):", param.selector_type())
        } else {
            format!("{} ({}):", param.name, param.selector_type())
        };
        let validator_type = ty.clone();
        let value = inquire::Text::new(&label)
            .with_help_message(type_hint(&ty))
            .with_validator(move |input: &str| {
                Ok(match validator_type.coerce_str(input) {
                    Ok(_) => inquire::validator::Validation::Valid,
                    Err(err) => inquire::validator::Validation::Invalid(
                        format!("not a valid {validator_type}: {err}").into(),
                    ),
                })
            })
            .prompt()?;
        values.push(serde_json::Value::String(value));
    }
    Ok(serde_json::Value::Array(values).to_string())
}

/// What to type for a Solidity type.
fn type_hint(ty: &alloy_dyn_abi::DynSolType) -> &'static str {
    use alloy_dyn_abi::DynSolType;
    match ty {
        DynSolType::Address => "0x-prefixed 20-byte address",
        DynSolType::Bool => "true or false",
        DynSolType::Uint(_) => "decimal integer, e.g. 1000000 (wei-level units, no decimals)",
        DynSolType::Int(_) => "decimal integer, negative allowed",
        DynSolType::Bytes | DynSolType::FixedBytes(_) => "0x-prefixed hex bytes",
        DynSolType::String => "plain text",
        DynSolType::Array(_) | DynSolType::FixedArray(..) => {
            "array like [1, 2, 3] or [0xabc..., 0xdef...]"
        }
        DynSolType::Tuple(_) => "tuple like (0xabc..., 1000, true)",
        DynSolType::Function => "24-byte function selector (0x-hex)",
    }
}

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = ContractCallContext)]
#[interactive_clap(output_context = RawCalldataContext)]
pub struct RawCalldata {
    /// Calldata hex (0x...):
    calldata: HexBytes,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct RawCalldataContext(SpecContext);

impl RawCalldataContext {
    pub fn from_previous_context(
        previous_context: ContractCallContext,
        scope: &<RawCalldata as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let call_description = if scope.calldata.0.len() >= 4 {
            format!(
                "call with raw calldata (selector 0x{})",
                hex::encode(&scope.calldata.0[..4])
            )
        } else {
            "call with raw calldata".to_string()
        };
        Ok(Self(previous_context.into_spec_context(
            scope.calldata.0.clone(),
            &call_description,
        )))
    }
}

impl From<RawCalldataContext> for SpecContext {
    fn from(item: RawCalldataContext) -> Self {
        item.0
    }
}

// ---------------------------------------------------------------------- raw

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = EvmChainContext)]
#[interactive_clap(output_context = RawContext)]
pub struct Raw {
    /// Recipient address (0x...):
    to: EthAddress,
    #[interactive_clap(skip_default_input_arg)]
    /// Attached native value (e.g. 0 ETH):
    value: EthAmount,
    /// Transaction data hex (0x...; empty for none):
    data: HexBytes,
    #[interactive_clap(named_arg)]
    /// Derivation path - determines the acting foreign account
    derivation_path: crate::commands::transaction::construct::sign_as::DerivationPath,
}

#[derive(Clone)]
pub struct RawContext(SpecContext);

impl RawContext {
    pub fn from_previous_context(
        previous_context: EvmChainContext,
        scope: &<Raw as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let spec = EvmActionSpec {
            to: scope.to.as_bytes(),
            value_wei: scope.value.wei,
            data: scope.data.0.clone(),
            summary: format!(
                "raw transaction to {}: value {}, {} bytes of data",
                scope.to,
                scope.value,
                scope.data.0.len()
            ),
        };
        Ok(Self(previous_context.into_spec_context(spec)))
    }
}

impl From<RawContext> for SpecContext {
    fn from(item: RawContext) -> Self {
        item.0
    }
}

impl Raw {
    fn input_value(_context: &EvmChainContext) -> color_eyre::eyre::Result<Option<EthAmount>> {
        Ok(Some(
            CustomType::new("Attached native value (e.g. 0 ETH):")
                .with_starting_input("0 ETH")
                .prompt()?,
        ))
    }
}
