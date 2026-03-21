use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ApiBlock {
    pub index:         u64,
    pub hash:          String,
    pub previous_hash: String,
    pub timestamp:     u64,
    pub nonce:         u64,
    pub difficulty:    usize,
    pub reward:        u64,
    pub tx_count:      usize,
    pub transactions:  Vec<ApiTx>,
    pub miner:         String,
}

#[derive(Serialize)]
pub struct ApiTx {
    pub id:          String,
    pub is_coinbase: bool,
    pub inputs:      Vec<ApiInput>,
    pub outputs:     Vec<ApiOutput>,
    pub fee:         u64,
}

#[derive(Serialize)]
pub struct ApiInput {
    pub tx_id:     String,
    pub out_index: u32,
}

#[derive(Serialize)]
pub struct ApiOutput {
    pub value:   u64,
    pub address: String,
}

#[derive(Serialize)]
pub struct ApiChainInfo {
    pub height:             u64,
    pub difficulty:         usize,
    pub total_supply:       u64,
    pub pending_txs:        usize,
    pub total_fees_pending: u64,
    pub is_valid:           bool,
}

#[derive(Serialize)]
pub struct ApiMempool {
    pub count:        usize,
    pub total_fees:   u64,
    pub transactions: Vec<ApiTx>,
}

#[derive(Serialize)]
pub struct ApiAddressInfo {
    pub address:      String,
    pub balance:      u64,
    pub tx_count:     usize,
    pub transactions: Vec<ApiAddressTx>,
}

#[derive(Serialize)]
pub struct ApiAddressTx {
    pub tx_id:         String,
    pub block:         u64,
    pub confirmations: u64,
    pub net:           i64,
    pub fee:           u64,
    pub direction:     String,
}

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(Serialize)]
pub struct ApiWallet {
    pub name:    String,
    pub address: String,
    pub balance: u64,
    pub active:  bool,
}

#[derive(Serialize)]
pub struct ApiMnemonicResult {
    pub name:    String,
    pub address: String,
    pub phrase:  String,
}

#[derive(Serialize)]
pub struct ApiSuccess {
    pub ok:      bool,
    pub message: String,
}

#[derive(Serialize)]
pub struct ApiPeers {
    pub count: usize,
    pub peers: Vec<String>,
}

#[derive(Serialize)]
pub struct ApiMiningStatus {
    pub mining:  bool,
    pub address: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqCreateWallet {
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqRecoverWallet {
    pub name:   String,
    pub phrase: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqImportWallet {
    pub path: String,
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqExportWallet {
    pub name:   String,
    pub format: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqSaveWallet {
    pub name:     String,
    pub password: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqLoadWallet {
    pub name:     String,
    pub password: String,
}

#[derive(Deserialize, Serialize)]
pub struct ReqSend {
    pub to:     String,
    pub amount: u64,
    pub fee:    u64,
}

#[derive(Serialize)]
pub struct ApiSendResult {
    pub tx_id: String,
    pub fee:   u64,
}