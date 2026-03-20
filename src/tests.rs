use tempfile::TempDir;

use crate::models::block::Block;
use crate::models::blockchain::Blockchain;
use crate::models::difficulty::{
    calculate_next_difficulty, is_retarget_block, TARGET_TIMESPAN, RETARGET_INTERVAL,
};
use crate::models::genesis::{self, GENESIS_DIFFICULTY, GENESIS_REWARD};
use crate::models::history::{build_history, TxDirection};
use crate::models::mempool::Mempool;
use crate::models::storage::BlockStore;
use crate::models::transaction::{OutPoint, TXOutput, Transaction};
use crate::models::wallet::Wallet;
use crate::models::wallet_store::WalletManager;

fn temp_store() -> (BlockStore, TempDir) {
    let dir   = TempDir::new().expect("tempdir");
    let store = BlockStore::open(dir.path().to_str().unwrap()).expect("open store");
    (store, dir)
}

fn fresh_chain() -> (Blockchain, BlockStore, TempDir) {
    let (store, dir) = temp_store();
    let chain = Blockchain::new(&store);
    (chain, store, dir)
}

fn mine_block(chain: &mut Blockchain, store: &BlockStore, wallet: &Wallet) {
    chain.mine_pending_transactions(wallet, store);
}

fn funded_chain() -> (Blockchain, BlockStore, TempDir, Wallet) {
    let (mut chain, store, dir) = fresh_chain();
    let miner = Wallet::new();
    mine_block(&mut chain, &store, &miner);
    (chain, store, dir, miner)
}

#[cfg(test)]
mod wallet {
    use super::*;

    #[test]
    fn new_wallet_produces_unique_keypairs() {
        let a = Wallet::new();
        let b = Wallet::new();
        assert_ne!(a.address(), b.address());
    }

    #[test]
    fn address_is_64_char_hex() {
        let w    = Wallet::new();
        let addr = w.address();
        assert_eq!(addr.len(), 64);
        assert!(addr.chars().all(|c: char| c.is_ascii_hexdigit()));
    }

    #[test]
    fn pub_key_hash_matches_address() {
        let w = Wallet::new();
        assert_eq!(hex::encode(w.pub_key_hash()), w.address());
    }

    #[test]
    fn wif_roundtrip_mainnet() {
        let original = Wallet::new();
        let wif      = original.to_wif(false);
        let restored = Wallet::from_wif(&wif, false).expect("from_wif");
        assert_eq!(original.address(), restored.address());
    }

    #[test]
    fn wif_roundtrip_testnet() {
        let original = Wallet::new();
        let wif      = original.to_wif(true);
        let restored = Wallet::from_wif(&wif, true).expect("from_wif testnet");
        assert_eq!(original.address(), restored.address());
    }

    #[test]
    fn wif_wrong_network_rejected() {
        let w   = Wallet::new();
        let wif = w.to_wif(false);
        assert!(Wallet::from_wif(&wif, true).is_err());
    }

    #[test]
    fn json_roundtrip() {
        let original = Wallet::new();
        let json     = original.to_json();
        let restored = Wallet::from_json(&json).expect("from_json");
        assert_eq!(original.address(), restored.address());
    }

    #[test]
    fn sign_and_verify_message() {
        let wallet    = Wallet::new();
        let msg_bytes = [0xab_u8; 32];
        let sig       = wallet.sign_msg(&msg_bytes);
        assert!(!sig.is_empty());
    }
}

#[cfg(test)]
mod transaction {
    use super::*;

    #[test]
    fn coinbase_is_detected() {
        let w  = Wallet::new();
        let tx = Transaction::coinbase(&w.pub_key_hash(), 50);
        assert!(tx.is_coinbase());
    }

    #[test]
    fn genesis_coinbase_is_deterministic() {
        let tx1 = Transaction::coinbase_genesis(&[0u8; 32], 50);
        let tx2 = Transaction::coinbase_genesis(&[0u8; 32], 50);
        assert_eq!(tx1.id, tx2.id);
    }

    #[test]
    fn regular_coinbase_ids_differ_over_time() {
        let w   = Wallet::new();
        let tx1 = Transaction::coinbase(&w.pub_key_hash(), 50);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let tx2 = Transaction::coinbase(&w.pub_key_hash(), 50);
        assert_ne!(tx1.id, tx2.id);
    }

    #[test]
    fn coinbase_verify_basic() {
        let w  = Wallet::new();
        let tx = Transaction::coinbase(&w.pub_key_hash(), 50);
        assert!(tx.verify_basic());
    }

    #[test]
    fn coinbase_signatures_always_valid() {
        let w  = Wallet::new();
        let tx = Transaction::coinbase(&w.pub_key_hash(), 50);
        assert!(tx.verify_signatures());
    }

    #[test]
    fn signing_hash_excludes_signatures() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let (tx, _)  = chain
            .create_transaction(&miner, &receiver.pub_key_hash(), 10, 1)
            .expect("create tx");
        let hash_a = tx.signing_hash();
        let hash_b = tx.signing_hash();
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn signed_transaction_verifies() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let (tx, _)  = chain
            .create_transaction(&miner, &receiver.pub_key_hash(), 10, 1)
            .expect("create tx");
        assert!(tx.verify_signatures());
    }

    #[test]
    fn tampered_signature_fails_verification() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let (mut tx, _) = chain
            .create_transaction(&miner, &receiver.pub_key_hash(), 10, 1)
            .expect("create tx");
        tx.vin[0].signature[0] ^= 0xff;
        assert!(!tx.verify_signatures());
    }
}

#[cfg(test)]
mod block {
    use super::*;

    #[test]
    fn mine_produces_valid_hash() {
        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let mut block = Block::new(1, "prev".to_string(), vec![coinbase], 50, 1);
        block.mine(1);
        assert!(block.hash.starts_with('0'));
        assert_eq!(block.hash, block.calculate_hash());
    }

    #[test]
    fn calculate_hash_is_deterministic() {
        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let block    = Block::new(1, "prev".to_string(), vec![coinbase], 50, 1);
        assert_eq!(block.calculate_hash(), block.calculate_hash());
    }

    #[test]
    fn mutated_block_hash_changes() {
        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let mut block = Block::new(1, "prev".to_string(), vec![coinbase], 50, 1);
        let original  = block.calculate_hash();
        block.proof_of_work += 1;
        assert_ne!(block.calculate_hash(), original);
    }
}

#[cfg(test)]
mod difficulty {
    use super::*;

    #[test]
    fn no_change_on_target() {
        assert_eq!(calculate_next_difficulty(4, TARGET_TIMESPAN), 4);
    }

    #[test]
    fn increases_when_blocks_too_fast() {
        assert_eq!(calculate_next_difficulty(4, TARGET_TIMESPAN / 4), 16);
    }

    #[test]
    fn decreases_when_blocks_too_slow() {
        assert_eq!(calculate_next_difficulty(16, TARGET_TIMESPAN * 4), 4);
    }

    #[test]
    fn clamped_max_increase_is_4x() {
        assert_eq!(calculate_next_difficulty(4, TARGET_TIMESPAN / 100), 16);
    }

    #[test]
    fn clamped_max_decrease_is_4x() {
        assert_eq!(calculate_next_difficulty(16, TARGET_TIMESPAN * 100), 4);
    }

    #[test]
    fn never_drops_below_minimum() {
        assert_eq!(calculate_next_difficulty(1, TARGET_TIMESPAN * 100), 1);
    }

    #[test]
    fn retarget_fires_on_correct_heights() {
        assert!(!is_retarget_block(0));
        assert!(!is_retarget_block(1));
        assert!(!is_retarget_block(RETARGET_INTERVAL - 1));
        assert!(is_retarget_block(RETARGET_INTERVAL));
        assert!(!is_retarget_block(RETARGET_INTERVAL + 1));
        assert!(is_retarget_block(RETARGET_INTERVAL * 2));
    }
}

#[cfg(test)]
mod mempool {
    use super::*;

    fn unique_coinbase(n: u64) -> Transaction {
        let w = Wallet::new();
        let mut tx = Transaction::coinbase(&w.pub_key_hash(), 10 + n);
        tx.vin[0].previous_output.out_index = n as u32;
        tx
    }

    #[test]
    fn add_and_contains() {
        let mut pool = Mempool::new();
        let tx       = unique_coinbase(0);
        let id       = tx.id.clone();
        pool.add(tx, 5).expect("add ok");
        assert!(pool.contains(&id));
    }

    #[test]
    fn duplicate_rejected() {
        let mut pool = Mempool::new();
        let tx       = unique_coinbase(1);
        pool.add(tx.clone(), 5).expect("first add ok");
        assert!(pool.add(tx, 5).is_err());
    }

    #[test]
    fn sorted_by_fee_descending() {
        let mut pool = Mempool::new();
        pool.add(unique_coinbase(10), 1).unwrap();
        pool.add(unique_coinbase(20), 9).unwrap();
        pool.add(unique_coinbase(30), 5).unwrap();
        let fees: Vec<u64> = pool.entries.iter().map(|e| e.fee).collect();
        assert!(fees.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(pool.entries[0].fee, 9);
    }

    #[test]
    fn collect_for_block_respects_limit() {
        let mut pool = Mempool::new();
        for i in 0..10u64 {
            pool.add(unique_coinbase(i * 100), i).unwrap();
        }
        assert_eq!(pool.collect_for_block(3).len(), 3);
    }

    #[test]
    fn purge_confirmed_removes_txs() {
        let mut pool = Mempool::new();
        let tx       = unique_coinbase(99);
        let id       = tx.id.clone();
        pool.add(tx, 5).unwrap();
        pool.purge_confirmed(&[id.clone()]);
        assert!(!pool.contains(&id));
        assert_eq!(pool.size(), 0);
    }

    #[test]
    fn total_fees_sums_correctly() {
        let mut pool = Mempool::new();
        pool.add(unique_coinbase(200), 3).unwrap();
        pool.add(unique_coinbase(201), 7).unwrap();
        assert_eq!(pool.total_fees(), 10);
    }

    #[test]
    fn double_spend_via_reserved_inputs_rejected() {
        let w  = Wallet::new();
        let op = OutPoint { tx_id: "shared_utxo".to_string(), out_index: 0 };

        let make_tx = |id: &str| {
            let mut tx = Transaction::new_unsigned(
                vec![(op.clone(), w.public_key.serialize().to_vec())],
                vec![TXOutput { value: 5, pub_key_hash: w.pub_key_hash() }],
            );
            tx.id = id.to_string();
            tx
        };

        let mut pool = Mempool::new();
        pool.add(make_tx("spend1"), 1).expect("first spend ok");
        assert!(pool.add(make_tx("spend2"), 1).is_err());
    }
}

#[cfg(test)]
mod storage {
    use super::*;

    #[test]
    fn save_and_reload_single_block() {
        let (store, _dir) = temp_store();
        let genesis       = genesis::build();
        store.save_block(&genesis).expect("save");
        let loaded = store.load_chain().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].hash, genesis.hash);
    }

    #[test]
    fn load_preserves_order() {
        let (store, _dir) = temp_store();
        let genesis       = genesis::build();
        store.save_block(&genesis).expect("save genesis");

        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let mut b1   = Block::new(1, genesis.hash.clone(), vec![coinbase], 50, 1);
        b1.mine(1);
        store.save_block(&b1).expect("save b1");

        let chain = store.load_chain().expect("load");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].index, 0);
        assert_eq!(chain[1].index, 1);
    }

    #[test]
    fn persisted_height_reflects_last_save() {
        let (store, _dir) = temp_store();
        assert!(store.persisted_height().unwrap().is_none());
        let genesis = genesis::build();
        store.save_block(&genesis).expect("save");
        assert_eq!(store.persisted_height().unwrap(), Some(0));
    }

    #[test]
    fn reopen_store_retains_data() {
        let dir     = TempDir::new().unwrap();
        let path    = dir.path().to_str().unwrap().to_string();
        let genesis = genesis::build();
        {
            let store = BlockStore::open(&path).unwrap();
            store.save_block(&genesis).unwrap();
        }
        let store2 = BlockStore::open(&path).unwrap();
        let chain  = store2.load_chain().unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].hash, genesis.hash);
    }
}

#[cfg(test)]
mod genesis_block {
    use super::*;

    #[test]
    fn genesis_hash_satisfies_difficulty() {
        let g      = genesis::build();
        let target = "0".repeat(GENESIS_DIFFICULTY);
        assert!(g.hash.starts_with(&target));
    }

    #[test]
    fn genesis_is_deterministic() {
        let g1 = genesis::build();
        let g2 = genesis::build();
        assert_eq!(g1.hash, g2.hash);
    }

    #[test]
    fn genesis_reward_matches_constant() {
        let g = genesis::build();
        assert_eq!(g.reward, GENESIS_REWARD);
    }

    #[test]
    fn genesis_has_single_coinbase_tx() {
        let g = genesis::build();
        assert_eq!(g.transactions.len(), 1);
        assert!(g.transactions[0].is_coinbase());
    }

    #[test]
    fn genesis_previous_hash_is_empty() {
        let g = genesis::build();
        assert!(g.previous_hash.is_empty());
    }
}

#[cfg(test)]
mod blockchain_core {
    use super::*;

    #[test]
    fn new_chain_starts_at_height_zero() {
        let (chain, _store, _dir) = fresh_chain();
        assert_eq!(chain.height(), 0);
    }

    #[test]
    fn new_chain_is_valid() {
        let (chain, _store, _dir) = fresh_chain();
        assert!(chain.is_valid());
    }

    #[test]
    fn genesis_block_is_deterministic_across_instances() {
        let (c1, _s1, _d1) = fresh_chain();
        let (c2, _s2, _d2) = fresh_chain();
        assert_eq!(c1.chain[0].hash, c2.chain[0].hash);
    }

    #[test]
    fn tip_hash_matches_last_block() {
        let (chain, _store, _dir) = fresh_chain();
        assert_eq!(chain.tip_hash(), chain.chain.last().unwrap().hash);
    }

    #[test]
    fn utxo_set_includes_genesis_output() {
        let (chain, _store, _dir) = fresh_chain();
        assert_eq!(chain.build_utxo_set().len(), 1);
    }

    #[test]
    fn mine_increases_height() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        mine_block(&mut chain, &store, &w);
        assert_eq!(chain.height(), 1);
    }

    #[test]
    fn chain_remains_valid_after_mining() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        for _ in 0..3 { mine_block(&mut chain, &store, &w); }
        assert!(chain.is_valid());
    }

    #[test]
    fn miner_receives_coinbase_reward() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        mine_block(&mut chain, &store, &w);
        assert_eq!(chain.get_balance(&w), 50);
    }

    #[test]
    fn mined_block_satisfies_pow() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        mine_block(&mut chain, &store, &w);
        let block  = chain.chain.last().unwrap();
        let target = "0".repeat(block.difficulty);
        assert!(block.hash.starts_with(&target));
    }

    #[test]
    fn tampered_block_invalidates_chain() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        mine_block(&mut chain, &store, &w);
        chain.chain[1].proof_of_work += 1;
        assert!(!chain.is_valid());
    }

    #[test]
    fn chain_loaded_from_disk_matches_original() {
        let dir  = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let w    = Wallet::new();
        let tip_hash;
        {
            let store = BlockStore::open(&format!("{}/chain", path)).unwrap();
            let mut chain = Blockchain::new(&store);
            for _ in 0..3 { mine_block(&mut chain, &store, &w); }
            tip_hash = chain.tip_hash();
        }
        let store2 = BlockStore::open(&format!("{}/chain", path)).unwrap();
        let chain2 = Blockchain::new(&store2);
        assert_eq!(chain2.height(), 3);
        assert_eq!(chain2.tip_hash(), tip_hash);
        assert!(chain2.is_valid());
    }
}

#[cfg(test)]
mod blockchain_transactions {
    use super::*;

    #[test]
    fn create_transaction_adds_to_mempool() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).expect("create tx");
        assert_eq!(chain.mempool.size(), 1);
    }

    #[test]
    fn create_transaction_returns_correct_fee() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let (_, fee) = chain
            .create_transaction(&miner, &receiver.pub_key_hash(), 10, 3)
            .expect("create tx");
        assert_eq!(fee, 3);
    }

    #[test]
    fn insufficient_funds_returns_error() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        assert!(chain.create_transaction(&miner, &receiver.pub_key_hash(), 9999, 1).is_err());
    }

    #[test]
    fn transaction_cleared_from_mempool_after_mining() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).expect("create tx");
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.mempool.size(), 0);
    }

    #[test]
    fn receiver_balance_updated_after_confirmation() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).expect("create tx");
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.get_balance(&receiver), 10);
    }

    #[test]
    fn sender_balance_reduced_after_confirmation() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let before   = chain.get_balance(&miner);
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).expect("create tx");
        let miner2 = Wallet::new();
        mine_block(&mut chain, &store, &miner2);
        assert!(chain.get_balance(&miner) < before);
    }

    #[test]
    fn miner_collects_transaction_fee() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        let miner2   = Wallet::new();
        let fee      = 5u64;
        let amount   = 10u64;

        chain.create_transaction(&miner, &receiver.pub_key_hash(), amount, fee).expect("create tx");
        mine_block(&mut chain, &store, &miner2);

        assert_eq!(chain.get_balance(&receiver), amount, "receiver must get exact amount");
        assert_eq!(
            chain.get_balance(&miner2),
            50 + fee,
            "miner2 must collect base reward plus fee"
        );
        assert!(chain.is_valid());
    }

    #[test]
    fn change_is_returned_to_sender() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).expect("create tx");
        mine_block(&mut chain, &store, &miner);
        let change: u64 = chain.get_wallet_utxos(&miner).iter().map(|(_, o)| o.value).sum();
        assert!(change > 0);
    }

    #[test]
    fn double_spend_same_utxo_rejected() {
        let (mut chain, _store, _dir, miner) = funded_chain();
        let r1 = Wallet::new();
        let r2 = Wallet::new();
        chain.create_transaction(&miner, &r1.pub_key_hash(), 10, 1).expect("first tx ok");
        assert!(chain.create_transaction(&miner, &r2.pub_key_hash(), 10, 1).is_err());
    }

    #[test]
    fn invalid_transaction_rejected_by_validate() {
        let (chain, _store, _dir) = fresh_chain();
        let sender   = Wallet::new();
        let receiver = Wallet::new();
        let fake     = OutPoint { tx_id: "nonexistent".to_string(), out_index: 0 };
        let mut tx   = Transaction::new_unsigned(
            vec![(fake, sender.public_key.serialize().to_vec())],
            vec![TXOutput { value: 10, pub_key_hash: receiver.pub_key_hash() }],
        );
        tx.sign_inputs(&sender);
        assert!(!chain.validate_transaction(&tx));
    }

    #[test]
    fn multi_hop_transfer() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let alice = Wallet::new();
        let bob   = Wallet::new();

        chain.create_transaction(&miner, &alice.pub_key_hash(), 20, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.get_balance(&alice), 20);

        chain.create_transaction(&alice, &bob.pub_key_hash(), 10, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.get_balance(&bob), 10);
        assert_eq!(chain.get_balance(&alice), 9);
    }
}

#[cfg(test)]
mod blockchain_sync {
    use super::*;

    #[test]
    fn longer_valid_chain_replaces_shorter() {
        let w = Wallet::new();
        let (mut short_chain, short_store, _short_dir) = fresh_chain();
        mine_block(&mut short_chain, &short_store, &w);

        let (mut long_chain, long_store, _long_dir) = fresh_chain();
        for _ in 0..3 { mine_block(&mut long_chain, &long_store, &w); }

        let candidate = long_chain.chain.clone();
        assert!(short_chain.try_replace_chain(candidate, &short_store));
        assert_eq!(short_chain.height(), 3);
        assert!(short_chain.is_valid());
    }

    #[test]
    fn shorter_chain_does_not_replace_longer() {
        let w = Wallet::new();
        let (mut long_chain, long_store, _long_dir) = fresh_chain();
        for _ in 0..3 { mine_block(&mut long_chain, &long_store, &w); }

        let (mut short_chain, short_store, _short_dir) = fresh_chain();
        mine_block(&mut short_chain, &short_store, &w);

        let candidate    = short_chain.chain.clone();
        let original_tip = long_chain.tip_hash();
        assert!(!long_chain.try_replace_chain(candidate, &long_store));
        assert_eq!(long_chain.tip_hash(), original_tip);
    }

    #[test]
    fn chain_with_wrong_genesis_rejected() {
        let w = Wallet::new();
        let (mut chain_a, store_a, _dir_a) = fresh_chain();
        for _ in 0..3 { mine_block(&mut chain_a, &store_a, &w); }

        let (mut chain_b, store_b, _dir_b) = fresh_chain();
        for _ in 0..5 { mine_block(&mut chain_b, &store_b, &w); }
        chain_b.chain[0].proof_of_work += 999;

        assert!(!chain_a.try_replace_chain(chain_b.chain.clone(), &store_a));
    }

    #[test]
    fn append_block_with_wrong_height_rejected() {
        let (mut chain, store, _dir) = fresh_chain();
        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let mut block = Block::new(99, chain.tip_hash(), vec![coinbase], 50, chain.difficulty);
        block.mine(chain.difficulty);
        assert!(!chain.try_append_block(block, &store));
    }

    #[test]
    fn append_block_with_wrong_prev_hash_rejected() {
        let (mut chain, store, _dir) = fresh_chain();
        let w        = Wallet::new();
        let coinbase = Transaction::coinbase(&w.pub_key_hash(), 50);
        let mut block = Block::new(1, "wrong_prev".to_string(), vec![coinbase], 50, chain.difficulty);
        block.mine(chain.difficulty);
        assert!(!chain.try_append_block(block, &store));
    }

    #[test]
    fn equal_length_chain_does_not_replace() {
        let w = Wallet::new();
        let (mut chain_a, store_a, _dir_a) = fresh_chain();
        mine_block(&mut chain_a, &store_a, &w);

        let (mut chain_b, store_b, _dir_b) = fresh_chain();
        mine_block(&mut chain_b, &store_b, &w);

        let original_tip = chain_a.tip_hash();
        assert!(!chain_a.try_replace_chain(chain_b.chain.clone(), &store_a));
        assert_eq!(chain_a.tip_hash(), original_tip);
    }
}

#[cfg(test)]
mod blockchain_halving {
    use super::*;

    #[test]
    fn mining_reward_halves_at_210_blocks() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        assert_eq!(chain.mining_reward, 50);
        for _ in 0..210 { mine_block(&mut chain, &store, &w); }
        assert_eq!(chain.mining_reward, 25);
    }

    #[test]
    fn reward_does_not_halve_before_210() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        for _ in 0..208 { mine_block(&mut chain, &store, &w); }
        assert_eq!(chain.mining_reward, 50, "reward must still be 50 before block 210");
    }
}

#[cfg(test)]
mod history {
    use super::*;

    #[test]
    fn empty_history_for_unknown_address() {
        let (chain, _store, _dir) = fresh_chain();
        let stranger = Wallet::new();
        assert!(build_history(&chain.chain, &stranger.pub_key_hash(), chain.height()).is_empty());
    }

    #[test]
    fn coinbase_shows_as_received() {
        let (mut chain, store, _dir) = fresh_chain();
        let miner = Wallet::new();
        mine_block(&mut chain, &store, &miner);
        let records = build_history(&chain.chain, &miner.pub_key_hash(), chain.height());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].direction, TxDirection::Received);
        assert_eq!(records[0].net, 50);
    }

    #[test]
    fn sent_tx_shows_as_sent() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        let records = build_history(&chain.chain, &miner.pub_key_hash(), chain.height());
        let sent: Vec<_> = records.iter().filter(|r| r.direction == TxDirection::Sent).collect();
        assert!(!sent.is_empty());
        assert!(sent[0].net < 0);
    }

    #[test]
    fn received_tx_shows_correct_amount() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 15, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        let records = build_history(&chain.chain, &receiver.pub_key_hash(), chain.height());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].direction, TxDirection::Received);
        assert_eq!(records[0].net, 15);
    }

    #[test]
    fn confirmations_increase_with_depth() {
        let (mut chain, store, _dir) = fresh_chain();
        let miner = Wallet::new();
        for _ in 0..3 { mine_block(&mut chain, &store, &miner); }
        let records = build_history(&chain.chain, &miner.pub_key_hash(), chain.height());
        assert!(records.last().unwrap().confirmations >= 3);
    }

    #[test]
    fn history_ordered_most_recent_first() {
        let (mut chain, store, _dir) = fresh_chain();
        let miner = Wallet::new();
        for _ in 0..3 { mine_block(&mut chain, &store, &miner); }
        let records = build_history(&chain.chain, &miner.pub_key_hash(), chain.height());
        let heights: Vec<u64> = records.iter().map(|r| r.block_height).collect();
        assert!(heights.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn fee_recorded_correctly_in_history() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let receiver = Wallet::new();
        chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 4).unwrap();
        mine_block(&mut chain, &store, &miner);
        let records = build_history(&chain.chain, &miner.pub_key_hash(), chain.height());
        let sent    = records.iter().find(|r| r.direction == TxDirection::Sent).unwrap();
        assert_eq!(sent.fee, 4);
    }
}

#[cfg(test)]
mod wallet_manager {
    use super::*;

    fn temp_manager() -> (WalletManager, TempDir) {
        let dir     = TempDir::new().unwrap();
        let manager = WalletManager::new(dir.path().to_str().unwrap());
        (manager, dir)
    }

    #[test]
    fn default_wallet_created_on_first_run() {
        let (wm, _dir) = temp_manager();
        assert!(wm.get_wallet("default").is_some());
    }

    #[test]
    fn create_wallet_returns_valid_address() {
        let (mut wm, _dir) = temp_manager();
        let addr = wm.create_wallet("alice").expect("create");
        assert_eq!(addr.len(), 64);
    }

    #[test]
    fn duplicate_wallet_name_rejected() {
        let (mut wm, _dir) = temp_manager();
        wm.create_wallet("bob").unwrap();
        assert!(wm.create_wallet("bob").is_err());
    }

    #[test]
    fn select_wallet_changes_active() {
        let (mut wm, _dir) = temp_manager();
        wm.create_wallet("carol").unwrap();
        wm.select_wallet("carol").expect("select");
        assert_eq!(wm.selected, "carol");
    }

    #[test]
    fn select_nonexistent_wallet_fails() {
        let (mut wm, _dir) = temp_manager();
        assert!(wm.select_wallet("ghost").is_err());
    }

    #[test]
    fn list_wallets_includes_all_created() {
        let (mut wm, _dir) = temp_manager();
        wm.create_wallet("w1").unwrap();
        wm.create_wallet("w2").unwrap();
        let list = wm.list_wallets();
        assert!(list.contains(&"default".to_string()));
        assert!(list.contains(&"w1".to_string()));
        assert!(list.contains(&"w2".to_string()));
    }

    #[test]
    fn wallets_persisted_across_manager_instances() {
        let dir  = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let addr;
        {
            let mut wm = WalletManager::new(&path);
            addr = wm.create_wallet("persisted").unwrap();
        }
        let wm2 = WalletManager::new(&path);
        let w   = wm2.get_wallet("persisted").expect("must survive reload");
        assert_eq!(w.address(), addr);
    }

    #[test]
    fn export_and_import_json_roundtrip() {
        let dir  = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let mut wm   = WalletManager::new(&path);
        let original = wm.create_wallet("export_me").unwrap();
        wm.export_wallet("export_me", "json").unwrap();
        let json_path = format!("{}/wallets/export_me.json", path);
        let imported  = wm.import_wallet(&json_path, Some("imported")).unwrap();
        assert_eq!(original, imported);
    }

    #[test]
    fn export_and_import_wif_roundtrip() {
        let dir  = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let mut wm   = WalletManager::new(&path);
        let original = wm.create_wallet("wif_wallet").unwrap();
        wm.export_wallet("wif_wallet", "wif").unwrap();
        let wif_path = format!("{}/wallets/wif_wallet.wif", path);
        let imported = wm.import_wallet(&wif_path, Some("wif_imported")).unwrap();
        assert_eq!(original, imported);
    }
}

#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    fn full_payment_lifecycle() {
        let (mut chain, store, _dir, miner) = funded_chain();
        let alice = Wallet::new();
        let bob   = Wallet::new();

        assert_eq!(chain.get_balance(&miner), 50);

        chain.create_transaction(&miner, &alice.pub_key_hash(), 20, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.get_balance(&alice), 20);

        chain.create_transaction(&alice, &bob.pub_key_hash(), 8, 1).unwrap();
        mine_block(&mut chain, &store, &miner);
        assert_eq!(chain.get_balance(&bob), 8);
        assert_eq!(chain.get_balance(&alice), 11);
        assert!(chain.is_valid());
    }

    #[test]
    fn chain_survives_full_reload_with_transactions() {
        let dir      = TempDir::new().unwrap();
        let path     = dir.path().to_str().unwrap().to_string();
        let miner    = Wallet::new();
        let receiver = Wallet::new();
        {
            let store = BlockStore::open(&format!("{}/chain", path)).unwrap();
            let mut chain = Blockchain::new(&store);
            mine_block(&mut chain, &store, &miner);
            chain.create_transaction(&miner, &receiver.pub_key_hash(), 10, 1).unwrap();
            mine_block(&mut chain, &store, &miner);
        }
        let store2 = BlockStore::open(&format!("{}/chain", path)).unwrap();
        let chain2 = Blockchain::new(&store2);
        assert_eq!(chain2.height(), 2);
        assert!(chain2.is_valid());
        assert_eq!(chain2.get_balance(&receiver), 10);
    }

    #[test]
    fn total_supply_tracks_coinbase_emissions() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        for _ in 0..5 { mine_block(&mut chain, &store, &w); }
        let supply: u64 = chain.chain.iter()
            .flat_map(|b| b.transactions.iter().filter(|tx: &&Transaction| tx.is_coinbase()))
            .flat_map(|tx| tx.vout.iter())
            .map(|o| o.value)
            .sum();
        assert_eq!(supply, 50 * 6);
    }

    #[test]
    fn node_syncs_via_chain_replacement() {
        let w = Wallet::new();
        let (mut node_a, store_a, _dir_a) = fresh_chain();
        for _ in 0..5 { mine_block(&mut node_a, &store_a, &w); }

        let (mut node_b, store_b, _dir_b) = fresh_chain();
        mine_block(&mut node_b, &store_b, &w);

        let candidate = node_a.chain.clone();
        assert!(node_b.try_replace_chain(candidate, &store_b));
        assert_eq!(node_b.height(), 5);
        assert!(node_b.is_valid());
        assert_eq!(node_b.tip_hash(), node_a.tip_hash());
    }

    #[test]
    fn difficulty_info_returns_sensible_values() {
        let (mut chain, store, _dir) = fresh_chain();
        let w = Wallet::new();
        for _ in 0..3 { mine_block(&mut chain, &store, &w); }
        let info = chain.difficulty_info();
        assert!(info.current >= 1);
        assert_eq!(info.height, 3);
        assert!(info.blocks_until_retarget > 0);
        assert!(info.target_block_time_secs > 0);
    }
}