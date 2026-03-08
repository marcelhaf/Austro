# Austro — A Peer-to-Peer Blockchain in Rust

Austro is a fully functional proof-of-work blockchain built from scratch in Rust.
It features a UTXO-based transaction model, dynamic difficulty adjustment, peer
discovery via mDNS, and automatic chain synchronization — all over a decentralized
P2P network powered by libp2p.

> Contributions and forks are welcome.

---

## Features

- **Proof-of-Work mining** with dynamic difficulty retargeting (every 2016 blocks, ~60s target)
- **UTXO model** — same design principle as Bitcoin
- **ECDSA signatures** (secp256k1) for transaction authorization
- **P2P networking** via libp2p (gossipsub + mDNS peer discovery)
- **Automatic chain sync** — nodes converge on the longest valid chain
- **Mempool** with fee prioritization
- **Multi-wallet support** per node
- **Transaction history** with confirmations, direction, and fee tracking
- **Halving** every 210 blocks (base reward starts at 50 AUSTRO)

---

## Requirements

| Tool    | Version  |
|---------|----------|
| Rust    | ≥ 1.75   |
| Cargo   | ≥ 1.75   |
| Git     | any      |

---

## Installation

### Windows

```powershell
# 1. Install Rust (if not installed)
winget install Rustlang.Rustup
# Or visit: https://rustup.rs

# 2. Restart your terminal, then verify
rustc --version
cargo --version

# 3. Clone the repository
git clone https://github.com/marcelhaf/Austro.git
cd austro

# 4. Build
cargo build --release
```

### Linux / macOS

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 2. Verify
rustc --version
cargo --version

# 3. Clone the repository
git clone https://github.com/marcelhaf/Austro.git
cd austro

# 4. Build
cargo build --release
```

---

## Running a Node

Each node requires a unique data directory. The directory is created automatically.

### Windows

```powershell
# Terminal 1 — first node
cargo run -- node1

# Terminal 2 — second node (open a new terminal in the same folder)
cargo run -- node2
```

### Linux / macOS

```bash
# Terminal 1
cargo run -- node1

# Terminal 2
cargo run -- node2
```

Nodes on the **same local network** discover each other automatically via mDNS.
No configuration needed.

> **Important:** wait for `Accepted block N` or `Already in sync` to appear
> in the second terminal before running `mine` or `send`. This confirms the
> node has synchronized with the network.

---

## Available Commands

| Command | Description |
|---|---|
| `mine` | Mine a new block and collect the reward + mempool fees |
| `send <address> <amount> [fee]` | Send AUSTRO to an address (fee defaults to 1) |
| `bal [wallet_name]` | Show balance of the active wallet or a named wallet |
| `history [wallet_name\|address]` | Show full transaction history |
| `mempool` | List pending unconfirmed transactions |
| `diff` | Show current difficulty, retarget info, and avg block time |
| `chain` | Print all blocks with hash, tx count, and difficulty |
| `peers` | List currently connected peers |
| `sync` | Manually request chain sync from peers |
| `newwallet <name>` | Create a new named wallet |
| `selectwallet <name>` | Switch the active wallet |
| `listwallets` | List all wallets with balances |
| `exportwallet <name> [wif/json]` | Export wallet to file (WIF or JSON) |
| `importwallet <file> [name]` | Import wallet from file |
---

## Example Session

```
# Node 1 — mine 3 blocks and send to Node 2
mine
mine
mine
bal
send <node2_address> 100

# Node 2 — after sync completes
mine        ← confirms the TX from Node 1
history     ← shows +50 (coinbase) + +100 (received)
bal         ← shows 150 AUSTRO (minus fee)
```

---

### How Sync Works

When a new node connects, it sends a `GetBlocks` message with its current tip hash
and height. The peer responds with a `BlocksBatch` containing all missing blocks.
If the received chain is longer and valid, the node replaces its local chain
(chain reorganization). This is analogous to Bitcoin's `getblocks` / `inv` / `getdata`
message flow.

### Consensus Rules

- Longest valid chain wins
- Each block must satisfy `hash.starts_with("0" * difficulty)`
- Difficulty retargets every 2016 blocks to maintain ~60s block time
- Coinbase transactions are the only transactions without inputs
- All non-coinbase inputs must reference unspent outputs (UTXO set)
- Input values must be ≥ output values (difference = miner fee)

---

## Resetting the Chain

To start fresh and wipe all chain data:

**Windows:**
```powershell
Remove-Item -Recurse -Force node1, node2
```

**Linux / macOS:**
```bash
rm -rf node1 node2
```

---

## Known Limitations

- mDNS peer discovery only works on the **same local network**
- No persistent mempool across restarts
- No block propagation to peers discovered after a block is mined (broadcast only)
- Single-threaded mining (blocks the event loop)

---

## Roadmap

- [x] Wallet import/export (JSON + WIF)
- [ ] `tracing` structured logging
- [ ] Integration test suite (`cargo test`)

---

## Contributing

Pull requests are welcome. For major changes, please open an issue first.

1. Fork the repository
2. Create a branch: `git checkout -b feature/my-feature`
3. Commit your changes: `git commit -m 'Add my feature'`
4. Push: `git push origin feature/my-feature`
5. Open a Pull Request

---

## License

MIT — see [LICENSE](LICENSE) for details.
