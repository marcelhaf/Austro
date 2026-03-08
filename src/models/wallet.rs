use secp256k1::{Secp256k1, SecretKey, PublicKey};
use sha2::{Sha256, Digest};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Wallet {
    pub private_key: SecretKey,
    pub public_key: PublicKey,
}

impl Wallet {
    pub fn new() -> Self {
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut rand::thread_rng());
        Wallet { private_key: secret_key, public_key }
    }

    pub fn pub_key_hash(&self) -> Vec<u8> {
        let pub_bytes = self.public_key.serialize().to_vec();
        let sha1 = Sha256::digest(&pub_bytes);
        let sha2 = Sha256::digest(&sha1);
        sha2.to_vec()
    }

    pub fn address(&self) -> String {
        hex::encode(self.pub_key_hash())
    }

    pub fn sign_msg(&self, msg: &[u8]) -> Vec<u8> {
        let secp = Secp256k1::new();
        let message = secp256k1::Message::from_digest_slice(msg).expect("32 bytes");
        let sig = secp.sign_ecdsa(&message, &self.private_key);
        sig.serialize_der().to_vec()
    }

    pub fn to_wif(&self, testnet: bool) -> String {
        let mut payload: Vec<u8> = Vec::new();
        payload.push(if testnet { 0xef } else { 0x80 });
        payload.extend_from_slice(&self.private_key.secret_bytes());
        payload.push(0x01);

        let check = wif_checksum(&payload);
        payload.extend_from_slice(&check);
        bs58::encode(payload).into_string()
    }

    pub fn from_wif(wif: &str, testnet: bool) -> Result<Self, String> {
        let decoded = bs58::decode(wif).into_vec()
            .map_err(|_| "Invalid Base58 encoding".to_string())?;

        if decoded.len() != 38 {
            return Err(format!("Invalid WIF length: {} (expected 38)", decoded.len()));
        }

        let expected_prefix = if testnet { 0xef } else { 0x80 };
        if decoded[0] != expected_prefix {
            return Err(format!("Invalid WIF prefix: 0x{:02x}", decoded[0]));
        }

        if decoded[33] != 0x01 {
            return Err("Only compressed keys supported".to_string());
        }

        let payload = &decoded[..34];
        let checksum = &decoded[34..38];
        let expected = wif_checksum(payload);
        if checksum != expected {
            return Err("Invalid WIF checksum".to_string());
        }

        let priv_bytes = &decoded[1..33];
        let secret_key = SecretKey::from_slice(priv_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;
        let secp = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        Ok(Wallet { private_key: secret_key, public_key })
    }

    pub fn to_json(&self) -> String {
        let priv_hex = hex::encode(self.private_key.secret_bytes());
        let pub_hex  = hex::encode(self.public_key.serialize());
        let address  = self.address();
        format!(
            "{{\n  \"version\": 1,\n  \"private_key\": \"{}\",\n  \"public_key\": \"{}\",\n  \"address\": \"{}\"\n}}",
            priv_hex, pub_hex, address
        )
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct WalletJson {
            private_key: String,
            #[allow(dead_code)]
            public_key: Option<String>,
            #[allow(dead_code)]
            address: Option<String>,
            #[allow(dead_code)]
            version: Option<u32>,
        }
        let w: WalletJson = serde_json::from_str(json)
            .map_err(|e| format!("JSON parse error: {}", e))?;
        let priv_bytes = hex::decode(&w.private_key)
            .map_err(|_| "Invalid private key hex".to_string())?;
        let secret_key = SecretKey::from_slice(&priv_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;
        let secp = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        Ok(Wallet { private_key: secret_key, public_key })
    }
}

fn wif_checksum(data: &[u8]) -> [u8; 4] {
    let hash1 = Sha256::digest(data);
    let hash2 = Sha256::digest(&hash1);
    hash2[0..4].try_into().unwrap()
}
