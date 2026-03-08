// Mempool with fee-based priority — higher fee TXs are collected first,
// analogous to Bitcoin's fee-rate ordering for block template construction.

use std::collections::HashSet;

use crate::models::transaction::{OutPoint, Transaction};

#[derive(Debug, Clone)]
pub struct MempoolEntry {
    pub tx: Transaction,
    pub fee: u64,
}

#[derive(Debug, Clone)]
pub struct Mempool {
    pub entries: Vec<MempoolEntry>,
    pub reserved_inputs: HashSet<OutPoint>,
}

impl Mempool {
    pub fn new() -> Self {
        Mempool {
            entries: Vec::new(),
            reserved_inputs: HashSet::new(),
        }
    }

    pub fn contains(&self, tx_id: &str) -> bool {
        self.entries.iter().any(|e| e.tx.id == tx_id)
    }

    pub fn add(&mut self, tx: Transaction, fee: u64) -> Result<(), String> {
        if self.contains(&tx.id) {
            return Err("Duplicate TX".to_string());
        }

        for input in &tx.vin {
            if self.reserved_inputs.contains(&input.previous_output) {
                return Err("Double-spend detected".to_string());
            }
        }

        for input in &tx.vin {
            self.reserved_inputs.insert(input.previous_output.clone());
        }

        self.entries.push(MempoolEntry { tx, fee });

        // Sort by fee descending — highest fee TXs included first
        self.entries.sort_by(|a, b| b.fee.cmp(&a.fee));

        Ok(())
    }

    /// Returns up to `limit` highest-fee TXs for block construction.
    pub fn collect_for_block(&self, limit: usize) -> Vec<Transaction> {
        self.entries.iter()
            .take(limit)
            .map(|e| e.tx.clone())
            .collect()
    }

    pub fn purge_confirmed(&mut self, confirmed_ids: &[String]) {
        let id_set: HashSet<&String> = confirmed_ids.iter().collect();

        for entry in self.entries.iter().filter(|e| id_set.contains(&e.tx.id)) {
            for input in &entry.tx.vin {
                self.reserved_inputs.remove(&input.previous_output);
            }
        }

        self.entries.retain(|e| !id_set.contains(&e.tx.id));
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn total_fees(&self) -> u64 {
        self.entries.iter().map(|e| e.fee).sum()
    }

    pub fn pending_txs(&self) -> Vec<Transaction> {
        self.entries.iter().map(|e| e.tx.clone()).collect()
    }
}
