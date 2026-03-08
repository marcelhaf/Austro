use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::models::blockchain::Blockchain;
use crate::api::types::*;

#[derive(Clone)]
pub struct AppState {
    pub blockchain: Arc<Mutex<Blockchain>>,
    pub node_peer_id: String,
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Chain info
        .route("/api/info",           get(get_info))
        .route("/api/chain",          get(get_chain))
        .route("/api/block/:hash",    get(get_block_by_hash))
        .route("/api/block/height/:n",get(get_block_by_height))
        .route("/api/tx/:id",         get(get_tx))
        .route("/api/address/:addr",  get(get_address))
        .route("/api/mempool",        get(get_mempool))
        // Serve the static explorer UI
        .nest_service("/", ServeDir::new("docs"))
        .layer(cors)
        .with_state(state)
}

// ── /api/info ─────────────────────────────────────────────────────────────────

async fn get_info(State(state): State<AppState>) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    let total_supply: u64 = chain.chain.iter()
        .flat_map(|b| b.transactions.iter().filter(|tx| tx.is_coinbase()))
        .flat_map(|tx| tx.vout.iter())
        .map(|o| o.value)
        .sum();
    Json(ApiChainInfo {
        height: chain.height(),
        difficulty: chain.difficulty,
        total_supply,
        pending_txs: chain.mempool.size(),
        total_fees_pending: chain.mempool.total_fees(),
        is_valid: chain.is_valid(),
    })
}

// ── /api/chain ────────────────────────────────────────────────────────────────

async fn get_chain(State(state): State<AppState>) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    let blocks: Vec<ApiBlock> = chain.chain.iter().rev().map(block_to_api).collect();
    Json(blocks)
}

// ── /api/block/:hash ──────────────────────────────────────────────────────────

async fn get_block_by_hash(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    match chain.chain.iter().find(|b| b.hash == hash) {
        Some(b) => Json(block_to_api(b)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiError { error: format!("Block '{}' not found", hash) }),
        ).into_response(),
    }
}

// ── /api/block/height/:n ──────────────────────────────────────────────────────

async fn get_block_by_height(
    State(state): State<AppState>,
    Path(n): Path<u64>,
) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    match chain.chain.iter().find(|b| b.index == n) {
        Some(b) => Json(block_to_api(b)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiError { error: format!("Block at height {} not found", n) }),
        ).into_response(),
    }
}

// ── /api/tx/:id ───────────────────────────────────────────────────────────────

async fn get_tx(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    let utxos = chain.build_utxo_set();

    for block in &chain.chain {
        for tx in &block.transactions {
            if tx.id == id {
                let fee = if tx.is_coinbase() {
                    0
                } else {
                    let in_val: u64 = tx.vin.iter()
                        .filter_map(|i| utxos.get(&i.previous_output))
                        .map(|o| o.value)
                        .sum();
                    let out_val: u64 = tx.vout.iter().map(|o| o.value).sum();
                    in_val.saturating_sub(out_val)
                };
                return Json(tx_to_api(tx, fee)).into_response();
            }
        }
    }

    // Check mempool
    for entry in &chain.mempool.entries {
        if entry.tx.id == id {
            return Json(tx_to_api(&entry.tx, entry.fee)).into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(ApiError { error: format!("TX '{}' not found", id) }),
    ).into_response()
}

// ── /api/address/:addr ────────────────────────────────────────────────────────

async fn get_address(
    State(state): State<AppState>,
    Path(addr): Path<String>,
) -> impl IntoResponse {
    let pub_key_hash = match hex::decode(&addr) {
        Ok(h) if h.len() == 32 => h,
        _ => return (
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "Invalid address (expected 64-char hex)".to_string() }),
        ).into_response(),
    };

    let chain = state.blockchain.lock().unwrap();
    let records = crate::models::history::build_history(
        &chain.chain, &pub_key_hash, chain.height());

    let balance: u64 = chain.build_utxo_set()
        .into_iter()
        .filter(|(_, o)| o.pub_key_hash == pub_key_hash)
        .map(|(_, o)| o.value)
        .sum();

    let transactions = records.iter().map(|r| ApiAddressTx {
        tx_id: r.tx_id.clone(),
        block: r.block_height,
        confirmations: r.confirmations,
        net: r.net,
        fee: r.fee,
        direction: match r.direction {
            crate::models::history::TxDirection::Received => "in".to_string(),
            crate::models::history::TxDirection::Sent     => "out".to_string(),
            crate::models::history::TxDirection::Self_    => "self".to_string(),
        },
    }).collect();

    Json(ApiAddressInfo {
        address: addr,
        balance,
        tx_count: records.len(),
        transactions,
    }).into_response()
}

// ── /api/mempool ──────────────────────────────────────────────────────────────

async fn get_mempool(State(state): State<AppState>) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    let transactions = chain.mempool.entries.iter()
        .map(|e| tx_to_api(&e.tx, e.fee))
        .collect();
    Json(ApiMempool {
        count: chain.mempool.size(),
        total_fees: chain.mempool.total_fees(),
        transactions,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn block_to_api(block: &crate::models::block::Block) -> ApiBlock {
    let miner = block.transactions.iter()
        .find(|tx| tx.is_coinbase())
        .and_then(|tx| tx.vout.first())
        .map(|o| hex::encode(&o.pub_key_hash))
        .unwrap_or_default();

    let txs = block.transactions.iter().map(|tx| tx_to_api(tx, 0)).collect();

    ApiBlock {
        index: block.index,
        hash: block.hash.clone(),
        previous_hash: block.previous_hash.clone(),
        timestamp: block.timestamp,
        nonce: block.proof_of_work,
        difficulty: block.difficulty,
        reward: block.reward,
        tx_count: block.transactions.len(),
        transactions: txs,
        miner,
    }
}

fn tx_to_api(tx: &crate::models::transaction::Transaction, fee: u64) -> ApiTx {
    ApiTx {
        id: tx.id.clone(),
        is_coinbase: tx.is_coinbase(),
        inputs: tx.vin.iter().map(|i| ApiInput {
            tx_id: i.previous_output.tx_id.clone(),
            out_index: i.previous_output.out_index,
        }).collect(),
        outputs: tx.vout.iter().map(|o| ApiOutput {
            value: o.value,
            address: hex::encode(&o.pub_key_hash),
        }).collect(),
        fee,
    }
}
