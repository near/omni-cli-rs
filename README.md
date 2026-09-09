# omni-cli-rs

`omni` is your human-friendly companion for controlling accounts on other
chains from NEAR — via a single NEAR DAO (SputnikDAO) or a plain NEAR account,
using [MPC chain signatures](https://docs.near.org/chain-abstraction/chain-signatures)
and [omni-transaction-rs](https://github.com/Near-One/omni-transaction-rs)
transaction builders.

Instead of maintaining a separate multisig on every chain (Gnosis Safe, Squads,
Petra Vault, xDAO, ...), one DAO on NEAR approves proposals that ask the MPC to
sign chain-specific payloads; anyone then broadcasts the signed transaction on
the destination chain.

Built as a [near-cli-rs](https://github.com/near/near-cli-rs) extension: every
step can be answered interactively, and the CLI echoes the full non-interactive
command at the end so it can be scripted and shared with other DAO members.

See [DESIGN.md](DESIGN.md) for the full architecture.

## Status

The `construct` command with both execution routes, built on a family-generic
`ChainAdapter` (chain-signatures v2 interface: key domains, `payload_v2`).

- [x] `construct evm <chain>` — `transfer` / `contract-call` / `raw`
      (secp256k1, domain 0; defaults: eth, base, arb, bnb, pol, hyperevm, abs)
- [x] `construct svm <chain>` — `transfer` + `setup-nonce` (Solana/Fogo;
      ed25519, domain 1). The DAO route uses a durable nonce account: create
      the deterministic one with `setup-nonce` (account-owned paths) or pass
      an externally created one via `--nonce-account` (DAO-owned paths)
- [x] `construct aptos <chain>` — `transfer` (ed25519; DAO route supported —
      expiration is set 14 days out)
- [x] `construct sui <chain>` — `transfer` (ed25519; DAO route supported —
      no expiry, but gas-coin references go stale if the coins are touched)
- [x] `construct utxo btc` — `transfer` (P2WPKH; one MPC signature per input,
      matched to inputs by verification; change returns to the sender;
      Esplora API for UTXOs/fees/broadcast)
- [x] `construct ton <chain>` — `transfer` (v5r1 wallet; ed25519 over the
      body cell hash; the wallet deploys itself with its first transaction)
- [x] `sign-as-account` — your account calls the MPC directly; the CLI
      extracts the signature from the receipts, assembles, and broadcasts
- [x] `sign-as-dao` — wraps the sign request in a SputnikDAO proposal with a
      reviewable envelope in the description
- [x] `transaction broadcast <near-tx-hash> <tx-signer>` — finalizes an approved
      DAO proposal (envelope recovered from the proposal) or retries a failed
      direct-route broadcast (`--unsigned-tx` envelope blob)
- [x] `account show / balance` — derived addresses across all registered
      chains, and native balances per chain
- [ ] Zcash (transparent) — needs an indexer choice; the builders exist
- [ ] `proposal list / review / vote`

## Usage

```console
# Interactive - just follow the prompts:
omni transaction construct

# Direct: your account owns the derived foreign account (sub-minute end to end)
omni transaction construct evm eth \
    transfer 0x000000000000000000000000000000000000dEaD '0.001 ETH' \
    derivation-path my-treasury \
    sign-as-account you.testnet \
    network-config testnet sign-with-keychain send
# (with network-config testnet, `eth` resolves to Sepolia automatically)

# Solana works the same way (ed25519 key domain):
omni transaction construct svm solana \
    transfer 4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi '0.1 SOL' \
    derivation-path my-treasury \
    sign-as-account you.testnet \
    network-config testnet sign-with-keychain send

# DAO: wrap the sign request in a SputnikDAO proposal
omni transaction construct evm base \
    contract-call 0xYourLocker '0 ETH' function-signature 'pause()' '[]' \
    derivation-path base-locker-admin \
    sign-as-dao bridge-dao.sputnik-dao.near 'Pause Base locker during incident #42' \
    proposer.near \
    network-config mainnet sign-with-keychain send
```

The derived foreign address is a function of *(owner account, derivation
path)*: `dao.near + "treasury"` and `alice.near + "treasury"` are unrelated
addresses on every chain. The CLI always prints the owner and path alongside
the derived address.

## Configuration

On first run a chain registry is created next to the near-cli-rs config
(`omni-config.toml`). Chains are logical — you pick `base` or `solana`, and
the concrete endpoint/chain id resolves from the NEAR network selected later
at the `network-config` step (NEAR mainnet → the chain's mainnet, NEAR
testnet → its testnet):

```toml
[chains.base]
family = "evm"
[chains.base.networks.mainnet]
rpc_url = "https://mainnet.base.org"
chain_id = 8453
explorer_tx_url = "https://basescan.org/tx/"
[chains.base.networks.testnet]
rpc_url = "https://sepolia.base.org"
chain_id = 84532
explorer_tx_url = "https://sepolia.basescan.org/tx/"
```

Adding one more chain is a config entry, not a new release. The same file
also holds `default_derivation_path` (pre-filled in interactive prompts,
`omni-1` out of the box) and the `[mpc]` signer settings.

## Build

```console
cargo build --release
./target/release/omni
```
