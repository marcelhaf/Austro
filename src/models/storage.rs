use sled::Db;
use tracing::{debug, instrument};

use crate::models::block::Block;

const BLOCKS_TREE: &str = "blocks";
const META_TREE:   &str = "meta";
const KEY_HEIGHT:  &str = "chain_height";

pub struct BlockStore {
    db: Db,
}

impl BlockStore {
    #[instrument(fields(path))]
    pub fn open(path: &str) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        debug!(path, "Block store opened");
        Ok(BlockStore { db })
    }

    #[instrument(skip(self, block), fields(height = block.index, hash = %block.hash))]
    pub fn save_block(&self, block: &Block) -> Result<(), sled::Error> {
        let tree  = self.db.open_tree(BLOCKS_TREE)?;
        let key   = block.index.to_be_bytes();
        let value = serde_json::to_vec(block).expect("Block serializable");
        tree.insert(key, value)?;
        tree.flush()?;

        let meta = self.db.open_tree(META_TREE)?;
        meta.insert(KEY_HEIGHT, &block.index.to_be_bytes())?;
        meta.flush()?;

        debug!(height = block.index, "Block persisted");
        Ok(())
    }

    #[instrument(skip(self))]
    pub fn load_chain(&self) -> Result<Vec<Block>, sled::Error> {
        let tree = self.db.open_tree(BLOCKS_TREE)?;
        let mut blocks = Vec::new();

        for item in tree.iter() {
            let (_, value) = item?;
            let block: Block = serde_json::from_slice(&value).expect("Block deserializable");
            blocks.push(block);
        }

        blocks.sort_by_key(|b| b.index);
        debug!(count = blocks.len(), "Chain loaded from disk");
        Ok(blocks)
    }

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