# Austro — Proof-of-Work Blockchain in Rust

Austro is a fully functional proof-of-work blockchain built from scratch in Rust. It features a UTXO-based transaction model, ECDSA signatures, dynamic difficulty adjustment, BIP39 seed phrases, AES-256-GCM wallet encryption, peer discovery via mDNS and bootstrap nodes, and automatic chain synchronization over a decentralized P2P network powered by libp2p.

> **Explorer:** https://cryptoaustro.duckdns.org
> **GitHub Pages:** https://marcelhaf.github.io/Austro
> **Bootstrap node:** `/ip4/147.224.133.52/tcp/4001`

---

## Download

Pre-built binaries are available on the [Releases](https://github.com/marcelhaf/Austro/releases/latest) page.

| Platform | File |
|---|---|
| Windows x86_64 | `austro-windows-x86_64.zip` |
| Linux x86_64 | `austro-linux-x86_64.tar.gz` |
| macOS Intel | `austro-macos-x86_64.tar.gz` |
| macOS Apple Silicon | `austro-macos-arm64.tar.gz` |

### Quick start (Windows)
```powershell
Expand-Archive austro-windows-x86_64.zip
.\austro-windows-x86_64.exe node
```

### Quick start (Linux / macOS)
```bash
tar -xzf austro-linux-x86_64.tar.gz
chmod +x austro-linux-x86_64
./austro-linux-x86_64 node
```

The node connects automatically to the bootstrap node and syncs the chain. No configuration needed.

---

## Build from Source

### Requirements

| Tool | Version |
|---|---|
| Rust | ≥ 1.75 |
| Cargo | ≥ 1.75 |
| Git | any |

### Windows
```powershell
winget install Rustlang.Rustup
rustc --version
git clone https://github.com/marcelhaf/Austro.git
cd Austro
cargo build --release
.\target\release\austro.exe node
```

### Linux / macOS
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
git clone https://github.com/marcelhaf/Austro.git
cd Austro
cargo build --release
./target/release/austro node
```

---

## Contributing

Pull requests are welcome. For major changes, open an issue first.
```bash
git checkout -b feature/my-feature
git commit -m 'feat: add my feature'
git push origin feature/my-feature
```

---

## License

MIT — see [LICENSE](LICENSE) for details.