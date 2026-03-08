use std::fs;
use std::path::Path;

use crate::models::wallet::Wallet;

#[derive(Debug)]
pub struct WalletManager {
    pub selected: String,
    wallets: std::collections::HashMap<String, Wallet>,
    data_dir: String,
}

impl WalletManager {
    pub fn new(data_dir: &str) -> Self {
        let mut wm = WalletManager {
            selected: "default".to_string(),
            wallets: std::collections::HashMap::new(),
            data_dir: data_dir.to_string(),
        };
        wm.load_wallets();
        if wm.wallets.is_empty() {
            let default = Wallet::new();
            wm.create_wallet_raw("default", default);
        }
        wm
    }

    fn load_wallets(&mut self) {
        let wallet_dir = format!("{}/wallets", self.data_dir);
        if !Path::new(&wallet_dir).exists() {
            fs::create_dir_all(&wallet_dir).expect("Create wallets dir");
            return;
        }
        for entry in fs::read_dir(&wallet_dir).expect("Read wallets dir") {
            let entry = entry.expect("Read wallet entry");
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                match self.load_wallet_json(&path) {
                    Ok(wallet) => {
                        self.wallets.insert(name.clone(), wallet);
                    }
                    Err(e) => eprintln!("Failed to load wallet {}: {}", name, e),
                }
            }
        }
    }

    fn load_wallet_json(&self, path: &std::path::Path) -> Result<Wallet, String> {
        let json = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Wallet::from_json(&json)
    }

    pub fn create_wallet(&mut self, name: &str) -> Result<String, String> {
        if self.wallets.contains_key(name) {
            return Err("Wallet exists".to_string());
        }
        let wallet = Wallet::new();
        self.create_wallet_raw(name, wallet)
    }

    fn create_wallet_raw(&mut self, name: &str, wallet: Wallet) -> Result<String, String> {
        self.wallets.insert(name.to_string(), wallet.clone());

        let wallet_dir = format!("{}/wallets", self.data_dir);
        let path = format!("{}/{}.json", wallet_dir, name);
        let json = wallet.to_json();
        fs::write(&path, json).map_err(|e| e.to_string())?;

        Ok(wallet.address())
    }

    pub fn select_wallet(&mut self, name: &str) -> Result<(), String> {
        if !self.wallets.contains_key(name) {
            return Err("Wallet not found".to_string());
        }
        self.selected = name.to_string();
        Ok(())
    }

    pub fn current_wallet(&self) -> &Wallet {
        self.wallets.get(&self.selected).expect("Active wallet missing")
    }

    pub fn get_wallet(&self, name: &str) -> Option<&Wallet> {
        self.wallets.get(name)
    }

    pub fn list_wallets(&self) -> Vec<String> {
        self.wallets.keys().cloned().collect()
    }

    pub fn export_wallet(&self, name: &str, format: &str) -> Result<String, String> {
        let wallet = self.wallets.get(name).ok_or("Wallet not found")?;
        let path = format!("{}/wallets/{}.{}", self.data_dir, name, format);
        match format {
            "wif" => {
                let wif = wallet.to_wif(false);
                fs::write(&path, wif.clone()).map_err(|e| e.to_string())?;
                Ok(wif)
            }
            "json" => {
                let json = wallet.to_json();
                fs::write(&path, json.clone()).map_err(|e| e.to_string())?;
                Ok(json)
            }
            _ => Err("Format must be 'wif' or 'json'".to_string()),
        }
    }

    pub fn import_wallet(&mut self, file_path: &str, name: Option<&str>) -> Result<String, String> {
        let content = fs::read_to_string(file_path).map_err(|e| e.to_string())?;

        let wallet = if content.contains('{') {
            Wallet::from_json(&content)?
        } else {
            Wallet::from_wif(&content, false)?
        };

        let name = name.unwrap_or("imported").to_string();
        self.create_wallet_raw(&name, wallet)
    }
}
