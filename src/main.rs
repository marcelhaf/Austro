#[cfg(test)]
mod tests;

mod api;
mod models;
mod network;

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::{debug, error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    fmt::{self, time::UtcTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

use models::blockchain::Blockchain;
use models::storage::BlockStore;
use models::wallet_store::WalletManager;
use api::routes::ApiCommand;

pub struct NodeConfig {
    pub data_dir:        String,
    pub port:            u16,
    pub explorer_port:   u16,
    pub bootstrap_peers: Vec<String>,
    pub log_format:      String,
    pub log_dir:         Option<String>,
}

impl NodeConfig {
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let data_dir = args.get(1).cloned()
            .unwrap_or_else(|| "austro_node_default".to_string());

        let mut port:            u16            = 0;
        let mut explorer_port:   u16            = 3000;
        let mut bootstrap_peers: Vec<String>    = Vec::new();
        let mut log_format:      String         = "pretty".to_string();
        let mut log_dir:         Option<String> = None;
        let mut no_default_peer: bool           = false;

        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--port" | "-p" => {
                    if let Some(v) = args.get(i + 1) {
                        port = v.parse().unwrap_or(0); i += 2;
                    } else { i += 1; }
                }
                "--explorer-port" | "-e" => {
                    if let Some(v) = args.get(i + 1) {
                        explorer_port = v.parse().unwrap_or(3000); i += 2;
                    } else { i += 1; }
                }
                "--peer" => {
                    if let Some(v) = args.get(i + 1) {
                        bootstrap_peers.push(v.clone()); i += 2;
                    } else { i += 1; }
                }
                "--no-default-peer" => {
                    no_default_peer = true;
                    i += 1;
                }
                "--log-format" => {
                    if let Some(v) = args.get(i + 1) {
                        log_format = v.clone(); i += 2;
                    } else { i += 1; }
                }
                "--log-dir" => {
                    if let Some(v) = args.get(i + 1) {
                        log_dir = Some(v.clone()); i += 2;
                    } else { i += 1; }
                }
                _ => { i += 1; }
            }
        }

        if bootstrap_peers.is_empty() && !no_default_peer {
            bootstrap_peers.push("/ip4/147.224.133.52/tcp/4001".to_string());
        }

        NodeConfig { data_dir, port, explorer_port, bootstrap_peers, log_format, log_dir }
    }
}

fn init_tracing(config: &NodeConfig) -> WorkerGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("austro=info,libp2p=warn,sled=warn,tower_http=warn")
    });

    let (file_writer, guard) = if let Some(ref log_dir) = config.log_dir {
        std::fs::create_dir_all(log_dir).expect("Create log dir");
        let appender = tracing_appender::rolling::daily(log_dir, "austro.log");
        tracing_appender::non_blocking(appender)
    } else {
        tracing_appender::non_blocking(std::io::stdout())
    };

    let stdout_layer = {
        let base = fmt::layer()
            .with_timer(UtcTime::rfc_3339())
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false);

        match config.log_format.as_str() {
            "json"    => base.json().with_current_span(true).with_span_list(true).boxed(),
            "compact" => base.compact().boxed(),
            _         => base.pretty().boxed(),
        }
    };

    let file_layer: Option<Box<dyn Layer<_> + Send + Sync>> =
        if config.log_dir.is_some() {
            Some(
                fmt::layer()
                    .json()
                    .with_writer(file_writer)
                    .with_timer(UtcTime::rfc_3339())
                    .with_current_span(true)
                    .with_span_list(true)
                    .with_target(true)
                    .boxed(),
            )
        } else {
            None
        };

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    guard
}

#[tokio::main]
async fn main() {
    let config = NodeConfig::from_args();
    std::fs::create_dir_all(&config.data_dir).expect("Create data dir");

    let _tracing_guard = init_tracing(&config);

    let wallet_manager = Arc::new(Mutex::new(WalletManager::new(&config.data_dir)));
    {
        let wm = wallet_manager.lock().unwrap();
        info!(
            data_dir  = %config.data_dir,
            p2p_port  = config.port,
            explorer  = config.explorer_port,
            bootstrap = %config.bootstrap_peers.join(", "),
            wallet    = %wm.selected,
            address   = %wm.current_wallet().address(),
            wallets   = %wm.list_wallets().join(", "),
            "Austro P2P node starting"
        );
    }

    let block_store_path = format!("{}/chain", config.data_dir);
    let store      = Arc::new(BlockStore::open(&block_store_path).expect("Open block store"));
    let blockchain = Arc::new(Mutex::new(Blockchain::new(&store)));

    {
        let chain = blockchain.lock().unwrap();
        info!(
            height     = chain.height(),
            difficulty = chain.difficulty,
            valid      = chain.is_valid(),
            "Chain loaded"
        );
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ApiCommand>();

    let app_state = api::routes::AppState {
        blockchain:     blockchain.clone(),
        wallet_manager: wallet_manager.clone(),
        cmd_tx,
        node_peer_id:   String::from("starting..."),
    };

    let app       = api::routes::build_router(app_state);
    let bind_addr = format!("0.0.0.0:{}", config.explorer_port);
    let listener  = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("Explorer port bind failed");

    info!(
        address = %format!("http://127.0.0.1:{}", config.explorer_port),
        "Block explorer listening"
    );

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "Explorer HTTP server error");
        }
    });

    network::node::run_node(blockchain, store, wallet_manager, config, cmd_rx).await;

    debug!("Main loop exited");
}
