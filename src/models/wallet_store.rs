use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::models::wallet::Wallet;

const WALLETS_DIR: &str = "wallets";
const NONCE_LEN: usize = 12;

fn derive_key(data_dir: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"austro-wallet-v2:");
    hasher.update(data_dir.as_bytes());
    hasher.finalize().into()
}

pub struct WalletManager {
    data_dir: PathBuf,
    key: [u8; 32],
    pub wallets: HashMap<String, Wallet>,
    pub selected: String,
}

impl WalletManager {
    pub fn new(data_dir: &str) -> Self {
        let wallets_dir = Path::new(data_dir).join(WALLETS_DIR);
        fs::create_dir_all(&wallets_dir).expect("Create wallets dir");

        let key = derive_key(data_dir);
        let mut manager = WalletManager {
            data_dir: PathBuf::from(data_dir),
            key,
            wallets: HashMap::new(),
            selected: String::new(),
        };

        manager.scan_wallets();

        // If no wallets exist, create default
        if manager.wallets.is_empty() {
            manager.create_wallet("default").expect("Create default wallet");
        }

        if manager.selected.is_empty() {
            manager.selected = manager.wallets.keys().next().unwrap().clone();
        }

        manager
    }

    pub fn list_wallets(&self) -> Vec<String> {
        let mut names: Vec<String> = self.wallets.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn select_wallet(&mut self, name: &str) -> Result<(), String> {
        if self.wallets.contains_key(name) {
            self.selected = name.to_string();
            Ok(())
        } else {
            Err(format!("Wallet '{}' not found", name))
        }
    }

    pub fn current_wallet(&self) -> &Wallet {
        self.wallets.get(&self.selected).expect("Selected wallet exists")
    }

    pub fn get_wallet(&self, name: &str) -> Option<&Wallet> {
        self.wallets.get(name)
    }

    pub fn create_wallet(&mut self, name: &str) -> Result<String, String> {
        if self.wallets.contains_key(name) {
            return Err(format!("Wallet '{}' already exists", name));
        }
        let wallet = Wallet::new();
        let address = wallet.address();
        self.save_wallet_to_disk(name, &wallet)?;
        self.wallets.insert(name.to_string(), wallet);

        if self.selected.is_empty() {
            self.selected = name.to_string();
        }

        Ok(address)
    }

    fn wallet_path(&self, name: &str) -> PathBuf {
        self.data_dir.join(WALLETS_DIR).join(format!("{}.dat", name))
    }

    fn scan_wallets(&mut self) {
        let wallets_dir = self.data_dir.join(WALLETS_DIR);
        if !wallets_dir.exists() {
            return;
        }

        if let Ok(entries) = fs::read_dir(&wallets_dir) {
            let mut found: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let fname = e.file_name().to_string_lossy().to_string();
                    fname.strip_suffix(".dat").map(|s| s.to_string())
                })
                .collect();

            found.sort();

            for name in found {
                match self.load_wallet_from_disk(&name) {
                    Ok(wallet) => {
                        if self.selected.is_empty() {
                            self.selected = name.clone();
                        }
                        self.wallets.insert(name, wallet);
                    }
                    Err(e) => eprintln!("Failed to load wallet '{}': {}", name, e),
                }
            }
        }
    }

    pub fn save_wallet_to_disk(&self, name: &str, wallet: &Wallet) -> Result<(), String> {
        let path = self.wallet_path(name);
        let raw_key = wallet.private_key.secret_bytes();

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key));
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, raw_key.as_ref())
            .map_err(|_| "Encryption failed".to_string())?;

        let mut data = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        data.extend_from_slice(&nonce_bytes);
        data.extend_from_slice(&ciphertext);

        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &data).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;

        Ok(())
    }

    fn load_wallet_from_disk(&self, name: &str) -> Result<Wallet, String> {
        let path = self.wallet_path(name);
        let data = fs::read(&path).map_err(|e| e.to_string())?;

        if data.len() <= NONCE_LEN {
            return Err("Wallet file too short".to_string());
        }

        let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key));

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| "Decrypt failed — wrong key or corrupted file".to_string())?;

        if plaintext.len() != 32 {
            return Err("Invalid key length".to_string());
        }

        Wallet::from_raw_key(&plaintext).map_err(|e| e.to_string())
    }
}
