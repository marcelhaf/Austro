use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use libp2p::{
    gossipsub::{Event as GossipsubEvent, IdentTopic},
    mdns::Event as MdnsEvent,
    swarm::SwarmEvent,
    Multiaddr, SwarmBuilder,
};
use tokio::io::{self, AsyncBufReadExt};

use crate::models::blockchain::Blockchain;
use crate::models::storage::BlockStore;
use crate::models::transaction::Transaction;
use crate::models::wallet_store::WalletManager;
use crate::network::behaviour::{
    AustroBehaviour, AustroBehaviourEvent, TOPIC_BLOCKS, TOPIC_TRANSACTIONS,
};
use crate::network::sync::{random_nonce, BlocksResponse, GetBlocks, NetworkMessage};

pub async fn run_node(
    blockchain: Arc<Mutex<Blockchain>>,
    store: Arc<BlockStore>,
    wallet_manager: Arc<Mutex<WalletManager>>,
    config: crate::NodeConfig,
) {
    let local_key = libp2p::identity::Keypair::generate_ed25519();
    let local_peer_id = libp2p::PeerId::from(local_key.public());
    println!("Peer id  : {}", local_peer_id);

    let topic_blocks = IdentTopic::new(TOPIC_BLOCKS);
    let topic_txs = IdentTopic::new(TOPIC_TRANSACTIONS);
    let key_for_behaviour = local_key.clone();

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .expect("TCP transport")
        .with_behaviour(|key| {
            AustroBehaviour::new(libp2p::PeerId::from(key.public()), &key_for_behaviour)
        })
        .expect("Behaviour")
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(300)))
        .build();

    swarm.behaviour_mut().gossipsub.subscribe(&topic_blocks).unwrap();
    swarm.behaviour_mut().gossipsub.subscribe(&topic_txs).unwrap();

    let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", config.port);
    swarm.listen_on(listen_addr.parse().unwrap()).unwrap();

    for peer_addr_str in &config.bootstrap_peers {
        match peer_addr_str.parse::<Multiaddr>() {
            Ok(addr) => {
                println!("Dialing bootstrap peer: {}", addr);
                if let Err(e) = swarm.dial(addr) {
                    println!("Bootstrap dial error: {:?}", e);
                }
            }
            Err(e) => println!("Invalid --peer address '{}': {}", peer_addr_str, e),
        }
    }

    println!("Commands:");
    println!("  mine");
    println!("  send <address> <amount> [fee]");
    println!("  bal [wallet_name]");
    println!("  newwallet <name>");
    println!("  selectwallet <name>");
    println!("  listwallets");
    println!("  exportwallet <name> [wif|json]");
    println!("  importwallet <file> [name]");
    println!("  mempool");
    println!("  diff");
    println!("  peers");
    println!("  chain");
    println!("  sync");
    println!("  history [wallet_name|address]");

    let stdin = io::stdin();
    let mut lines = io::BufReader::new(stdin).lines();

    let mut sync_interval = tokio::time::interval(Duration::from_secs(3));
    let mut has_peers = false;
    let mut synced = false;
    let mut tx_buffer: VecDeque<Transaction> = VecDeque::new();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                if let Ok(Some(cmd)) = line {
                    handle_command(
                        cmd.trim(),
                        &mut swarm,
                        &blockchain,
                        &store,
                        &wallet_manager,
                        &topic_blocks,
                        &topic_txs,
                    ).await;
                }
            }

            _ = sync_interval.tick() => {
                if has_peers && !synced {
                    let chain = blockchain.lock().unwrap();
                    let req = NetworkMessage::GetBlocks(GetBlocks {
                        from_hash: chain.tip_hash(),
                        from_height: chain.height(),
                        nonce: random_nonce(),
                    });
                    drop(chain);
                    let _ = swarm.behaviour_mut().gossipsub
                        .publish(topic_blocks.clone(), req.serialize());
                }
            }

            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        println!("Listening on: {}", address);
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Mdns(
                        MdnsEvent::Discovered(list)
                    )) => {
                        for (peer_id, multiaddr) in list {
                            println!("Discovered peer: {}", peer_id);
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            let _ = swarm.dial(multiaddr);
                        }
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Mdns(
                        MdnsEvent::Expired(list)
                    )) => {
                        for (peer_id, _) in list {
                            swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        }
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Gossipsub(
                        GossipsubEvent::Message { message, .. }
                    )) => {
                        let sync_happened = handle_network_message(
                            message.data,
                            &mut swarm,
                            &blockchain,
                            &store,
                            &topic_blocks,
                            &topic_txs,
                            &mut tx_buffer,
                            synced,
                        );
                        if sync_happened {
                            synced = true;
                            if !tx_buffer.is_empty() {
                                let mut chain = blockchain.lock().unwrap();
                                let mut reaccepted = 0usize;
                                while let Some(tx) = tx_buffer.pop_front() {
                                    if chain.mempool.contains(&tx.id) { continue; }
                                    if chain.validate_transaction(&tx) {
                                        let fee = chain.calculate_fee(&tx).unwrap_or(0);
                                        if chain.mempool.add(tx, fee).is_ok() { reaccepted += 1; }
                                    }
                                }
                                if reaccepted > 0 {
                                    println!("Revalidated {} buffered TX(s) after sync", reaccepted);
                                }
                            }
                        }
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Gossipsub(
                        GossipsubEvent::Subscribed { peer_id, topic }
                    )) => {
                        println!("Peer {} joined {}", peer_id, topic);
                        has_peers = true;
                        synced = false;
                        let chain = blockchain.lock().unwrap();
                        let req = NetworkMessage::GetBlocks(GetBlocks {
                            from_hash: chain.tip_hash(),
                            from_height: chain.height(),
                            nonce: random_nonce(),
                        });
                        let mempool_txs = chain.mempool.pending_txs();
                        drop(chain);
                        let _ = swarm.behaviour_mut().gossipsub
                            .publish(topic_blocks.clone(), req.serialize());
                        if !mempool_txs.is_empty() {
                            let _ = swarm.behaviour_mut().gossipsub
                                .publish(topic_txs.clone(),
                                    NetworkMessage::MempoolTxs(mempool_txs).serialize());
                        }
                    }

                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        println!("Connected   : {}", peer_id);
                        has_peers = true;
                        let chain = blockchain.lock().unwrap();
                        if chain.height() == 0 { synced = false; }
                        drop(chain);
                    }

                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        println!("Disconnected: {} | {:?}", peer_id, cause);
                        if swarm.connected_peers().count() == 0 {
                            has_peers = false;
                            synced = false;
                        }
                    }

                    _ => {}
                }
            }
        }
    }
}

// ── Network ───────────────────────────────────────────────────────────────────

fn handle_network_message(
    data: Vec<u8>,
    swarm: &mut libp2p::Swarm<AustroBehaviour>,
    blockchain: &Arc<Mutex<Blockchain>>,
    store: &Arc<BlockStore>,
    topic_blocks: &IdentTopic,
    topic_txs: &IdentTopic,
    tx_buffer: &mut VecDeque<Transaction>,
    synced: bool,
) -> bool {
    let msg = match NetworkMessage::deserialize(&data) {
        Some(m) => m,
        None => return false,
    };

    match msg {
        NetworkMessage::NewBlock(block) => {
            let mut chain = blockchain.lock().unwrap();
            if chain.try_append_block(block.clone(), store) {
                println!("Accepted block {} | {}", block.index, &block.hash[..16]);
                let confirmed: Vec<String> =
                    block.transactions.iter().map(|tx| tx.id.clone()).collect();
                chain.mempool.purge_confirmed(&confirmed);
            } else if block.index > chain.height() + 1 {
                let req = NetworkMessage::GetBlocks(GetBlocks {
                    from_hash: chain.tip_hash(),
                    from_height: chain.height(),
                    nonce: random_nonce(),
                });
                drop(chain);
                let _ = swarm.behaviour_mut().gossipsub
                    .publish(topic_blocks.clone(), req.serialize());
            }
            false
        }

        NetworkMessage::NewTx(tx) => {
            let mut chain = blockchain.lock().unwrap();
            if chain.mempool.contains(&tx.id) { return false; }
            if chain.validate_transaction(&tx) {
                let fee = chain.calculate_fee(&tx).unwrap_or(0);
                match chain.mempool.add(tx.clone(), fee) {
                    Ok(_) => {
                        println!("New TX: {} | fee={} AUSTRO", &tx.id[..8], fee);
                        drop(chain);
                        let _ = swarm.behaviour_mut().gossipsub
                            .publish(topic_txs.clone(), NetworkMessage::NewTx(tx).serialize());
                    }
                    Err(e) => println!("TX rejected: {}", e),
                }
            } else if !synced {
                println!("TX buffered (awaiting sync): {}", &tx.id[..8]);
                tx_buffer.push_back(tx);
            } else {
                println!("TX invalid — discarded");
            }
            false
        }

        NetworkMessage::GetBlocks(req) => {
            let chain = blockchain.lock().unwrap();

            if req.from_height == chain.height() && req.from_hash == chain.tip_hash() {
                return false;
            }

            let start = if req.from_height == 0 {
                if !chain.chain.is_empty() && chain.chain[0].hash == req.from_hash { 1 } else { 0 }
            } else {
                let h = req.from_height as usize;
                if h < chain.chain.len() && chain.chain[h].hash == req.from_hash {
                    h + 1
                } else {
                    0
                }
            };

            if start >= chain.chain.len() { return false; }

            let blocks: Vec<_> = chain.chain[start..].to_vec();
            drop(chain);

            println!("Sending {} blocks to peer (from height {})", blocks.len(), start);
            let _ = swarm.behaviour_mut().gossipsub.publish(
                topic_blocks.clone(),
                NetworkMessage::BlocksBatch(BlocksResponse { blocks, nonce: random_nonce() }).serialize(),
            );
            false
        }

        NetworkMessage::BlocksBatch(resp) => {
            if resp.blocks.is_empty() { return false; }
            let mut chain = blockchain.lock().unwrap();

            let mut candidate = chain.chain.clone();
            for block in &resp.blocks {
                let idx = block.index as usize;
                if idx < candidate.len() {
                    if candidate[idx].hash != block.hash {
                        candidate.truncate(idx);
                        candidate.push(block.clone());
                    }
                } else if idx == candidate.len() {
                    candidate.push(block.clone());
                }
            }

            if candidate.len() > chain.chain.len() {
                if chain.try_replace_chain(candidate, store) {
                    println!("Sync complete — height: {}", chain.height());
                    return true;
                }
            } else {
                println!("Already in sync — height: {}", chain.height());
                return true;
            }
            false
        }

        NetworkMessage::GetMempool => {
            let chain = blockchain.lock().unwrap();
            let txs = chain.mempool.pending_txs();
            drop(chain);
            if !txs.is_empty() {
                let _ = swarm.behaviour_mut().gossipsub
                    .publish(topic_txs.clone(), NetworkMessage::MempoolTxs(txs).serialize());
            }
            false
        }

        NetworkMessage::MempoolTxs(txs) => {
            let mut chain = blockchain.lock().unwrap();
            let mut accepted = 0usize;
            for tx in txs {
                if chain.mempool.contains(&tx.id) { continue; }
                if chain.validate_transaction(&tx) {
                    let fee = chain.calculate_fee(&tx).unwrap_or(0);
                    if chain.mempool.add(tx, fee).is_ok() { accepted += 1; }
                }
            }
            if accepted > 0 { println!("Mempool sync: +{} TXs from peer", accepted); }
            false
        }
    }
}

// ── CLI ───────────────────────────────────────────────────────────────────────

async fn handle_command(
    cmd: &str,
    swarm: &mut libp2p::Swarm<AustroBehaviour>,
    blockchain: &Arc<Mutex<Blockchain>>,
    store: &Arc<BlockStore>,
    wallet_manager: &Arc<Mutex<WalletManager>>,
    topic_blocks: &IdentTopic,
    topic_txs: &IdentTopic,
) {
    let parts: Vec<&str> = cmd.splitn(4, ' ').collect();
    match parts[0] {
        "mine" => {
            let miner = wallet_manager.lock().unwrap().current_wallet().clone();
            let mut chain = blockchain.lock().unwrap();
            chain.mine_pending_transactions(&miner, store);
            let last = chain.chain.last().unwrap().clone();
            drop(chain);
            match swarm.behaviour_mut().gossipsub
                .publish(topic_blocks.clone(), NetworkMessage::NewBlock(last).serialize())
            {
                Ok(id) => println!("Block broadcast ({:?})", id),
                Err(e) => println!("Broadcast error: {:?}", e),
            }
        }

        "send" => {
            if parts.len() < 3 { println!("Usage: send <address> <amount> [fee]"); return; }
            let to_addr = parts[1];
            let amount: u64 = match parts[2].parse() {
                Ok(v) => v, Err(_) => { println!("Invalid amount"); return; }
            };
            let fee: u64 = if parts.len() == 4 {
                match parts[3].parse() { Ok(v) => v, Err(_) => { println!("Invalid fee"); return; } }
            } else { 1 };
            let to_hash = match hex::decode(to_addr) {
                Ok(h) if h.len() == 32 => h,
                _ => { println!("Invalid address (expected 64-char hex)"); return; }
            };
            let from = wallet_manager.lock().unwrap().current_wallet().clone();
            let mut chain = blockchain.lock().unwrap();
            match chain.create_transaction(&from, &to_hash, amount, fee) {
                Ok((tx, actual_fee)) => {
                    println!("TX created: {} | {} AUSTRO → {} | fee: {} AUSTRO",
                        &tx.id[..8], amount, &to_addr[..16], actual_fee);
                    let tx_clone = tx.clone();
                    drop(chain);
                    match swarm.behaviour_mut().gossipsub
                        .publish(topic_txs.clone(), NetworkMessage::NewTx(tx_clone).serialize())
                    {
                        Ok(_) => println!("TX broadcast to peers"),
                        Err(e) => println!("TX broadcast error: {:?}", e),
                    }
                }
                Err(e) => println!("TX error: {}", e),
            }
        }

        "bal" => {
            let wm = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            if parts.len() == 2 {
                match wm.get_wallet(parts[1]) {
                    Some(w) => {
                        println!("Wallet  : {}", parts[1]);
                        println!("Address : {}", w.address());
                        println!("Balance : {} AUSTRO", chain.get_balance(w));
                    }
                    None => println!("Wallet '{}' not found", parts[1]),
                }
            } else {
                let w = wm.current_wallet();
                println!("Wallet  : {} (active)", wm.selected);
                println!("Address : {}", w.address());
                println!("Balance : {} AUSTRO", chain.get_balance(w));
            }
        }

        "newwallet" => {
            if parts.len() < 2 { println!("Usage: newwallet <name>"); return; }
            let mut wm = wallet_manager.lock().unwrap();
            match wm.create_wallet(parts[1]) {
                Ok(addr) => println!("Wallet '{}' created\nAddress: {}", parts[1], addr),
                Err(e) => println!("Error: {}", e),
            }
        }

        "selectwallet" => {
            if parts.len() < 2 { println!("Usage: selectwallet <name>"); return; }
            let mut wm = wallet_manager.lock().unwrap();
            match wm.select_wallet(parts[1]) {
                Ok(_) => {
                    let addr = wm.current_wallet().address();
                    println!("Active wallet: {} | {}", parts[1], &addr[..16]);
                }
                Err(e) => println!("Error: {}", e),
            }
        }

        "listwallets" => {
            let wm = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            println!("Wallets ({}):", wm.list_wallets().len());
            for name in wm.list_wallets() {
                let wallet = wm.get_wallet(&name).unwrap();
                let bal = chain.get_balance(wallet);
                let active = if name == wm.selected { " ← active" } else { "" };
                println!("  {:12} | {} | {} AUSTRO{}", name, &wallet.address()[..16], bal, active);
            }
        }

        "exportwallet" => {
            if parts.len() < 2 { println!("Usage: exportwallet <name> [wif|json]"); return; }
            let name = parts[1];
            let format = if parts.len() == 3 { parts[2] } else { "json" };
            let wm = wallet_manager.lock().unwrap();
            match wm.export_wallet(name, format) {
                Ok(content) => {
                    println!("Wallet '{}' exported to {}.{}", name, format.to_uppercase(), format);
                    println!("Content: {}", content);
                }
                Err(e) => println!("Export error: {}", e),
            }
        }

        "importwallet" => {
            if parts.len() < 2 { println!("Usage: importwallet <file> [name]"); return; }
            let file_path = parts[1];
            let name = if parts.len() == 3 { Some(parts[2]) } else { None };
            let mut wm = wallet_manager.lock().unwrap();
            match wm.import_wallet(file_path, name) {
                Ok(addr) => println!("Wallet imported\nAddress: {}", addr),
                Err(e) => println!("Import error: {}", e),
            }
        }

        "mempool" => {
            let chain = blockchain.lock().unwrap();
            println!("Mempool: {} TX(s) | total fees: {} AUSTRO",
                chain.mempool.size(), chain.mempool.total_fees());
            for entry in &chain.mempool.entries {
                let total_out: u64 = entry.tx.vout.iter().map(|o| o.value).sum();
                println!("  {} | inputs={} outputs={} amount={} fee={}",
                    &entry.tx.id[..16], entry.tx.vin.len(),
                    entry.tx.vout.len(), total_out, entry.fee);
            }
        }

        "diff" => {
            let chain = blockchain.lock().unwrap();
            let info = chain.difficulty_info();
            println!("┌─ Difficulty ─────────────────────────────┐");
            println!("│ Current difficulty   : {:>6} leading 0s │", info.current);
            println!("│ Chain height         : {:>6} blocks     │", info.height);
            println!("│ Next retarget in     : {:>6} blocks     │", info.blocks_until_retarget);
            println!("│ Avg block time (10)  : {:>6}s           │", info.avg_block_time_secs);
            println!("│ Target block time    : {:>6}s           │", info.target_block_time_secs);
            println!("└──────────────────────────────────────────┘");
        }

        "peers" => {
            let peers: Vec<_> = swarm.connected_peers().cloned().collect();
            println!("Connected peers: {}", peers.len());
            for p in &peers { println!("  {}", p); }
        }

        "chain" => {
            let chain = blockchain.lock().unwrap();
            println!("Height: {} | Difficulty: {} | Valid: {}",
                chain.height(), chain.difficulty, chain.is_valid());
            for block in &chain.chain {
                println!("  Block {:>4} | {} | txs={:>2} | diff={} | reward={}",
                    block.index, &block.hash[..16],
                    block.transactions.len(), block.difficulty, block.reward);
            }
        }

        "sync" => {
            let chain = blockchain.lock().unwrap();
            let req = NetworkMessage::GetBlocks(GetBlocks {
                from_hash: chain.tip_hash(),
                from_height: chain.height(),
                nonce: random_nonce(),
            });
            drop(chain);
            match swarm.behaviour_mut().gossipsub
                .publish(topic_blocks.clone(), req.serialize())
            {
                Ok(_) => println!("Sync request sent"),
                Err(e) => println!("Sync error: {:?}", e),
            }
        }

        "history" => {
            let wm = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            let (label, pub_key_hash): (String, Vec<u8>) = if parts.len() == 2 {
                let arg = parts[1];
                if arg.len() == 64 {
                    match hex::decode(arg) {
                        Ok(hash) => (format!("{}...", &arg[..16]), hash),
                        Err(_) => { println!("Invalid address hex"); return; }
                    }
                } else {
                    match wm.get_wallet(arg) {
                        Some(w) => (arg.to_string(), w.pub_key_hash()),
                        None => { println!("Wallet '{}' not found", arg); return; }
                    }
                }
            } else {
                let w = wm.current_wallet();
                (wm.selected.clone(), w.pub_key_hash())
            };

            let records = crate::models::history::build_history(
                &chain.chain, &pub_key_hash, chain.height());
            if records.is_empty() { println!("No transactions for '{}'", label); return; }

            println!("History for '{}' ({} TXs):", label, records.len());
            println!("{:<18} {:>6} {:>6} {:>10} {:>8} {:>5}",
                "TX ID", "BLOCK", "CONF", "NET", "FEE", "DIR");
            println!("{}", "─".repeat(62));
            for r in &records {
                let dir = match r.direction {
                    crate::models::history::TxDirection::Received => "IN ",
                    crate::models::history::TxDirection::Sent     => "OUT",
                    crate::models::history::TxDirection::Self_    => "---",
                };
                let net_str = if r.net >= 0 { format!("+{}", r.net) } else { format!("{}", r.net) };
                println!("{:<18} {:>6} {:>6} {:>10} {:>8} {:>5}",
                    &r.tx_id[..16], r.block_height, r.confirmations, net_str, r.fee, dir);
            }
            let total_received: i64 = records.iter()
                .filter(|r| r.direction == crate::models::history::TxDirection::Received)
                .map(|r| r.net).sum();
            let total_sent: i64 = records.iter()
                .filter(|r| r.direction == crate::models::history::TxDirection::Sent)
                .map(|r| r.net).sum();
            let total_fees: u64 = records.iter().map(|r| r.fee).sum();
            println!("{}", "─".repeat(62));
            println!("  Received: {:>8} AUSTRO | Sent: {:>8} AUSTRO | Fees paid: {} AUSTRO",
                total_received, total_sent.abs(), total_fees);
        }

        other => { if !other.is_empty() { println!("Unknown command: '{}'", other); } }
    }
}
