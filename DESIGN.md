# omni-cli-rs — Design Document

**Status:** Draft v1 (design settled, pre-implementation)
**Date:** 2026-09-09

## 1. Motivation

The Omni Bridge and Intents teams manage locker contracts through DAOs on many
chains. Every L1 has its own multisig stack (Gnosis Safe on EVM, Squads on SVM,
Petra Vault on Aptos, xDAO on TON, …), which means researching and setting up a
DAO on every new chain, maintaining the same member set in each, and juggling
tabs to review proposals.

NEAR MPC chain signatures remove the need for per-chain governance entirely: a
single NEAR account — a personal account or a single DAO on NEAR — controls
derived accounts on every supported chain. The flow becomes:

> One NEAR DAO creates proposals asking the MPC to sign chain-specific
> payloads; once approved, anyone broadcasts the signed transaction on the
> destination chain.

`omni-cli-rs` is the CLI that makes this workflow practical: building
chain-specific payloads, wrapping them in reviewable DAO proposals (or signing
immediately from a plain account), verifying proposals byte-for-byte at review
time, and assembling + broadcasting the final signed transaction.

## 2. Design principles

- **Stateless CLI.** The NEAR chain is the single source of truth. The
  proposal *is* the pending foreign transaction; the signature lives in NEAR
  receipts once produced. No local database, no coordination server, no
  daemons. Anyone with the CLI and the DAO id can reconstruct everything.
- **No new contracts.** The DAO (or account) calls the MPC signer directly.
  Nothing to develop, deploy, or audit on-chain.
- **No external services.** Runtime dependencies are exactly two RPC
  endpoints: NEAR and the destination chain. No indexers, explorers, ABI
  registries, or simulation services.
- **One-shot commands.** Every command runs and exits, near-cli-rs style.
  Automation is the caller's job (cron, scripts).
- **near-cli-rs native.** Built with `interactive-clap` on top of
  `near-cli-rs` as a library (the near-validator-cli-rs pattern). Every step
  can be typed or answered interactively, and the CLI echoes the full
  non-interactive command line at the end for reproduction — which doubles as
  the artifact you paste into DAO discussions.

## 3. System roles

| Component | Role |
|---|---|
| **omni-cli-rs** | Stateless orchestrator: builds payloads, submits NEAR txs, verifies proposals, assembles + broadcasts signed txs |
| **SputnikDAO v2** | Holds `FunctionCall` proposals targeting the MPC signer; members vote via `act_proposal` |
| **MPC signer contract** | Produces signatures for `(predecessor, path)`-derived keys via `sign(payload, path, …)` (yield/resume) |
| **omni-transaction-rs** | Builds byte-exact unsigned transactions per chain: `build_for_signing()` / `build_with_signature()` |
| **Destination chain RPC** | Queried at construct time (nonce, gas, blockhash, UTXOs) and at broadcast time |

### Key derivation fact that anchors the design

The derived foreign address is a function of **(predecessor account,
derivation path)**. When a DAO calls `sign`, the foreign accounts are rooted at
the DAO — no member key ever controls them. When a personal account calls
`sign`, it gets its own, unrelated family of foreign accounts.
`alice.near + "treasury"` and `dao.sputnik-dao.near + "treasury"` are
different addresses on every chain. The CLI prints the owner prominently in
every render so funds are never sent to the wrong derivation.

## 4. Architecture

### 4.1 Layering

```
 commands (interactive-clap tree)
        │
        ▼
 executor layer ──► sign-as-account : one NEAR tx (account → mpc.sign),
        │                            signature parsed from the same tx outcome,
        │                            assemble + broadcast inline
        │
        └────────► sign-as-dao     : wrap the same sign args in
                                     dao.add_proposal + envelope;
                                     lifecycle = proposal review/vote,
                                     then `omni transaction broadcast`
        │
        ▼
 ChainAdapter trait (one impl per chain family)
        │
        ▼
 omni-transaction-rs builders + thin JSON-RPC clients (reqwest)
```

Everything below the executor layer — chain adapters, derivation math, payload
building, assembly, broadcast, chain registry — is shared verbatim between the
two routes. The route is an `interactive-clap` enum step, not a separate
command, so there is no duplicated construction logic.

### 4.2 The `ChainAdapter` trait

The per-chain abstraction is the load-bearing wall of the codebase. Everything
above it (lifecycle, DAO client, MPC plumbing, review UX) is written once.

```
trait ChainAdapter
├── derived_address(owner, path)      // epsilon derivation → chain-native address
├── fetch_context(rpc, latency)       // nonce / blockhash / UTXOs / gas / seqno
├── build_unsigned(action, ctx)       // → omni-transaction-rs builder
├── signing_payloads(unsigned_tx)     // → Vec<(bytes, SignatureScheme)>  ← Vec!
├── assemble(unsigned_tx, sigs)       // build_with_signature()
├── describe(unsigned_tx)             // human-readable review rendering
├── validity(unsigned_tx, latency)    // expiry / staleness report
└── broadcast(signed_tx, rpc)
```

Two shapes encoded deliberately:

- **`signing_payloads` returns a `Vec`** because UTXO chains need one MPC
  signature *per input*. A SputnikDAO proposal supports multiple actions on
  one receiver, so a 3-input Bitcoin tx is one proposal with three `sign`
  actions (still one foreign transaction).
- **`fetch_context` / `validity` take an execution-latency class**
  (`Immediate` for `sign-as-account`, `Governance` for `sign-as-dao`) because
  chains punish the proposal-to-execution gap differently (see §7).

### 4.3 Chain families

Modules map to omni-transaction-rs one-to-one but are grouped by **family**,
not by chain:

| Family | Chains | Signature scheme | MPC payload |
|---|---|---|---|
| `evm` | Ethereum, Base, Arbitrum, BNB, Polygon, HyperEVM, Avalanche, … (any chain-id) | secp256k1 | 32-byte sighash |
| `svm` | Solana, Fogo, … (differ by RPC + genesis) | ed25519 | full message bytes |
| `utxo` | Bitcoin, Zcash (ZIP-225 v5 transparent) | secp256k1 | 32-byte sighash **per input** |
| `aptos` | Aptos | ed25519 | full signing message (SHA3-256 domain) |
| `sui` | Sui | ed25519 | full intent message (blake2b-256 domain) |
| `ton` | TON | ed25519 | full cell hash payload; wallet deploy handled (address = hash of stateinit) |
| `starknet` | Starknet | STARK curve — **open question §10** | invoke v3 tx hash (Poseidon felt) |

For **ed25519 families the complete transaction is on-chain in the `sign`
args** (the MPC signs the full message), so proposals are reviewable from the
args alone. For **secp256k1 families only the sighash is on-chain**, which is
why the envelope (§6) exists.

## 5. Command tree

```
omni                                   # noun-first: verbs live under nouns
├── transaction
│   ├── construct                      # build foreign tx → choose execution route
│   │   → evm | svm | utxo | aptos | sui | ton | starknet
│   │   → <chain>                      # logical; endpoint resolves at network-config
│   │   → transfer | contract-call | raw   # actions vary per family
│   │   → derivation-path <path>
│   │   ├→ sign-as-account             # owner = the NEAR tx signer
│   │   └→ sign-as-dao <dao-account>   # owner = the DAO
│   │        → network-config <net>
│   │        → sign-with-keychain | sign-with-ledger | …   # near-cli-rs tail
│   │        → send | display          # display = dry-run
│   └── broadcast <near-tx-hash> <tx-signer>    # signature → assemble → destination chain
│        → [--unsigned-tx <base64-envelope>]    # for sign-as-account recovery
│        → network-config <net>
├── proposal
│   ├── list   <dao>
│   ├── review <dao> <id>              # decode + validity + hash verification (read-only)
│   └── vote   <dao> <id> approve|reject
└── account
    ├── show    <owner> <path>         # derived addresses across chains
    └── balance <owner> <path> <chain>
```

Example (reproducible command echoed after any interactive session):

```
omni transaction construct evm base \
  contract-call 0xA0b8… function-signature 'pause()' \
  derivation-path base-locker-admin \
  sign-as-dao bridge-dao.sputnik-dao.near \
  network-config mainnet sign-with-keychain send
```

### 5.1 Ordering nuance: owner resolution

The derived sender (and therefore nonce/UTXOs/balances) depends on the owner,
which is only known at the route fork (`sign-as-dao <dao>`) or, for
`sign-as-account`, at the `sign-with-…` step where the signer account is
named. The user-facing prompt order stays natural, but the internal order is:

> action spec → owner resolution → chain-context fetch → build → render
> (derived sender, decoded action, fees, validity) → confirm → submit

With `sign-as-dao` the render appears right after the fork; with
`sign-as-account` it appears as the final confirmation before `send`.

### 5.2 Terminal steps

- `send` — submit the NEAR transaction (and, for `sign-as-account`, continue
  through signature extraction, assembly, and destination-chain broadcast in
  the same run).
- `display` — dry-run: print the fully-built unsigned foreign tx, the exact
  NEAR `sign` call or `add_proposal` args (envelope included), and the
  reproducible command. Useful for pasting into discussion before proposing.

## 6. Proposal envelope (DAO route only)

A 32-byte sighash is unreviewable on its own, so the full unsigned transaction
rides along in the SputnikDAO `description` field. Layout: the human-readable
intent line **first** (so AstroDAO-style UIs show something legible), then the
base64 envelope:

```json
{
  "omni": 1,                       // envelope version
  "family": "evm",
  "chain": "base",                 // registry key
  "path": "base-locker-admin",     // derivation path = acting foreign account
  "unsigned_tx": "<base64, omni-transaction-rs serialization>",
  "intent": "Pause Base locker during incident #42",
  "meta": { "nonce": 17, "expires": null, "builder_version": "0.1.0" }
}
```

**Trust anchor:** `proposal review` recomputes
`signing_payloads(unsigned_tx)` and verifies **byte-equality** against the
payloads in the proposal's `sign` action args. On mismatch: red banner, no
vote command suggested. The envelope is presentation + reconstruction data;
the hash check is what reviewers rely on. One foreign transaction per
proposal (multi-tx batching would be an envelope v2).

### Description size limits (verified against sputnikdao2 source)

`add_proposal` performs **no length validation** on `description`. Practical
constraints: NEAR caps a signed transaction at ~1.5 MB (args at 4 MB), and
storage (~1 NEAR / 100 KB) is paid from the **DAO's balance** and never
reclaimed — proposals persist in state. Real envelopes are a few hundred bytes
(EVM/UTXO) to ~2–3 KB (Solana v0 with lookup tables), i.e. ~0.02–0.03 NEAR of
DAO storage per proposal. The CLI warns above 16 KB.

## 7. Payload validity vs. voting latency

Payloads are built at construct time; DAO voting takes hours or days. Each
family punishes that differently — `validity()` encodes these rules and
`review` displays them ("valid until …", "assumes nonce 17 — ⚠ 1 other open
proposal also uses it"):

| Family | What goes stale | Mitigation (Governance latency) |
|---|---|---|
| EVM | nonce, gas price | Fine if no concurrent proposals on the path; generous `max_fee_per_gas` ceiling (EIP-1559 refunds the excess). CLI scans open proposals for nonce collisions at construct time |
| SVM | recent blockhash (~1 min) | **Durable nonce account required**; `omni account setup-nonce` creates one per derived account (itself via a proposal). `sign-as-account` route uses a plain recent blockhash |
| UTXO | UTXOs could be spent | Fine — the MPC-derived key is the only spender |
| Aptos | `expiration_timestamp_secs` | Set days out |
| TON | seqno + `valid_until` | `valid_until` far out; seqno stale if concurrent proposals |
| Sui | gas object versions | Report at review; rebuild if consumed |
| Starknet | nonce | Same story as EVM |

Recovery from staleness is honest and cheap: `broadcast` reports why the tx is
no longer valid and the fix is a fresh `construct`. The old signature signs a
now-invalid transaction, so it is harmless.

**Nonce collisions:** two open proposals for the same derived EVM account get
the same nonce. `construct` detects this by scanning open proposals for the
path and offers nonce+1; this is why `proposal list` decodes envelopes rather
than just printing descriptions.

## 8. Lifecycle walkthroughs

### 8.1 `sign-as-account` (no DAO)

```
omni construct … sign-as-account network-config testnet sign-with-keychain send
  1. build unsigned tx (context fetched after signer known)
  2. render + confirm
  3. echo recovery command (incl. --unsigned-tx base64) BEFORE submitting   ← secp256k1 only
  4. submit NEAR tx: account → mpc.sign(payload, path, …)
  5. parse SignatureResponse from the same tx's execution outcome
  6. assemble via build_with_signature()
  7. broadcast to destination RPC → print foreign tx hash
```

Sub-minute end to end. If step 7 fails (RPC hiccup), the recovery command from
step 3 re-enters at `omni transaction broadcast`.

### 8.2 `sign-as-dao`

```
member A:  omni construct … sign-as-dao <dao> … send
             → builds tx, wraps sign args + envelope in add_proposal
             → prints proposal id + ready-made review command
members:   omni proposal review <dao> <id>
             → decode, validity report, ✅ payload hash matches sign args
           omni proposal vote <dao> <id> approve
             → standard near-cli-rs sign-and-send
             → when this vote crosses the threshold, act_proposal executes the
               sign call(s); the CLI prints the ready-to-paste finalize line:
               omni transaction broadcast <near-tx-hash>
anyone:    omni transaction broadcast <near-tx-hash>
             → extract signature(s) from receipts → assemble → broadcast
             → print foreign tx hash
```

### 8.3 `omni transaction broadcast` resolution logic

Given a NEAR tx hash, the CLI inspects the transaction:

1. **`act_proposal`** → walk receipts to the DAO + proposal id → read the
   envelope from the description → verify hash-vs-signature consistency →
   assemble → broadcast.
2. **Direct `sign`, ed25519 family** → the full signed message *is* the
   payload in the args; reconstruct entirely from on-chain data.
3. **Direct `sign`, secp256k1 family** → only the sighash is on-chain;
   requires `--unsigned-tx <base64>` (echoed by `construct` as the recovery
   command).

`broadcast` is idempotent: the signature already exists on NEAR, assembly is
deterministic, and destination chains reject duplicates/stale nonces
harmlessly. Re-broadcast on a different RPC is the same command.

## 9. Configuration

### 9.1 Chain registry

User-editable TOML in the config directory (alongside near-cli-rs config).
Chains are **logical**: one entry per chain, with a variant per NEAR network.
Selecting the chain never asks mainnet/testnet - the concrete endpoint and
chain id resolve when `network-config` is chosen (NEAR mainnet -> the chain's
mainnet, NEAR testnet -> its testnet), so a testnet DAO + testnet MPC can
never sign a payload reviewers mistake for mainnet:

```toml
[chains.base]
family = "evm"
[chains.base.networks.mainnet]
rpc_url = "https://mainnet.base.org"
chain_id = 8453
[chains.base.networks.testnet]
rpc_url = "https://sepolia.base.org"
chain_id = 84532
```

Adding one more EVM/SVM chain is a config entry, not a release. A chain with
no variant for the selected NEAR network is a hard error naming the networks
it does have.

### 9.2 Other settings

- MPC signer contract account per NEAR network (defaulted, overridable).
- Derivation paths are **fully free-form**: no discovery, no local registry;
  `account show` takes an explicit path.

### 9.3 EVM calldata input

Two local-only interactive variants (no ABI fetching):

- **raw hex** — paste pre-encoded calldata (from cast/foundry);
- **function-signature** — cast-style local ABI encoding: type
  `transfer(address,uint256)` plus args. The signature travels in the
  envelope so `review` can render the decoded form.

## 10. Open questions for the first spike (empirical, not design)

1. ~~**MPC signer version/account**~~ - RESOLVED: v1.signer / v1.signer-prod.testnet
   run the chain-signatures v2 contract with key domains (0 = secp256k1,
   1 = ed25519, verified live on testnet + near/mpc source). `sign` takes
   `{"request": {"path", "payload_v2": {"Ecdsa"|"Eddsa": "<hex>"}, "domain_id"}}`;
   responses are tagged with `"scheme"`. Ed25519 payloads are the full message
   bytes (<= 1232 bytes, the Solana packet limit).
2. **Starknet curve** — Starknet natively uses the STARK curve, which is NOT
   an MPC key domain, so standard Starknet accounts cannot be MPC-controlled.
   The route (if wanted) is a secp256k1 account contract (e.g. an EthAccount
   implementation) deployed per derived key. Deferred.
3. **Gas per `sign` action** — whether N sign actions (multi-input UTXO) fit
   in one `act_proposal` under the 300 TGas cap with current yield/resume
   costs; otherwise multi-input spends need proposal splitting.
4. **Envelope size in practice** — measure worst-case Solana v0 envelopes.

## 11. Repository layout & dependencies

```
src/
├── main.rs                     # interactive-clap root, near-cli-rs GlobalContext
├── commands/
│   ├── construct/              # family/chain/action steps + sign-as-* fork
│   ├── proposal/               # list / review / vote
│   ├── broadcast/
│   └── account/                # show, balance, setup-nonce (svm)
├── chains/                     # ChainAdapter + evm, svm, utxo, aptos, sui, ton, starknet
├── envelope.rs                 # encode / decode / verify
├── mpc.rs                      # sign-args construction, derived-key math,
│                               #   signature extraction from outcomes/receipts
├── dao.rs                      # SputnikDAO v2: add_proposal / act_proposal / queries
└── config.rs                   # chain registry TOML
```

Dependencies mirror near-validator-cli-rs: `near-cli-rs` (as a library) +
`interactive-clap`, plus `near-api` for the CLI's own NEAR queries
(`derived_public_key`, DAO policy), `omni-transaction-rs` (git) for
transaction building, and thin JSON-RPC via `reqwest` for destination
chains — **no** solana-sdk / heavy chain SDKs, matching omni-transaction-rs's
own "heavy SDKs are dev-only" philosophy. The one measured exception:
`alloy-dyn-abi` + `alloy-primitives` (encoding-only crates, no RPC stack) for
cast-style `function-signature` calldata encoding, keccak256, and checksummed
addresses — hand-rolling ABI encoding on a security-critical path is the
wrong trade.

## 12. Milestones

1. **Skeleton + EVM + `sign-as-account`** — interactive-clap tree wired to
   near-cli-rs, `ChainAdapter` trait, EVM adapter, direct route end-to-end on
   testnet (construct → sign → broadcast), `omni transaction broadcast`, `account show`.
   Includes the spike resolving §10. The direct route is the fastest proof of
   the whole stack — same adapters and broadcast code the DAO route reuses.
2. **DAO route** — envelope, `sign-as-dao`, `proposal list/review/vote`,
   receipt-walking in `broadcast`, nonce-collision scan.
3. **SVM** — first ed25519 family + durable-nonce management.
4. **UTXO** — multi-signature-action proposals (gas question from §10).
5. **Aptos, Sui, TON, Starknet** — thin adapters over the proven trait; TON
   adds wallet-deploy handling.

## 13. Decision log

| Area | Decision | Alternatives considered |
|---|---|---|
| Architecture | Pure CLI; DAO/account calls MPC signer directly | Intermediate "omni-controller" contract (stores tx + signature on-chain, enforces hash match in consensus) — deferred; envelope format is versioned so it can slot in later |
| Governance | SputnikDAO v2 only | Pluggable governance trait |
| Chains | Everything omni-transaction-rs offers, grouped into 7 family adapters | EVM-first subset |
| Execution routes | One `construct` flow; route = `sign-as-account` / `sign-as-dao` enum step | Separate `send` and `proposal create` commands (duplicated construction logic) |
| Unsigned tx storage | Base64 envelope in proposal description, intent line first | Deterministic rebuild from inputs (fragile across versions); off-chain registry (availability dependency) |
| Batching | One foreign tx per proposal | Multi-tx envelopes (gas caps, partial-broadcast states) |
| Derivation paths | Free-form, no discovery | History-scan discovery; convention + local registry |
| Signature retrieval | `broadcast <near-tx-hash>`; vote echoes the hash when threshold crossed | Indexer lookup (external dependency); vote-and-finalize fusion |
| Review depth | Local decode + validity + hash verification only | ABI/IDL fetching from explorers; RPC simulation |
| Review vs vote | Strictly separate commands | Inline vote prompt after review |
| Execution model | One-shot commands only | `--watch` polling; notify daemon |
| Network coupling | Logical chains with per-NEAR-network variants; endpoint resolves at `network-config` (revised from flat network-pinned entries after CLI feedback) | Flat entries per chain+network; independent selection |
| Naming | `omni construct`; `sign-as-account`/`sign-as-dao`; `derivation-path` | `transaction construct`, `tokens`/`contract` split; `as-transaction`/`as-dao-proposal`, `submit-directly`; `acting-as` |
