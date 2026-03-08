use crate::models::block::Block;
use crate::models::transaction::Transaction;

pub const GENESIS_PUB_KEY_HASH: [u8; 32] = [
    0x4d, 0x69, 0x73, 0x65, 0x73, 0x41, 0x75, 0x73,
    0x74, 0x72, 0x6f, 0x46, 0x6f, 0x75, 0x6e, 0x64,
    0x69, 0x6e, 0x67, 0x42, 0x6c, 0x6f, 0x63, 0x6b,
    0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x31,
];

pub const GENESIS_REWARD: u64 = 50;
pub const GENESIS_DIFFICULTY: usize = 2;

const GENESIS_TIMESTAMP: u64 = 1741564800;

pub fn build() -> Block {
    let coinbase = Transaction::coinbase_genesis(&GENESIS_PUB_KEY_HASH, GENESIS_REWARD);

    let mut block = Block {
        index: 0,
        timestamp: GENESIS_TIMESTAMP,
        proof_of_work: 0,
        previous_hash: String::new(),
        hash: String::new(),
        transactions: vec![coinbase],
        reward: GENESIS_REWARD,
        difficulty: GENESIS_DIFFICULTY,
    };

    block.mine(GENESIS_DIFFICULTY);
    block
}
