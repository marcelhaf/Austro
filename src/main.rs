mod api;
mod models;
mod network;

use std::sync::{Arc, Mutex};

use models::blockchain::Blockchain;
use models::storage::BlockStore;
use models::wallet_store::WalletManager;

pub struct NodeConfig {
    pub data_dir: String,
    pub port: u16,
    pub explorer_port: u16,
    pub bootstrap_peers: Vec<String>,
}

impl NodeConfig {
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let data_dir = args.get(1).cloned()
            .unwrap_or_else(|| "austro_node_default".to_string());

        let mut port: u16 = 0;
        let mut explorer_port: u16 = 3000;
        let mut bootstrap_peers: Vec<String> = Vec::new();

        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--port" | "-p" => {
                    if let Some(v) = args.get(i + 1) { port = v.parse().unwrap_or(0); i += 2; }
                    else { i += 1; }
                }
                "--explorer-port" | "-e" => {
                    if let Some(v) = args.get(i + 1) { explorer_port = v.parse().unwrap_or(3000); i += 2; }
                    else { i += 1; }
                }
                "--peer" => {
                    if let Some(v) = args.get(i + 1) { bootstrap_peers.push(v.clone()); i += 2; }
                    else { i += 1; }
                }
                _ => { i += 1; }
            }
        }

        NodeConfig { data_dir, port, explorer_port, bootstrap_peers }
    }
}

#[tokio::main]
async fn main() {
    let config = NodeConfig::from_args();
    std::fs::create_dir_all(&config.data_dir).expect("Create data dir");

    let wallet_manager = Arc::new(Mutex::new(WalletManager::new(&config.data_dir)));
    {
        let wm = wallet_manager.lock().unwrap();
        println!("=== Austro P2P Node ===");
        println!("Data dir : {}", config.data_dir);
        println!("P2P port : {}", if config.port == 0 { "auto".to_string() } else { config.port.to_string() });
        println!("Explorer : http://127.0.0.1:{}", config.explorer_port);
        println!("Peers    : {}", if config.bootstrap_peers.is_empty() { "mDNS only".to_string() } else { config.bootstrap_peers.join(", ") });
        println!("Active   : {} | {}", wm.selected, wm.current_wallet().address());
        println!("Wallets  : {}", wm.list_wallets().join(", "));
    }

    let block_store_path = format!("{}/chain", config.data_dir);
    let store = Arc::new(BlockStore::open(&block_store_path).expect("Open block store"));
    let blockchain = Arc::new(Mutex::new(Blockchain::new(&store)));

    {
        let chain = blockchain.lock().unwrap();
        println!(
            "Height   : {} | Difficulty: {} | Valid: {}",
            chain.height(), chain.difficulty, chain.is_valid()
        );
    }

    // Iniciar explorer HTTP em background
    let explorer_blockchain = blockchain.clone();
    let explorer_port = config.explorer_port;
    // Obtemos o peer_id mais tarde no node; passamos placeholder por ora
    let app_state = api::routes::AppState {
        blockchain: explorer_blockchain,
        node_peer_id: String::from("starting..."),
    };
    let app = api::routes::build_router(app_state);
    let listener = tokio::net::TcpListener::bind(
        format!("0.0.0.0:{}", explorer_port)
    ).await.expect("Explorer port bind failed");
    println!("Block explorer : http://127.0.0.1:{}", explorer_port);

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("Explorer server error");
    });

    network::node::run_node(blockchain, store, wallet_manager, config).await;
}
