use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use bip39::{Language, Mnemonic};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

#[derive(Debug, Clone)]
pub struct Wallet {
    pub private_key: SecretKey,
    pub public_key:  PublicKey,
    pub mnemonic:    Option<String>,
}

impl Wallet {
    pub fn new() -> Self {
        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut rand::thread_rng());
        Wallet { private_key: secret_key, public_key, mnemonic: None }
    }

    pub fn new_with_mnemonic() -> (Self, String) {
        use rand::RngCore;
        let mut entropy = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
            .expect("Failed to generate mnemonic");
        let phrase   = mnemonic.to_string();
        let wallet   = Self::from_mnemonic(&phrase)
            .expect("Derivation from fresh mnemonic failed");
        (wallet, phrase)
    }

    pub fn from_mnemonic(phrase: &str) -> Result<Self, String> {
        let mnemonic = Mnemonic::parse_in(Language::English, phrase)
            .map_err(|e| format!("Invalid mnemonic: {}", e))?;

        let seed = mnemonic.to_seed("");

        let mut derived = [0u8; 32];
        pbkdf2_hmac::<Sha512>(
            &seed,
            b"austro-wallet-v1",
            2048,
            &mut derived,
        );

        let secp       = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&derived)
            .map_err(|e| format!("Key derivation failed: {}", e))?;
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);

        Ok(Wallet {
            private_key: secret_key,
            public_key,
            mnemonic: Some(phrase.to_string()),
        })
    }

    pub fn pub_key_hash(&self) -> Vec<u8> {
        let pub_bytes = self.public_key.serialize().to_vec();
        let sha1      = Sha256::digest(&pub_bytes);
        let sha2      = Sha256::digest(sha1);
        sha2.to_vec()
    }

    pub fn address(&self) -> String {
        hex::encode(self.pub_key_hash())
    }

    pub fn sign_msg(&self, msg: &[u8]) -> Vec<u8> {
        let secp    = Secp256k1::new();
        let message = secp256k1::Message::from_digest_slice(msg).expect("32 bytes");
        let sig     = secp.sign_ecdsa(&message, &self.private_key);
        sig.serialize_der().to_vec()
    }

    pub fn encrypt_to_file(&self, path: &str, password: &str) -> Result<(), String> {
        let priv_hex = hex::encode(self.private_key.secret_bytes());
        let mnemonic = self.mnemonic.clone().unwrap_or_default();
        let plaintext = format!("{}:{}", priv_hex, mnemonic);

        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);

        let mut key_bytes = [0u8; 32];
        pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, 100_000, &mut key_bytes);

        let key    = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| format!("Encryption failed: {}", e))?;

        let file = EncryptedWalletFile {
            version:    2,
            salt:       hex::encode(salt),
            nonce:      hex::encode(nonce_bytes),
            ciphertext: hex::encode(ciphertext),
            address:    self.address(),
        };

        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| format!("Serialization failed: {}", e))?;

        std::fs::write(path, json).map_err(|e| format!("Write failed: {}", e))?;
        Ok(())
    }

    pub fn decrypt_from_file(path: &str, password: &str) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("Read failed: {}", e))?;

        let file: EncryptedWalletFile = serde_json::from_str(&json)
            .map_err(|e| format!("Parse failed: {}", e))?;

        let salt        = hex::decode(&file.salt)
            .map_err(|_| "Invalid salt".to_string())?;
        let nonce_bytes = hex::decode(&file.nonce)
            .map_err(|_| "Invalid nonce".to_string())?;
        let ciphertext  = hex::decode(&file.ciphertext)
            .map_err(|_| "Invalid ciphertext".to_string())?;

        let mut key_bytes = [0u8; 32];
        pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, 100_000, &mut key_bytes);

        let key    = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        let nonce  = Nonce::from_slice(&nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| "Wrong password or corrupted file".to_string())?;

        let content  = String::from_utf8(plaintext)
            .map_err(|_| "Invalid decrypted content".to_string())?;

        let mut parts = content.splitn(2, ':');
        let priv_hex  = parts.next().ok_or("Missing private key")?;
        let mnemonic  = parts.next().unwrap_or("").to_string();

        let priv_bytes = hex::decode(priv_hex)
            .map_err(|_| "Invalid private key hex".to_string())?;
        let secret_key = SecretKey::from_slice(&priv_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;
        let secp       = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);

        Ok(Wallet {
            private_key: secret_key,
            public_key,
            mnemonic: if mnemonic.is_empty() { None } else { Some(mnemonic) },
        })
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
        let payload  = &decoded[..34];
        let checksum = &decoded[34..38];
        let expected = wif_checksum(payload);
        if checksum != expected {
            return Err("Invalid WIF checksum".to_string());
        }
        let priv_bytes = &decoded[1..33];
        let secret_key = SecretKey::from_slice(priv_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;
        let secp       = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        Ok(Wallet { private_key: secret_key, public_key, mnemonic: None })
    }

    pub fn to_json(&self) -> String {
        let priv_hex = hex::encode(self.private_key.secret_bytes());
        let pub_hex  = hex::encode(self.public_key.serialize());
        let address  = self.address();
        let mnemonic = self.mnemonic.clone().unwrap_or_default();
        format!(
            "{{\n  \"version\": 1,\n  \"private_key\": \"{}\",\n  \"public_key\": \"{}\",\n  \"address\": \"{}\",\n  \"mnemonic\": \"{}\"\n}}",
            priv_hex, pub_hex, address, mnemonic
        )
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct WalletJson {
            private_key: String,
            #[allow(dead_code)]
            public_key:  Option<String>,
            #[allow(dead_code)]
            address:     Option<String>,
            #[allow(dead_code)]
            version:     Option<u32>,
            mnemonic:    Option<String>,
        }
        let w: WalletJson = serde_json::from_str(json)
            .map_err(|e| format!("JSON parse error: {}", e))?;
        let priv_bytes = hex::decode(&w.private_key)
            .map_err(|_| "Invalid private key hex".to_string())?;
        let secret_key = SecretKey::from_slice(&priv_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;
        let secp       = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        Ok(Wallet {
            private_key: secret_key,
            public_key,
            mnemonic: w.mnemonic.filter(|m| !m.is_empty()),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct EncryptedWalletFile {
    version:    u32,
    salt:       String,
    nonce:      String,
    ciphertext: String,
    address:    String,
}

fn wif_checksum(data: &[u8]) -> [u8; 4] {
    let hash1 = Sha256::digest(data);
    let hash2 = Sha256::digest(hash1);
    hash2[0..4].try_into().unwrap()
}