# omni-cli-rs

`omni` controls accounts on other chains from NEAR via a single NEAR DAO
(SputnikDAO) or a plain NEAR account using
[MPC chain signatures](https://docs.near.org/chain-abstraction/chain-signatures).

Instead of maintaining a separate multisig on every chain (Gnosis Safe, Squads,
Petra Vault, xDAO, ...), one DAO on NEAR approves proposals that ask the MPC to
sign chain-specific payloads; anyone then broadcasts the signed transaction on
the destination chain.

Built as a [near-cli-rs](https://github.com/near/near-cli-rs) extension: every
step can be answered interactively, and the CLI echoes the full non-interactive
command at the end so it can be scripted and shared with other DAO members.

## Install

Prebuilt binaries for macOS, Linux, and Windows are attached to every
[GitHub release](https://github.com/near/omni-cli-rs/releases).

macOS / Linux:

```console
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/near/omni-cli-rs/releases/latest/download/omni-cli-rs-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -c "irm https://github.com/near/omni-cli-rs/releases/latest/download/omni-cli-rs-installer.ps1 | iex"
```

Upgrade later with `omni self-update`. `omni` uses your existing near-cli-rs
network connections and keychain, so if `near` already works, `omni` does too.

## Quick start

```console
# Interactive - just follow the prompts:
omni

# Which foreign addresses does my account (or DAO) control?
omni account show you.testnet omni-1 network-config testnet

# Direct route: your NEAR account owns the derived foreign account
omni transaction construct evm eth \
    transfer 0x000000000000000000000000000000000000dEaD '0.001 ETH' \
    derivation-path omni-1 \
    sign-as-account you.testnet \
    network-config testnet sign-with-keychain send
# (with network-config testnet, `eth` resolves to Sepolia automatically)

# Solana works the same way:
omni transaction construct svm solana \
    transfer 4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi '0.1 SOL' \
    derivation-path omni-1 \
    sign-as-account you.testnet \
    network-config testnet sign-with-keychain send
```

The derived foreign address is a function of *(owner account, derivation
path)*: `dao.near + "treasury"` and `alice.near + "treasury"` are unrelated
addresses on every chain. The CLI always prints the owner and path alongside
the derived address. Fund the derived address before sending from it.

## The DAO route

For a DAO-controlled account the sign request becomes a SputnikDAO proposal.
The proposal description carries a readable JSON envelope with the exact
unsigned transaction, so reviewers verify what will be signed — not a hash.

```console
# 1. Propose (any member)
omni transaction construct evm base \
    contract-call 0xYourLocker '0 ETH' function-signature 'pause()' '[]' \
    derivation-path base-locker-admin \
    sign-as-dao bridge-dao.sputnik-dao.near 'Pause Base locker during incident #42' \
    proposer.near \
    network-config mainnet sign-with-keychain send

# 2. Review (every voter, before voting) - recomputes the signing payloads
#    from the envelope and byte-compares them against what the DAO would
#    actually ask the MPC to sign; refuses to recommend anything that differs
omni proposal review bridge-dao.sputnik-dao.near 42 network-config mainnet

# 3. Vote
omni proposal vote bridge-dao.sputnik-dao.near 42 approve voter.near \
    network-config mainnet sign-with-keychain send

# 4. Broadcast - the deciding vote prints this command with the right hash
omni transaction broadcast <NEAR-TX-HASH> voter.near network-config mainnet
```

`omni proposal list <dao>` shows recent proposals with omni envelopes decoded.

## Supported chains

| Family  | Default chains                          | Actions                          | Notes |
|---------|-----------------------------------------|----------------------------------|-------|
| `evm`   | eth, base, arb, bnb, pol, hyperevm, abs | `transfer`, `contract-call`, `raw` | EIP-1559; `contract-call` takes a cast-style function signature + JSON args |
| `svm`   | solana, fogo                            | `transfer`, `setup-nonce`        | The DAO route needs a durable nonce account: run `setup-nonce` once (account-owned paths) or pass `--nonce-account` (DAO-owned paths) |
| `utxo`  | btc                                     | `transfer`                       | P2WPKH; one MPC signature per input; change returns to the sender |
| `aptos` | aptos                                   | `transfer`                       | DAO proposals expire 14 days after construction |
| `sui`   | sui                                     | `transfer`                       | Gas-coin references go stale if the derived address is touched while a DAO votes |
| `ton`   | ton                                     | `transfer`                       | v5r1 wallet; deploys itself with its first transaction |

Any chain in a supported family can be added — see Configuration.

## Commands

```
omni
├── account      show / balance            derived addresses and native balances
├── transaction  construct / broadcast     build + sign with MPC; finalize on the destination chain
├── proposal     list / review / vote      the DAO lifecycle
├── config       show / add-chain / remove-chain / sync / reset
└── self-update
```

`transaction broadcast` also recovers a direct-route send whose broadcast
failed: `construct` echoes an `--unsigned-tx` envelope blob for exactly that.

## Configuration

The chain registry lives next to the near-cli-rs config as `omni-config.toml`
(created on first run). Chains are logical — you pick `base` or `solana`, and
the concrete endpoint/chain id resolves from the NEAR network selected at the
`network-config` step (NEAR mainnet → the chain's mainnet, NEAR testnet → its
testnet):

```toml
[chains.base]
family = "evm"
[chains.base.networks.mainnet]
rpc_url = "https://base-rpc.publicnode.com"
chain_id = 8453
explorer_tx_url = "https://basescan.org/tx/"
[chains.base.networks.testnet]
rpc_url = "https://base-sepolia-rpc.publicnode.com"
chain_id = 84532
explorer_tx_url = "https://sepolia.basescan.org/tx/"
```

- `omni config add-chain <key> <family>` writes an entry for you; `omni config
  show` lists the registry.
- `omni config sync` adds default chains a newer CLI version ships without
  touching your own entries; `omni config reset` restores the defaults. Every
  write backs up the previous file to `omni-config.toml.bak`.
- The same file holds `default_derivation_path` (pre-filled in prompts,
  `omni-1` out of the box) and the `[mpc]` signer settings.

## Development

Build from source:

```console
cargo build --release
./target/release/omni
```

Releases are produced by [dist](https://github.com/axodotdev/cargo-dist)
(config in `[workspace.metadata.dist]`, workflow in
`.github/workflows/release.yml`). Bump the version in `Cargo.toml` and push a
matching tag:

```console
git tag v0.1.0 && git push origin v0.1.0
```

After changing the dist config, regenerate the workflow with `dist init`.

The landing page lives in `site/` (vanilla HTML/CSS/JS, same design system
as [near.cli.rs](https://near.cli.rs)) and deploys to GitHub Pages via
`.github/workflows/pages.yml`. Preview it locally with
`python3 -m http.server -d site 8080`.
