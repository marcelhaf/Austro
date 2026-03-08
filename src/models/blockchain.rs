use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use crate::models::block::Block;
use crate::models::difficulty::{
    calculate_next_difficulty, is_retarget_block, RETARGET_INTERVAL, TARGET_TIMESPAN,
};
use crate::models::genesis;
use crate::models::mempool::Mempool;
use crate::models::storage::BlockStore;
use crate::models::transaction::{OutPoint, Transaction, TXOutput};
use crate::models::wallet::Wallet;

const MAX_TXS_PER_BLOCK: usize = 500;

#[derive(Debug, Clone)]
pub struct Blockchain {
    pub chain: Vec<Block>,
    pub difficulty: usize,
    pub mempool: Mempool,
    pub mining_reward: u64,
}

impl Blockchain {
    pub fn new(store: &BlockStore) -> Self {
        let persisted = store.load_chain().unwrap_or_default();

        let (chain, difficulty) = if persisted.is_empty() {
            let genesis_block = genesis::build();
            println!("Genesis: {}", &genesis_block.hash[..16]);
            store.save_block(&genesis_block).expect("Persist genesis");
            let diff = genesis_block.difficulty;
            (vec![genesis_block], diff)
        } else {
            let diff = persisted.last().unwrap().difficulty;
            println!(
                "Loaded {} blocks from disk (tip: {} | diff: {})",
                persisted.len(),
                &persisted.last().unwrap().hash[..16],
                diff
            );
            (persisted, diff)
        };

        Blockchain {
            chain,
            difficulty,
            mempool: Mempool::new(),
            mining_reward: 50,
        }
    }

    pub fn height(&self) -> u64 {
        self.chain.last().map(|b| b.index).unwrap_or(0)
    }

    pub fn tip_hash(&self) -> String {
        self.chain.last().map(|b| b.hash.clone()).unwrap_or_default()
    }

    // ── Difficulty ────────────────────────────────────────────────────────

    fn compute_current_difficulty(&self) -> usize {
        let next_height = self.chain.len() as u64;
        if !is_retarget_block(next_height) {
            return self.difficulty;
        }

        let window_start_idx = (next_height - RETARGET_INTERVAL) as usize;
        let first_block = &self.chain[window_start_idx];
        let last_block = self.chain.last().unwrap();
        let actual_timespan = last_block.timestamp
            .saturating_sub(first_block.timestamp)
            .max(1);

        let new_difficulty = calculate_next_difficulty(self.difficulty, actual_timespan);

        println!("── Difficulty retarget at block {} ──", next_height);
        println!(
            "   Timespan: {}s actual | {}s target",
            actual_timespan, TARGET_TIMESPAN
        );
        println!("   Difficulty: {} → {}", self.difficulty, new_difficulty);

        new_difficulty
    }

    // ── UTXO ──────────────────────────────────────────────────────────────

    pub fn build_utxo_set(&self) -> HashMap<OutPoint, TXOutput> {
        let mut spent: HashSet<OutPoint> = HashSet::new();
        let mut utxos: HashMap<OutPoint, TXOutput> = HashMap::new();

        for block in &self.chain {
            for tx in &block.transactions {
                if !tx.is_coinbase() {
                    for input in &tx.vin {
                        spent.insert(input.previous_output.clone());
                    }
                }
            }
        }
        for block in &self.chain {
            for tx in &block.transactions {
                for (idx, output) in tx.vout.iter().enumerate() {
                    let op = OutPoint { tx_id: tx.id.clone(), out_index: idx as u32 };
                    if !spent.contains(&op) {
                        utxos.insert(op, output.clone());
                    }
                }
            }
        }
        utxos
    }

    // ── Validation ────────────────────────────────────────────────────────

    fn validate_against(tx: &Transaction, utxos: &HashMap<OutPoint, TXOutput>) -> bool {
        if tx.is_coinbase() { return tx.verify_basic(); }
        if tx.vin.is_empty() || !tx.verify_basic() { return false; }
        if !tx.verify_signatures() { return false; }

        let mut seen: HashSet<OutPoint> = HashSet::new();
        let mut input_total = 0u64;

        for input in &tx.vin {
            if !seen.insert(input.previous_output.clone()) { return false; }
            let referenced = match utxos.get(&input.previous_output) {
                Some(o) => o,
                None => return false,
            };
            let mut hasher = Sha256::new();
            hasher.update(&input.pub_key);
            if hasher.finalize().to_vec() != referenced.pub_key_hash { return false; }
            input_total += referenced.value;
        }

        let output_total: u64 = tx.vout.iter().map(|o| o.value).sum();
        input_total >= output_total
    }

    pub fn validate_transaction(&self, tx: &Transaction) -> bool {
        Self::validate_against(tx, &self.build_utxo_set())
    }

    pub fn calculate_fee(&self, tx: &Transaction) -> Option<u64> {
        if tx.is_coinbase() { return Some(0); }
        let utxos = self.build_utxo_set();
        let input_total: Option<u64> = tx.vin.iter()
            .map(|i| utxos.get(&i.previous_output).map(|o| o.value))
            .sum();
        let input_total = input_total?;
        let output_total: u64 = tx.vout.iter().map(|o| o.value).sum();
        if input_total >= output_total { Some(input_total - output_total) } else { None }
    }

    pub fn try_append_block(&mut self, block: Block, store: &BlockStore) -> bool {
        if block.index != self.chain.len() as u64 { return false; }
        if block.previous_hash != self.tip_hash() { return false; }
        if block.hash != block.calculate_hash() { return false; }
        if !block.hash.starts_with(&"0".repeat(block.difficulty)) { return false; }

        let expected_diff = self.compute_current_difficulty();
        if block.difficulty != expected_diff { return false; }

        let mut utxos = self.build_utxo_set();
        for tx in &block.transactions {
            if !tx.is_coinbase() && !Self::validate_against(tx, &utxos) { return false; }
            for input in &tx.vin { utxos.remove(&input.previous_output); }
            for (idx, output) in tx.vout.iter().enumerate() {
                utxos.insert(
                    OutPoint { tx_id: tx.id.clone(), out_index: idx as u32 },
                    output.clone(),
                );
            }
        }

        self.difficulty = block.difficulty;
        store.save_block(&block).expect("Persist block");
        self.chain.push(block);
        true
    }

    pub fn try_replace_chain(&mut self, candidate: Vec<Block>, store: &BlockStore) -> bool {
        if candidate.len() <= self.chain.len() { return false; }

        if candidate[0].hash != self.chain[0].hash {
            println!("Sync rejected: genesis mismatch");
            return false;
        }

        let mut utxos: HashMap<OutPoint, TXOutput> = HashMap::new();
        let mut current_diff = genesis::GENESIS_DIFFICULTY;

        for i in 0..candidate.len() {
            let block = &candidate[i];
            if block.hash != block.calculate_hash() {
                println!("Sync rejected: block {} invalid hash", i);
                return false;
            }
            if i > 0 {
                if block.previous_hash != candidate[i - 1].hash {
                    println!("Sync rejected: block {} broken chain link", i);
                    return false;
                }
                if !block.hash.starts_with(&"0".repeat(block.difficulty)) {
                    println!("Sync rejected: block {} insufficient PoW", i);
                    return false;
                }
            }
            for tx in &block.transactions {
                if !tx.is_coinbase() && !Self::validate_against(tx, &utxos) {
                    println!("Sync rejected: block {} invalid TX", i);
                    return false;
                }
                for input in &tx.vin { utxos.remove(&input.previous_output); }
                for (idx, output) in tx.vout.iter().enumerate() {
                    utxos.insert(
                        OutPoint { tx_id: tx.id.clone(), out_index: idx as u32 },
                        output.clone(),
                    );
                }
            }
            current_diff = block.difficulty;
        }

        println!("Chain reorg: {} → {} blocks", self.chain.len(), candidate.len());
        for block in &candidate[self.chain.len()..] {
            store.save_block(block).expect("Persist synced block");
        }
        self.chain = candidate;
        self.difficulty = current_diff;
        true
    }


    // ── Wallet ────────────────────────────────────────────────────────────

    pub fn get_wallet_utxos(&self, wallet: &Wallet) -> Vec<(OutPoint, TXOutput)> {
        let wallet_hash = wallet.pub_key_hash();
        let reserved = &self.mempool.reserved_inputs;
        self.build_utxo_set()
            .into_iter()
            .filter(|(op, output)| {
                output.pub_key_hash == wallet_hash && !reserved.contains(op)
            })
            .collect()
    }

    pub fn get_balance(&self, wallet: &Wallet) -> u64 {
        self.get_wallet_utxos(wallet).iter().map(|(_, o)| o.value).sum()
    }

    // ── Transactions ──────────────────────────────────────────────────────

    pub fn create_transaction(
        &mut self,
        from: &Wallet,
        to_pub_key_hash: &[u8],
        amount: u64,
        fee: u64,
    ) -> Result<(Transaction, u64), String> {
        let total_needed = amount.checked_add(fee).ok_or("Overflow")?;

        let mut available = self.get_wallet_utxos(from);
        available.sort_by(|a, b| a.1.value.cmp(&b.1.value));

        let mut selected = Vec::new();
        let mut total_in = 0u64;
        for (op, output) in available {
            total_in += output.value;
            selected.push((op, output));
            if total_in >= total_needed { break; }
        }

        if total_in < total_needed {
            return Err(format!(
                "Insufficient funds: have={} need={} (amount={} + fee={})",
                total_in, total_needed, amount, fee
            ));
        }

        let inputs: Vec<(OutPoint, Vec<u8>)> = selected.iter()
            .map(|(op, _)| (op.clone(), from.public_key.serialize().to_vec()))
            .collect();

        let mut outputs = vec![TXOutput { value: amount, pub_key_hash: to_pub_key_hash.to_vec() }];
        let change = total_in - amount - fee;
        if change > 0 {
            outputs.push(TXOutput { value: change, pub_key_hash: from.pub_key_hash() });
        }

        let actual_fee = total_in - amount - change;
        let mut tx = Transaction::new_unsigned(inputs, outputs);
        tx.sign_inputs(from);

        if !self.validate_transaction(&tx) {
            return Err("Transaction failed validation".to_string());
        }

        self.mempool.add(tx.clone(), actual_fee)?;
        Ok((tx, actual_fee))
    }

    // ── Mining ────────────────────────────────────────────────────────────

    pub fn mine_pending_transactions(&mut self, miner_wallet: &Wallet, store: &BlockStore) {
        let next_difficulty = self.compute_current_difficulty();
        self.difficulty = next_difficulty;

        let pending = self.mempool.collect_for_block(MAX_TXS_PER_BLOCK);
        let total_fees: u64 = self.mempool.entries.iter()
            .take(MAX_TXS_PER_BLOCK).map(|e| e.fee).sum();

        let coinbase_value = self.mining_reward + total_fees;
        let reward_tx = Transaction::coinbase(&miner_wallet.pub_key_hash(), coinbase_value);

        let mut transactions = vec![reward_tx];
        transactions.extend(pending.clone());

        if transactions.len() == 1 {
            println!("Mempool is empty, mining reward-only block");
        }

        let mut block = Block::new(
            self.chain.len() as u64,
            self.tip_hash(),
            transactions,
            coinbase_value,
            next_difficulty,
        );

        block.mine(next_difficulty);
        store.save_block(&block).expect("Persist mined block");
        self.chain.push(block);

        let confirmed: Vec<String> = pending.iter().map(|tx| tx.id.clone()).collect();
        self.mempool.purge_confirmed(&confirmed);

        if total_fees > 0 {
            println!("Fees collected: {} | Total reward: {} AUSTRO", total_fees, coinbase_value);
        }

        if self.chain.len() % 210 == 0 && self.mining_reward > 1 {
            self.mining_reward /= 2;
            println!("Halving! New base reward: {} AUSTRO", self.mining_reward);
        }

        println!("Mempool size after mining: {}", self.mempool.size());
    }

    // ── Difficulty info ───────────────────────────────────────────────────

    pub fn difficulty_info(&self) -> DifficultyInfo {
        let h = self.chain.len() as u64;
        let blocks_since = h % RETARGET_INTERVAL;
        let blocks_until_retarget = if blocks_since == 0 {
            RETARGET_INTERVAL
        } else {
            RETARGET_INTERVAL - blocks_since
        };

        // Exclui o bloco 0 (genesis com timestamp fixo de 2025) da janela
        // para evitar avg_block_time absurdo ao comparar com blocos de 2026.
        let avg_block_time = if self.chain.len() > 2 {
            let window = (self.chain.len() - 1).min(10);
            let start_idx = self.chain.len() - window;
            let first = &self.chain[start_idx];
            let last = self.chain.last().unwrap();
            let elapsed = last.timestamp.saturating_sub(first.timestamp);
            if window > 1 { elapsed / (window as u64 - 1) } else { 0 }
        } else {
            0
        };

        DifficultyInfo {
            current: self.difficulty,
            height: self.height(),
            blocks_until_retarget,
            avg_block_time_secs: avg_block_time,
            target_block_time_secs: crate::models::difficulty::TARGET_BLOCK_TIME_SECS,
        }
    }

    // ── History ───────────────────────────────────────────────────────────

    pub fn get_history(&self, wallet: &Wallet) -> Vec<crate::models::history::TxRecord> {
        crate::models::history::build_history(
            &self.chain,
            &wallet.pub_key_hash(),
            self.height(),
        )
    }

    // ── Integrity ─────────────────────────────────────────────────────────

    pub fn is_valid(&self) -> bool {
        if self.chain.is_empty() { return true; }

        for i in 0..self.chain.len() {
            let block = &self.chain[i];

            let computed = block.calculate_hash();
            if block.hash != computed {
                println!(
                    "INVALID: block {} hash mismatch\n  stored  : {}\n  computed: {}",
                    i, block.hash, computed
                );
                return false;
            }

            if i == 0 {
                if !block.previous_hash.is_empty() {
                    println!("INVALID: genesis previous_hash not empty");
                    return false;
                }
            } else {
                if block.previous_hash != self.chain[i - 1].hash {
                    println!("INVALID: block {} previous_hash mismatch", i);
                    return false;
                }
                if !block.hash.starts_with(&"0".repeat(block.difficulty)) {
                    println!("INVALID: block {} PoW insufficient (diff={})", i, block.difficulty);
                    return false;
                }
            }
        }
        true
    }
}

pub struct DifficultyInfo {
    pub current: usize,
    pub height: u64,
    pub blocks_until_retarget: u64,
    pub avg_block_time_secs: u64,
    pub target_block_time_secs: u64,
}
