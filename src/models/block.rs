use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::transaction::Transaction;

fn default_difficulty() -> usize { 2 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub index: u64,
    pub timestamp: u64,
    pub proof_of_work: u64,
    pub previous_hash: String,
    pub hash: String,
    pub transactions: Vec<Transaction>,
    pub reward: u64,
    #[serde(default = "default_difficulty")]
    pub difficulty: usize,
}

#[derive(Serialize)]
struct BlockHashable<'a> {
    index: u64,
    timestamp: u64,
    proof_of_work: u64,
    previous_hash: &'a str,
    transactions: &'a Vec<Transaction>,
    reward: u64,
    difficulty: usize,
}

impl Block {
    pub fn new(
        index: u64,
        previous_hash: String,
        transactions: Vec<Transaction>,
        reward: u64,
        difficulty: usize,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut block = Block {
            index,
            timestamp,
            proof_of_work: 0,
            previous_hash,
            hash: String::new(),
            transactions,
            reward,
            difficulty,
        };
        block.hash = block.calculate_hash();
        block
    }

    pub fn calculate_hash(&self) -> String {
        let hashable = BlockHashable {
            index: self.index,
            timestamp: self.timestamp,
            proof_of_work: self.proof_of_work,
            previous_hash: &self.previous_hash,
            transactions: &self.transactions,
            reward: self.reward,
            difficulty: self.difficulty,
        };
        let serialized = serde_json::to_string(&hashable).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn mine(&mut self, difficulty: usize) {
        let target = "0".repeat(difficulty);
        self.difficulty = difficulty;
        while !self.hash.starts_with(&target) {
            self.proof_of_work += 1;
            self.hash = self.calculate_hash();
        }
        println!(
            "Block {} mined: {} (nonce: {} | diff: {})",
            self.index, &self.hash[..16], self.proof_of_work, difficulty
        );
    }
}
