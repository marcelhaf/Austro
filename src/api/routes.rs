use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use tokio::sync::mpsc;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::models::blockchain::Blockchain;
use crate::models::wallet_store::WalletManager;
use crate::api::types::*;

#[derive(Clone)]
pub struct AppState {
    pub blockchain:     Arc<Mutex<Blockchain>>,
    pub wallet_manager: Arc<Mutex<WalletManager>>,
    pub cmd_tx:         mpsc::UnboundedSender<ApiCommand>,
    #[allow(dead_code)]
    pub node_peer_id:   String,
}

#[derive(Debug)]
pub enum ApiCommand {
    Mine,
    MineBlock { resp_tx: tokio::sync::oneshot::Sender<serde_json::Value> },
    StopMining,
    Send { to: Vec<u8>, amount: u64, fee: u64 },
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(
            DefaultMakeSpan::new()
                .level(Level::INFO)
                .include_headers(false),
        )
        .on_response(
            DefaultOnResponse::new()
                .level(Level::INFO)
                .include_headers(false),
        );

    Router::new()
        // ── existing endpoints ──────────────────────────────
        .route("/api/info",                    get(get_info))
        .route("/api/chain",                   get(get_chain))
        .route("/api/block/:hash",             get(get_block_by_hash))
        .route("/api/block/height/:n",         get(get_block_by_height))
        .route("/api/tx/:id",                  get(get_tx))
        .route("/api/address/:addr",           get(get_address))
        .route("/api/mempool",                 get(get_mempool))
        // ── wallet endpoints ────────────────────────────────
        .route("/api/wallets",                 get(get_wallets))
        .route("/api/wallet/create",           post(post_create_wallet))
        .route("/api/wallet/create-mnemonic",  post(post_create_mnemonic_wallet))
        .route("/api/wallet/recover",          post(post_recover_wallet))
        .route("/api/wallet/import",           post(post_import_wallet))
        .route("/api/wallet/export",           post(post_export_wallet))
        .route("/api/wallet/save",             post(post_save_wallet))
        .route("/api/wallet/load",             post(post_load_wallet))
        .route("/api/wallet/select/:name",     post(post_select_wallet))
        // ── send endpoint ───────────────────────────────────
        .route("/api/send",                    post(post_send))
        // ── mining endpoints ────────────────────────────────
        .route("/api/mine/start",              post(post_mine_start))
        .route("/api/mine/stop",               post(post_mine_stop))
        .route("/api/mine/status",             get(get_mine_status))
        .route("/api/mine/block",              post(post_mine_block))
        // ── peers endpoint ──────────────────────────────────
        .route("/api/peers",                   get(get_peers))
        // ── static files ────────────────────────────────────
        .nest_service("/", ServeDir::new("docs"))
        .layer(trace_layer)
        .layer(cors)
        .with_state(state)
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn ok(message: &str) -> Json<ApiSuccess> {
    Json(ApiSuccess { ok: true, message: message.to_string() })
}

fn err(code: StatusCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (code, Json(ApiError { error: message.to_string() }))
}

// ── existing handlers ─────────────────────────────────────────────────────────

async fn get_info(State(state): State<AppState>) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    let total_supply: u64 = chain.chain.iter()
        .flat_map(|b| b.transactions.iter().filter(|tx| tx.is_coinbase()))
        .flat_map(|tx| tx.vout.iter())
        .map(|o| o.value)
        .sum();
    Json(ApiChainInfo {
        height:             chain.height(),
        difficulty:         chain.difficulty,
        total_supply,
        pending_txs:        chain.mempool.size(),
        total_fees_pending: chain.mempool.total_fees(),
        is_valid:           chain.is_valid(),
    })
}

async fn get_chain(State(state): State<AppState>) -> impl IntoResponse {
    let chain  = state.blockchain.lock().unwrap();
    let blocks: Vec<ApiBlock> = chain.chain.iter().rev().map(block_to_api).collect();
    Json(blocks)
}

async fn get_block_by_hash(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    match chain.chain.iter().find(|b| b.hash == hash) {
        Some(b) => Json(block_to_api(b)).into_response(),
        None    => err(StatusCode::NOT_FOUND,
            &format!("Block '{}' not found", hash)).into_response(),
    }
}

async fn get_block_by_height(
    State(state): State<AppState>,
    Path(n): Path<u64>,
) -> impl IntoResponse {
    let chain = state.blockchain.lock().unwrap();
    match chain.chain.iter().find(|b| b.index == n) {
        Some(b) => Json(block_to_api(b)).into_response(),
        None    => err(StatusCode::NOT_FOUND,
            &format!("Block at height {} not found", n)).into_response(),
    }
}

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
                    let in_val:  u64 = tx.vin.iter()
                        .filter_map(|i| utxos.get(&i.previous_output))
                        .map(|o| o.value).sum();
                    let out_val: u64 = tx.vout.iter().map(|o| o.value).sum();
                    in_val.saturating_sub(out_val)
                };
                return Json(tx_to_api(tx, fee)).into_response();
            }
        }
    }

    for entry in &chain.mempool.entries {
        if entry.tx.id == id {
            return Json(tx_to_api(&entry.tx, entry.fee)).into_response();
        }
    }

    err(StatusCode::NOT_FOUND,
        &format!("TX '{}' not found", id)).into_response()
}

async fn get_address(
    State(state): State<AppState>,
    Path(addr): Path<String>,
) -> impl IntoResponse {
    let pub_key_hash = match hex::decode(&addr) {
        Ok(h) if h.len() == 32 => h,
        _ => return err(StatusCode::BAD_REQUEST,
            "Invalid address (expected 64-char hex)").into_response(),
    };

    let chain   = state.blockchain.lock().unwrap();
    let records = crate::models::history::build_history(
        &chain.chain, &pub_key_hash, chain.height());

    let balance: u64 = chain.build_utxo_set()
        .into_iter()
        .filter(|(_, o)| o.pub_key_hash == pub_key_hash)
        .map(|(_, o)| o.value)
        .sum();

    let transactions = records.iter().map(|r| ApiAddressTx {
        tx_id:         r.tx_id.clone(),
        block:         r.block_height,
        confirmations: r.confirmations,
        net:           r.net,
        fee:           r.fee,
        direction:     match r.direction {
            crate::models::history::TxDirection::Received => "in".to_string(),
            crate::models::history::TxDirection::Sent     => "out".to_string(),
            crate::models::history::TxDirection::Self_    => "self".to_string(),
        },
    }).collect();

    Json(ApiAddressInfo {
        address:  addr,
        balance,
        tx_count: records.len(),
        transactions,
    }).into_response()
}

async fn get_mempool(State(state): State<AppState>) -> impl IntoResponse {
    let chain        = state.blockchain.lock().unwrap();
    let transactions = chain.mempool.entries.iter()
        .map(|e| tx_to_api(&e.tx, e.fee))
        .collect();
    Json(ApiMempool {
        count:      chain.mempool.size(),
        total_fees: chain.mempool.total_fees(),
        transactions,
    })
}

// ── wallet handlers ───────────────────────────────────────────────────────────

async fn get_wallets(State(state): State<AppState>) -> impl IntoResponse {
    let wm    = state.wallet_manager.lock().unwrap();
    let chain = state.blockchain.lock().unwrap();

    let wallets: Vec<ApiWallet> = wm.list_wallets().into_iter().map(|name| {
        let wallet  = wm.get_wallet(&name).unwrap();
        let balance = chain.get_balance(wallet);
        ApiWallet {
            active:  name == wm.selected,
            address: wallet.address(),
            balance,
            name,
        }
    }).collect();

    Json(wallets)
}

async fn post_create_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqCreateWallet>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.create_wallet(&body.name) {
        Ok(addr) => Json(ApiSuccess { ok: true, message: addr }).into_response(),
        Err(e)   => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_create_mnemonic_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqCreateWallet>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.create_wallet_with_mnemonic(&body.name) {
        Ok((address, phrase)) => Json(ApiMnemonicResult {
            name: body.name,
            address,
            phrase,
        }).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_recover_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqRecoverWallet>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.recover_from_mnemonic(&body.name, &body.phrase) {
        Ok(addr) => ok(&addr).into_response(),
        Err(e)   => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_import_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqImportWallet>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.import_wallet(&body.path, Some(&body.name)) {
        Ok(addr) => ok(&addr).into_response(),
        Err(e)   => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_export_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqExportWallet>,
) -> impl IntoResponse {
    let wm = state.wallet_manager.lock().unwrap();
    match wm.export_wallet(&body.name, &body.format) {
        Ok(_)  => ok("Wallet exported").into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_save_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqSaveWallet>,
) -> impl IntoResponse {
    let wm = state.wallet_manager.lock().unwrap();
    match wm.save_encrypted(&body.name, &body.password) {
        Ok(_)  => ok("Wallet saved").into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_load_wallet(
    State(state): State<AppState>,
    Json(body): Json<ReqLoadWallet>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.load_encrypted(&body.name, &body.password) {
        Ok(addr) => ok(&addr).into_response(),
        Err(e)   => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

async fn post_select_wallet(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut wm = state.wallet_manager.lock().unwrap();
    match wm.select_wallet(&name) {
        Ok(_)  => ok(&format!("Selected wallet '{}'", name)).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, &e).into_response(),
    }
}

// ── send handler ──────────────────────────────────────────────────────────────

async fn post_send(
    State(state): State<AppState>,
    Json(body): Json<ReqSend>,
) -> impl IntoResponse {
    let to_hash = match hex::decode(&body.to) {
        Ok(h) if h.len() == 32 => h,
        _ => return err(StatusCode::BAD_REQUEST,
            "Invalid destination address (expected 64-char hex)").into_response(),
    };

    let from = state.wallet_manager.lock().unwrap()
        .current_wallet().clone();

    let mut chain = state.blockchain.lock().unwrap();
    match chain.create_transaction(&from, &to_hash, body.amount, body.fee) {
        Ok((tx, actual_fee)) => {
            let tx_id = tx.id.clone();
            drop(chain);
            let _ = state.cmd_tx.send(ApiCommand::Send {
                to:     to_hash,
                amount: body.amount,
                fee:    actual_fee,
            });
            Json(ApiSendResult { tx_id, fee: actual_fee }).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

// ── mining handlers ───────────────────────────────────────────────────────────

async fn post_mine_start(State(state): State<AppState>) -> impl IntoResponse {
    match state.cmd_tx.send(ApiCommand::Mine) {
        Ok(_)  => ok("Mining started").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to send mine command: {}", e)).into_response(),
    }
}

async fn post_mine_stop(State(state): State<AppState>) -> impl IntoResponse {
    match state.cmd_tx.send(ApiCommand::StopMining) {
        Ok(_)  => ok("Mining stopped").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to send stop command: {}", e)).into_response(),
    }
}

async fn get_mine_status(State(state): State<AppState>) -> impl IntoResponse {
    let wm      = state.wallet_manager.lock().unwrap();
    let address = wm.current_wallet().address();
    Json(ApiMiningStatus { mining: false, address })
}

async fn post_mine_block(State(state): State<AppState>) -> impl IntoResponse {
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

    if state.cmd_tx.send(ApiCommand::MineBlock { resp_tx }).is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR,
            "Node command channel unavailable").into_response();
    }

    match tokio::time::timeout(
        tokio::time::Duration::from_secs(120),
        resp_rx,
    ).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(_))     => err(StatusCode::INTERNAL_SERVER_ERROR,
            "Mining task dropped").into_response(),
        Err(_)         => err(StatusCode::REQUEST_TIMEOUT,
            "Mining timed out after 120s").into_response(),
    }
}

// ── peers handler ─────────────────────────────────────────────────────────────

async fn get_peers(State(state): State<AppState>) -> impl IntoResponse {
    let wm    = state.wallet_manager.lock().unwrap();
    let peers = wm.list_wallets();
    Json(ApiPeers {
        count: 0,
        peers: peers.into_iter()
            .map(|n| wm.get_wallet(&n).unwrap().address())
            .collect(),
    })
}

// ── conversion helpers ────────────────────────────────────────────────────────

fn block_to_api(block: &crate::models::block::Block) -> ApiBlock {
    let miner = block.transactions.iter()
        .find(|tx| tx.is_coinbase())
        .and_then(|tx| tx.vout.first())
        .map(|o| hex::encode(&o.pub_key_hash))
        .unwrap_or_default();

    ApiBlock {
        index:         block.index,
        hash:          block.hash.clone(),
        previous_hash: block.previous_hash.clone(),
        timestamp:     block.timestamp,
        nonce:         block.proof_of_work,
        difficulty:    block.difficulty,
        reward:        block.reward,
        tx_count:      block.transactions.len(),
        transactions:  block.transactions.iter().map(|tx| tx_to_api(tx, 0)).collect(),
        miner,
    }
}

fn tx_to_api(tx: &crate::models::transaction::Transaction, fee: u64) -> ApiTx {
    ApiTx {
        id:          tx.id.clone(),
        is_coinbase: tx.is_coinbase(),
        inputs:      tx.vin.iter().map(|i| ApiInput {
            tx_id:     i.previous_output.tx_id.clone(),
            out_index: i.previous_output.out_index,
        }).collect(),
        outputs:     tx.vout.iter().map(|o| ApiOutput {
            value:   o.value,
            address: hex::encode(&o.pub_key_hash),
        }).collect(),
        fee,
    }
}
