//! Shared plumbing for the guided (interactive-only) contract-call flows:
//! the CLI looks up the contract's published interface - an EVM ABI, an
//! Anchor IDL, a Move module's exposed functions - lists the callable
//! functions, and prompts for each argument with its declared type.
//!
//! The guidance never changes the command grammar: whatever the user picks
//! is written back as the same textual form the non-interactive command
//! takes (`function-signature 'f(address to)' '["0x.."]'`, `type:value`
//! Move args, a Solana accounts list + hex data), so the echoed command
//! stays reproducible without any lookup, and `proposal review` verifies
//! exactly those bytes.
//!
//! interactive-clap prompts one field at a time and hands each `input_*`
//! only the *previous* context, not the fields already typed for the
//! current struct. A flow that picks a function in one prompt and needs its
//! parameter list in the next therefore parks it in the thread-local
//! [`stash`] between prompts.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;

use color_eyre::owo_colors::OwoColorize;

use crate::config::{ChainDef, NetworkVariant};

thread_local! {
    static STASH: RefCell<HashMap<TypeId, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// Parks a value for a later prompt of the same interactive flow (one slot
/// per type).
pub fn stash<T: Any>(value: T) {
    STASH.with(|stash| {
        stash
            .borrow_mut()
            .insert(TypeId::of::<T>(), Box::new(value));
    });
}

/// Takes the value a previous prompt parked, if any.
pub fn take_stashed<T: Any>() -> Option<T> {
    STASH.with(|stash| {
        stash
            .borrow_mut()
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    })
}

/// Reads a parked value without taking it.
pub fn peek_stashed<T: Any + Clone>() -> Option<T> {
    STASH.with(|stash| {
        stash
            .borrow()
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    })
}

/// The chain's network variants in lookup order: mainnet first (where
/// contracts are most likely deployed), then the rest alphabetically. The
/// NEAR network is only chosen at the `network-config` step, after the
/// call is described, so interface lookups try the variants in turn and
/// use the first one where the contract exists.
pub fn lookup_networks(chain_def: &ChainDef) -> Vec<(String, NetworkVariant)> {
    let mut networks: Vec<(String, NetworkVariant)> = chain_def
        .networks
        .iter()
        .map(|(name, variant)| (name.clone(), variant.clone()))
        .collect();
    networks.sort_by_key(|(name, _)| (name != "mainnet", name.clone()));
    networks
}

/// A progress/explanation line for the lookup, in the prompt gutter and
/// de-emphasized so it reads as context rather than as a prompt.
pub fn note(text: &str) {
    crate::output::info(text.dimmed().to_string());
}

/// The escape hatch every guided select offers.
pub const MANUAL_ENTRY: &str = "(type it manually)";

/// A select over `options` with a trailing manual-entry choice. Returns the
/// index of the chosen option, or `None` for manual entry.
pub fn select_or_manual(
    prompt: &str,
    mut options: Vec<String>,
) -> color_eyre::eyre::Result<Option<usize>> {
    options.push(MANUAL_ENTRY.to_string());
    let manual_index = options.len() - 1;
    let choice = inquire::Select::new(prompt, options)
        .with_page_size(15)
        .raw_prompt()?;
    Ok((choice.index != manual_index).then_some(choice.index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_round_trips_by_type() {
        stash(41u32);
        stash(String::from("x"));
        assert_eq!(peek_stashed::<u32>(), Some(41));
        assert_eq!(take_stashed::<u32>(), Some(41));
        assert_eq!(take_stashed::<u32>(), None);
        assert_eq!(take_stashed::<String>(), Some(String::from("x")));
    }

    #[test]
    fn lookup_networks_puts_mainnet_first() {
        let mut chain = ChainDef {
            family: "evm".into(),
            networks: BTreeMap::default(),
        };
        for name in ["testnet", "mainnet", "devnet"] {
            chain.networks.insert(
                name.into(),
                NetworkVariant {
                    rpc_url: "https://example.invalid".into(),
                    chain_id: None,
                    explorer_tx_url: None,
                    symbol: None,
                    decimals: None,
                },
            );
        }
        let order: Vec<String> = lookup_networks(&chain)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(order, ["mainnet", "devnet", "testnet"]);
    }
}
