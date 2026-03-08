use serde::{Deserialize, Serialize};

use crate::models::block::Block;
use crate::models::transaction::Transaction;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlocks {
    pub from_hash: String,
    pub from_height: u64,
    pub nonce: u64, // breaks gossipsub deduplication
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlocksResponse {
    pub blocks: Vec<Block>,
    pub nonce: u64, // breaks gossipsub deduplication
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum NetworkMessage {
    NewBlock(Block),
    NewTx(Transaction),
    GetBlocks(GetBlocks),
    BlocksBatch(BlocksResponse),
    GetMempool,
    MempoolTxs(Vec<Transaction>),
}

impl NetworkMessage {
    pub fn serialize(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("NetworkMessage serializable")
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        serde_json::from_slice(data).ok()
    }
}

pub fn random_nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    // XOR with stack address to increase entropy
    let stack_var: u64 = 0;
    let addr = &stack_var as *const u64 as u64;
    nanos as u64 ^ addr
}
