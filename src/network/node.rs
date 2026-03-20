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
use tracing::{debug, info, instrument, warn};

use crate::models::blockchain::Blockchain;
use crate::models::storage::BlockStore;
use crate::models::transaction::Transaction;
use crate::models::wallet_store::WalletManager;
use crate::network::behaviour::{AustroBehaviour, AustroBehaviourEvent, TOPIC_BLOCKS, TOPIC_TRANSACTIONS};
use crate::network::sync::{random_nonce, BlocksResponse, GetBlocks, NetworkMessage};

pub async fn run_node(
    blockchain:     Arc<Mutex<Blockchain>>,
    store:          Arc<BlockStore>,
    wallet_manager: Arc<Mutex<WalletManager>>,
    config:         crate::NodeConfig,
) {
    let local_key     = libp2p::identity::Keypair::generate_ed25519();
    let local_peer_id = libp2p::PeerId::from(local_key.public());
    info!(peer_id = %local_peer_id, "P2P identity generated");

    let topic_blocks      = IdentTopic::new(TOPIC_BLOCKS);
    let topic_txs         = IdentTopic::new(TOPIC_TRANSACTIONS);
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
                info!(addr = %addr, "Dialing bootstrap peer");
                if let Err(e) = swarm.dial(addr) {
                    warn!(error = ?e, addr = %peer_addr_str, "Bootstrap dial failed");
                }
            }
            Err(e) => warn!(error = %e, addr = %peer_addr_str, "Invalid bootstrap peer address"),
        }
    }

    info!("Node ready — commands: mine | send | bal | newwallet | selectwallet | listwallets | exportwallet | importwallet | mempool | diff | peers | chain | sync | history");

    let stdin = io::stdin();
    let mut lines = io::BufReader::new(stdin).lines();

    let mut sync_interval = tokio::time::interval(Duration::from_secs(3));
    let mut has_peers = false;
    let mut synced    = false;
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
                        from_hash:   chain.tip_hash(),
                        from_height: chain.height(),
                        nonce:       random_nonce(),
                    });
                    drop(chain);
                    debug!("Sending periodic sync request");
                    let _ = swarm.behaviour_mut().gossipsub
                        .publish(topic_blocks.clone(), req.serialize());
                }
            }

            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(addr = %address, "Listening on new address");
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Mdns(MdnsEvent::Discovered(list))) => {
                        for (peer_id, multiaddr) in list {
                            info!(peer_id = %peer_id, addr = %multiaddr, "mDNS peer discovered");
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            let _ = swarm.dial(multiaddr);
                        }
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Mdns(MdnsEvent::Expired(list))) => {
                        for (peer_id, _) in list {
                            debug!(peer_id = %peer_id, "mDNS peer record expired");
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
                                let mut chain      = blockchain.lock().unwrap();
                                let mut reaccepted = 0usize;
                                while let Some(tx) = tx_buffer.pop_front() {
                                    if chain.mempool.contains(&tx.id) { continue; }
                                    if chain.validate_transaction(&tx) {
                                        let fee = chain.calculate_fee(&tx).unwrap_or(0);
                                        if chain.mempool.add(tx, fee).is_ok() { reaccepted += 1; }
                                    }
                                }
                                if reaccepted > 0 {
                                    info!(count = reaccepted, "Buffered transactions revalidated after sync");
                                }
                            }
                        }
                    }

                    SwarmEvent::Behaviour(AustroBehaviourEvent::Gossipsub(
                        GossipsubEvent::Subscribed { peer_id, topic }
                    )) => {
                        info!(peer_id = %peer_id, topic = %topic, "Peer joined topic");
                        has_peers = true;
                        synced    = false;

                        let chain       = blockchain.lock().unwrap();
                        let req         = NetworkMessage::GetBlocks(GetBlocks {
                            from_hash:   chain.tip_hash(),
                            from_height: chain.height(),
                            nonce:       random_nonce(),
                        });
                        let mempool_txs = chain.mempool.pending_txs();
                        drop(chain);

                        let _ = swarm.behaviour_mut().gossipsub
                            .publish(topic_blocks.clone(), req.serialize());
                        if !mempool_txs.is_empty() {
                            debug!(count = mempool_txs.len(), "Sharing mempool with new peer");
                            let _ = swarm.behaviour_mut().gossipsub
                                .publish(topic_txs.clone(),
                                    NetworkMessage::MempoolTxs(mempool_txs).serialize());
                        }
                    }

                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        info!(peer_id = %peer_id, "Connection established");
                        has_peers = true;
                        let chain = blockchain.lock().unwrap();
                        if chain.height() == 0 { synced = false; }
                    }

                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        info!(peer_id = %peer_id, cause = ?cause, "Connection closed");
                        if swarm.connected_peers().count() == 0 {
                            has_peers = false;
                            synced    = false;
                            debug!("No peers remaining");
                        }
                    }

                    _ => {}
                }
            }
        }
    }
}

fn handle_network_message(
    data:         Vec<u8>,
    swarm:        &mut libp2p::Swarm<AustroBehaviour>,
    blockchain:   &Arc<Mutex<Blockchain>>,
    store:        &Arc<BlockStore>,
    topic_blocks: &IdentTopic,
    topic_txs:    &IdentTopic,
    tx_buffer:    &mut VecDeque<Transaction>,
    synced:       bool,
) -> bool {
    let msg = match NetworkMessage::deserialize(&data) {
        Some(m) => m,
        None => {
            warn!(bytes = data.len(), "Received undeserializable network message");
            return false;
        }
    };

    match msg {
        NetworkMessage::NewBlock(block) => {
            let mut chain = blockchain.lock().unwrap();
            if chain.try_append_block(block.clone(), store) {
                let confirmed: Vec<String> =
                    block.transactions.iter().map(|tx| tx.id.clone()).collect();
                chain.mempool.purge_confirmed(&confirmed);
            } else if block.index > chain.height() + 1 {
                debug!(block_height = block.index, our_height = chain.height(), "Block ahead — requesting sync");
                let req = NetworkMessage::GetBlocks(GetBlocks {
                    from_hash:   chain.tip_hash(),
                    from_height: chain.height(),
                    nonce:       random_nonce(),
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
                        info!(tx_id = %tx.id, fee, mempool_size = chain.mempool.size(), "Transaction accepted into mempool");
                        drop(chain);
                        let _ = swarm.behaviour_mut().gossipsub
                            .publish(topic_txs.clone(), NetworkMessage::NewTx(tx).serialize());
                    }
                    Err(e) => warn!(tx_id = %tx.id, error = %e, "Transaction rejected by mempool"),
                }
            } else if !synced {
                debug!(tx_id = %tx.id, "Transaction buffered — awaiting sync");
                tx_buffer.push_back(tx);
            } else {
                warn!(tx_id = %tx.id, "Transaction invalid — discarded");
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
                if h < chain.chain.len() && chain.chain[h].hash == req.from_hash { h + 1 } else { 0 }
            };

            if start >= chain.chain.len() { return false; }

            let blocks     = chain.chain[start..].to_vec();
            let blocks_len = blocks.len();
            drop(chain);

            debug!(from_height = req.from_height, sending = blocks_len, "Serving block batch to peer");
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
                    info!(height = chain.height(), "Sync complete");
                    return true;
                }
            } else {
                debug!(height = chain.height(), "Already in sync");
                return true;
            }
            false
        }

        NetworkMessage::GetMempool => {
            let chain = blockchain.lock().unwrap();
            let txs   = chain.mempool.pending_txs();
            drop(chain);
            if !txs.is_empty() {
                debug!(count = txs.len(), "Serving mempool to peer");
                let _ = swarm.behaviour_mut().gossipsub
                    .publish(topic_txs.clone(), NetworkMessage::MempoolTxs(txs).serialize());
            }
            false
        }

        NetworkMessage::MempoolTxs(txs) => {
            let mut chain    = blockchain.lock().unwrap();
            let mut accepted = 0usize;
            for tx in txs {
                if chain.mempool.contains(&tx.id) { continue; }
                if chain.validate_transaction(&tx) {
                    let fee = chain.calculate_fee(&tx).unwrap_or(0);
                    if chain.mempool.add(tx, fee).is_ok() { accepted += 1; }
                }
            }
            if accepted > 0 {
                info!(count = accepted, "Mempool transactions accepted from peer");
            }
            false
        }
    }
}

#[instrument(skip(swarm, blockchain, store, wallet_manager, topic_blocks, topic_txs),
             fields(cmd))]
async fn handle_command(
    cmd:            &str,
    swarm:          &mut libp2p::Swarm<AustroBehaviour>,
    blockchain:     &Arc<Mutex<Blockchain>>,
    store:          &Arc<BlockStore>,
    wallet_manager: &Arc<Mutex<WalletManager>>,
    topic_blocks:   &IdentTopic,
    topic_txs:      &IdentTopic,
) {
    let parts: Vec<&str> = cmd.splitn(4, ' ').collect();
    match parts[0] {
        "mine" => {
            let miner       = wallet_manager.lock().unwrap().current_wallet().clone();
            let difficulty  = blockchain.lock().unwrap().difficulty;
            let tip_hash    = blockchain.lock().unwrap().tip_hash();
            let height      = blockchain.lock().unwrap().height();
            let reward      = blockchain.lock().unwrap().mining_reward;
            let pending     = blockchain.lock().unwrap().mempool.collect_for_block(500);
            let total_fees: u64 = blockchain.lock().unwrap().mempool.entries
                .iter().take(500).map(|e| e.fee).sum();

            let coinbase_value = reward + total_fees;
            let reward_tx = crate::models::transaction::Transaction::coinbase(
                &miner.pub_key_hash(), coinbase_value,
            );
            let mut transactions = vec![reward_tx];
            transactions.extend(pending.clone());

            let mut block = crate::models::block::Block::new(
                height + 1,
                tip_hash,
                transactions,
                coinbase_value,
                difficulty,
            );

            info!(miner = %miner.address(), height = height + 1, "Mining started (non-blocking)");

            let mined_block = tokio::task::spawn_blocking(move || {
                block.mine(difficulty);
                block
            }).await.unwrap();

            let mut chain = blockchain.lock().unwrap();
            if chain.try_append_block(mined_block.clone(), store) {
                let confirmed: Vec<String> = pending.iter().map(|tx| tx.id.clone()).collect();
                chain.mempool.purge_confirmed(&confirmed);
                if chain.chain.len() % 210 == 0 && chain.mining_reward > 1 {
                    chain.mining_reward /= 2;
                    info!(new_reward = chain.mining_reward, "Block reward halving");
                }
                drop(chain);
                match swarm.behaviour_mut().gossipsub
                    .publish(topic_blocks.clone(), NetworkMessage::NewBlock(mined_block.clone()).serialize())
                {
                    Ok(msg_id) => info!(
                        block_height = mined_block.index,
                        hash         = %mined_block.hash,
                        msg_id       = ?msg_id,
                        "Block mined and broadcast"
                    ),
                    Err(e) => warn!(error = ?e, "Block broadcast failed"),
                }
            } else {
                warn!("Mined block rejected by chain (stale)");
            }
        }

        "send" => {
            if parts.len() < 3 { warn!("Usage: send <address> <amount> [fee]"); return; }
            let to_addr = parts[1];
            let amount: u64 = match parts[2].parse() {
                Ok(v)  => v,
                Err(_) => { warn!("Invalid amount"); return; }
            };
            let fee: u64 = if parts.len() == 4 {
                match parts[3].parse() { Ok(v) => v, Err(_) => { warn!("Invalid fee"); return; } }
            } else { 1 };

            let to_hash = match hex::decode(to_addr) {
                Ok(h) if h.len() == 32 => h,
                _ => { warn!(addr = to_addr, "Invalid destination address"); return; }
            };

            let from      = wallet_manager.lock().unwrap().current_wallet().clone();
            let mut chain = blockchain.lock().unwrap();
            match chain.create_transaction(&from, &to_hash, amount, fee) {
                Ok((tx, actual_fee)) => {
                    let _ = store.save_mempool(&chain.mempool.entries);
                    let tx_id    = tx.id.clone();
                    let tx_clone = tx;
                    drop(chain);
                    match swarm.behaviour_mut().gossipsub
                        .publish(topic_txs.clone(), NetworkMessage::NewTx(tx_clone).serialize())
                    {
                        Ok(_)  => info!(tx_id = %tx_id, actual_fee, "Transaction broadcast"),
                        Err(e) => warn!(error = ?e, tx_id = %tx_id, "Transaction broadcast failed"),
                    }
                }
                Err(e) => warn!(error = %e, "Transaction creation failed"),
            }
        }

        "bal" => {
            let wm    = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            if parts.len() == 2 {
                match wm.get_wallet(parts[1]) {
                    Some(w) => info!(wallet = parts[1], address = %w.address(), balance = chain.get_balance(w), "Wallet balance"),
                    None    => warn!(name = parts[1], "Wallet not found"),
                }
            } else {
                let w = wm.current_wallet();
                info!(wallet = %wm.selected, address = %w.address(), balance = chain.get_balance(w), "Active wallet balance");
            }
        }

        "newwallet" => {
            if parts.len() < 2 { warn!("Usage: newwallet <n>"); return; }
            let mut wm = wallet_manager.lock().unwrap();
            match wm.create_wallet(parts[1]) {
                Ok(addr) => info!(name = parts[1], address = %addr, "Wallet created"),
                Err(e)   => warn!(error = %e, "Wallet creation failed"),
            }
        }

        "selectwallet" => {
            if parts.len() < 2 { warn!("Usage: selectwallet <n>"); return; }
            let mut wm = wallet_manager.lock().unwrap();
            match wm.select_wallet(parts[1]) {
                Ok(_)  => info!(name = parts[1], "Active wallet changed"),
                Err(e) => warn!(error = %e, "Wallet selection failed"),
            }
        }

        "listwallets" => {
            let wm    = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            for name in wm.list_wallets() {
                let wallet  = wm.get_wallet(&name).unwrap();
                let balance = chain.get_balance(wallet);
                let active  = name == wm.selected;
                debug!(name, address = %wallet.address(), balance, active, "Wallet entry");
            }
            info!(count = wm.list_wallets().len(), "Wallets listed");
        }

        "exportwallet" => {
            if parts.len() < 2 { warn!("Usage: exportwallet <n> [wif|json]"); return; }
            let name   = parts[1];
            let format = if parts.len() == 3 { parts[2] } else { "json" };
            let wm     = wallet_manager.lock().unwrap();
            match wm.export_wallet(name, format) {
                Ok(_)  => info!(name, format, "Wallet exported"),
                Err(e) => warn!(error = %e, name, "Wallet export failed"),
            }
        }

        "importwallet" => {
            if parts.len() < 2 { warn!("Usage: importwallet <file> [name]"); return; }
            let file_path = parts[1];
            let name      = if parts.len() == 3 { Some(parts[2]) } else { None };
            let mut wm    = wallet_manager.lock().unwrap();
            match wm.import_wallet(file_path, name) {
                Ok(addr) => info!(address = %addr, file = file_path, "Wallet imported"),
                Err(e)   => warn!(error = %e, file = file_path, "Wallet import failed"),
            }
        }

        "mempool" => {
            let chain = blockchain.lock().unwrap();
            info!(count = chain.mempool.size(), total_fees = chain.mempool.total_fees(), "Mempool status");
            for entry in &chain.mempool.entries {
                let total_out: u64 = entry.tx.vout.iter().map(|o| o.value).sum();
                debug!(
                    tx_id       = %entry.tx.id,
                    inputs      = entry.tx.vin.len(),
                    outputs     = entry.tx.vout.len(),
                    total_value = total_out,
                    fee         = entry.fee,
                    "Mempool entry"
                );
            }
        }

        "diff" => {
            let chain = blockchain.lock().unwrap();
            let info  = chain.difficulty_info();
            info!(
                current_difficulty     = info.current,
                height                 = info.height,
                blocks_until_retarget  = info.blocks_until_retarget,
                avg_block_time_secs    = info.avg_block_time_secs,
                target_block_time_secs = info.target_block_time_secs,
                "Difficulty info"
            );
        }

        "peers" => {
            let peers: Vec<_> = swarm.connected_peers().cloned().collect();
            info!(count = peers.len(), "Connected peers");
            for p in &peers { debug!(peer_id = %p, "Peer"); }
        }

        "chain" => {
            let chain = blockchain.lock().unwrap();
            info!(height = chain.height(), difficulty = chain.difficulty, valid = chain.is_valid(), "Chain status");
            for block in &chain.chain {
                debug!(
                    index    = block.index,
                    hash     = %block.hash,
                    tx_count = block.transactions.len(),
                    diff     = block.difficulty,
                    reward   = block.reward,
                    "Block"
                );
            }
        }

        "sync" => {
            let chain = blockchain.lock().unwrap();
            let req   = NetworkMessage::GetBlocks(GetBlocks {
                from_hash:   chain.tip_hash(),
                from_height: chain.height(),
                nonce:       random_nonce(),
            });
            drop(chain);
            match swarm.behaviour_mut().gossipsub.publish(topic_blocks.clone(), req.serialize()) {
                Ok(_)  => info!("Sync request sent"),
                Err(e) => warn!(error = ?e, "Sync request failed"),
            }
        }

        "history" => {
            let wm    = wallet_manager.lock().unwrap();
            let chain = blockchain.lock().unwrap();
            let (label, pub_key_hash): (String, Vec<u8>) = if parts.len() == 2 {
                let arg = parts[1];
                if arg.len() == 64 {
                    match hex::decode(arg) {
                        Ok(hash) => (format!("{}…", &arg[..16]), hash),
                        Err(_)   => { warn!("Invalid address hex"); return; }
                    }
                } else {
                    match wm.get_wallet(arg) {
                        Some(w) => (arg.to_string(), w.pub_key_hash()),
                        None    => { warn!(name = arg, "Wallet not found"); return; }
                    }
                }
            } else {
                let w = wm.current_wallet();
                (wm.selected.clone(), w.pub_key_hash())
            };

            let records = crate::models::history::build_history(
                &chain.chain, &pub_key_hash, chain.height());

            if records.is_empty() {
                info!(wallet = label, "No transaction history");
                return;
            }

            let total_received: i64 = records.iter()
                .filter(|r| r.direction == crate::models::history::TxDirection::Received)
                .map(|r| r.net).sum();
            let total_sent: i64 = records.iter()
                .filter(|r| r.direction == crate::models::history::TxDirection::Sent)
                .map(|r| r.net).sum();
            let total_fees: u64 = records.iter().map(|r| r.fee).sum();

            info!(
                wallet         = label,
                tx_count       = records.len(),
                total_received,
                total_sent     = total_sent.abs(),
                total_fees,
                "Transaction history"
            );
            for r in &records {
                let dir = match r.direction {
                    crate::models::history::TxDirection::Received => "IN",
                    crate::models::history::TxDirection::Sent     => "OUT",
                    crate::models::history::TxDirection::Self_    => "SELF",
                };
                debug!(
                    tx_id         = %r.tx_id,
                    block_height  = r.block_height,
                    confirmations = r.confirmations,
                    net           = r.net,
                    fee           = r.fee,
                    direction     = dir,
                    "TX record"
                );
            }
        }

        other => {
            if !other.is_empty() { warn!(command = other, "Unknown command"); }
        }
    }
}