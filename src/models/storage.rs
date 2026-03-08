// Persistent block storage backed by sled — an embedded key-value store.
// Design mirrors Bitcoin Core's block index: each block stored by its index,
// with metadata (height, hash, prev_hash) recoverable on startup.

use sled::{Db};

use crate::models::block::Block;

const BLOCKS_TREE: &str = "blocks";
const META_TREE: &str = "meta";
const KEY_HEIGHT: &str = "chain_height";

pub struct BlockStore {
    db: Db,
}

impl BlockStore {
    /// Open or create the persistent store at the given path.
    pub fn open(path: &str) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        Ok(BlockStore { db })
    }

    /// Persist a block. Key = big-endian u64 index for ordered iteration.
    pub fn save_block(&self, block: &Block) -> Result<(), sled::Error> {
        let tree = self.db.open_tree(BLOCKS_TREE)?;
        let key = block.index.to_be_bytes();
        let value = serde_json::to_vec(block).expect("Block serializable");
        tree.insert(key, value)?;
        tree.flush()?;

        // Update persisted height
        let meta = self.db.open_tree(META_TREE)?;
        meta.insert(KEY_HEIGHT, &block.index.to_be_bytes())?;
        meta.flush()?;

        Ok(())
    }

    /// Load all blocks in order — returns empty vec if no data exists.
    pub fn load_chain(&self) -> Result<Vec<Block>, sled::Error> {
        let tree = self.db.open_tree(BLOCKS_TREE)?;
        let mut blocks = Vec::new();

        for item in tree.iter() {
            let (_, value) = item?;
            let block: Block =
                serde_json::from_slice(&value).expect("Block deserializable");
            blocks.push(block);
        }

        // sled iterates in key order (big-endian u64 = ascending index)
        blocks.sort_by_key(|b| b.index);
        Ok(blocks)
    }

    /// Return the persisted chain height, or None if store is empty.
    pub fn persisted_height(&self) -> Result<Option<u64>, sled::Error> {
        let meta = self.db.open_tree(META_TREE)?;
        match meta.get(KEY_HEIGHT)? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes[..8].try_into().unwrap();
                Ok(Some(u64::from_be_bytes(arr)))
            }
            None => Ok(None),
        }
    }
}
