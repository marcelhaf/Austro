// Transaction history — analogous to Bitcoin Core's listtransactions RPC.
// Scans the full chain for every TX involving a given pub_key_hash,
// classifying each as sent, received, or self (change-only).

use serde::{Serialize};

use crate::models::block::Block;
use crate::models::transaction::{OutPoint, TXOutput, Transaction};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TxDirection {
    Received,
    Sent,
    Self_, // output goes back to same wallet (change-only TX)
}

#[derive(Debug, Clone, Serialize)]
pub struct TxRecord {
    pub tx_id: String,
    pub block_height: u64,
    pub confirmations: u64,
    pub direction: TxDirection,
    /// Net value change for this wallet:
    /// Received: +amount, Sent: -(amount+fee), Self: 0
    pub net: i64,
    pub fee: u64,
    pub timestamp: u64,
}

/// Scan the chain and return all TX records involving `pub_key_hash`.
pub fn build_history(
    chain: &[Block],
    pub_key_hash: &[u8],
    tip_height: u64,
) -> Vec<TxRecord> {
    // Build full spent-output map to compute fees
    let mut all_outputs: std::collections::HashMap<OutPoint, &TXOutput> =
        std::collections::HashMap::new();

    for block in chain {
        for tx in &block.transactions {
            for (idx, output) in tx.vout.iter().enumerate() {
                all_outputs.insert(
                    OutPoint { tx_id: tx.id.clone(), out_index: idx as u32 },
                    output,
                );
            }
        }
    }

    let mut records: Vec<TxRecord> = Vec::new();

    for block in chain {
        for tx in &block.transactions {
            if let Some(record) = classify_tx(
                tx,
                block.index,
                block.timestamp,
                tip_height,
                pub_key_hash,
                &all_outputs,
            ) {
                records.push(record);
            }
        }
    }

    // Most recent first — analogous to Bitcoin Core's default order
    records.sort_by(|a, b| b.block_height.cmp(&a.block_height));
    records
}

fn classify_tx(
    tx: &Transaction,
    block_height: u64,
    timestamp: u64,
    tip_height: u64,
    pub_key_hash: &[u8],
    all_outputs: &std::collections::HashMap<OutPoint, &TXOutput>,
) -> Option<TxRecord> {
    let confirmations = tip_height.saturating_sub(block_height) + 1;

    // Sum of outputs going TO this wallet
    let received: u64 = tx.vout.iter()
        .filter(|o| o.pub_key_hash == pub_key_hash)
        .map(|o| o.value)
        .sum();

    // For non-coinbase TXs, check if this wallet provided any inputs
    let wallet_spent: u64 = if tx.is_coinbase() {
        0
    } else {
        tx.vin.iter()
            .filter_map(|input| all_outputs.get(&input.previous_output))
            .filter(|o| o.pub_key_hash == pub_key_hash)
            .map(|o| o.value)
            .sum()
    };

    if received == 0 && wallet_spent == 0 {
        return None; // TX does not involve this wallet
    }

    // Compute fee: total_in - total_out (0 for coinbase)
    let fee: u64 = if tx.is_coinbase() {
        0
    } else {
        let total_in: u64 = tx.vin.iter()
            .filter_map(|i| all_outputs.get(&i.previous_output))
            .map(|o| o.value)
            .sum();
        let total_out: u64 = tx.vout.iter().map(|o| o.value).sum();
        total_in.saturating_sub(total_out)
    };

    let direction = if wallet_spent == 0 {
        // Only receiving — coinbase reward or incoming TX
        TxDirection::Received
    } else if received == wallet_spent {
        // All outputs back to self (e.g. pure self-transfer)
        TxDirection::Self_
    } else {
        TxDirection::Sent
    };

    let net: i64 = match direction {
        TxDirection::Received => received as i64,
        TxDirection::Sent => {
            // net = what we received back (change) - what we spent
            received as i64 - wallet_spent as i64
        }
        TxDirection::Self_ => 0,
    };

    Some(TxRecord {
        tx_id: tx.id.clone(),
        block_height,
        confirmations,
        direction,
        net,
        fee,
        timestamp,
    })
}
