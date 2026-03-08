use secp256k1::{Secp256k1, Message, PublicKey, ecdsa::Signature};
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};
use hex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct OutPoint {
    pub tx_id: String,
    pub out_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TXInput {
    pub previous_output: OutPoint,
    pub signature: Vec<u8>,   // DER-encoded ECDSA signature
    pub pub_key: Vec<u8>,     // Compressed public key (33 bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TXOutput {
    pub value: u64,
    pub pub_key_hash: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub vin: Vec<TXInput>,
    pub vout: Vec<TXOutput>,
}

impl Transaction {
    /// Genesis coinbase — deterministic ID, no timestamp.
    /// All nodes must produce the exact same TX ID for the genesis block.
    pub fn coinbase_genesis(pub_key_hash: &[u8], value: u64) -> Self {
        // Fixed nonce ensures every node computes identical TX ID
        let mut id_input = Vec::new();
        id_input.extend_from_slice(pub_key_hash);
        id_input.extend_from_slice(&value.to_le_bytes());
        id_input.extend_from_slice(b"austro-genesis-v1");

        let mut hasher = sha2::Sha256::new();
        hasher.update(&id_input);
        let id = format!("{:x}", hasher.finalize());

        Transaction {
            id,
            vin: vec![TXInput {
                previous_output: OutPoint {
                    tx_id: String::new(),
                    out_index: u32::MAX,
                },
                pub_key: Vec::new(),
                signature: Vec::new(),
            }],
            vout: vec![TXOutput {
                value,
                pub_key_hash: pub_key_hash.to_vec(),
            }],
        }
    }

    pub fn coinbase(pub_key_hash: &[u8], value: u64) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let mut id_input = Vec::new();
        id_input.extend_from_slice(pub_key_hash);
        id_input.extend_from_slice(&value.to_le_bytes());
        id_input.extend_from_slice(&nanos.to_le_bytes());

        let mut hasher = sha2::Sha256::new();
        hasher.update(&id_input);
        let id = format!("{:x}", hasher.finalize());

        Transaction {
            id,
            vin: vec![TXInput {
                previous_output: OutPoint {
                    tx_id: String::new(),
                    out_index: u32::MAX,
                },
                pub_key: Vec::new(),
                signature: Vec::new(),
            }],
            vout: vec![TXOutput {
                value,
                pub_key_hash: pub_key_hash.to_vec(),
            }],
        }
    }

    pub fn new_unsigned(inputs: Vec<(OutPoint, Vec<u8>)>, outputs: Vec<TXOutput>) -> Self {
        let vin = inputs
            .into_iter()
            .map(|(outpoint, pub_key)| TXInput {
                previous_output: outpoint,
                signature: vec![],
                pub_key,
            })
            .collect();

        let mut tx = Transaction {
            id: String::new(),
            vin,
            vout: outputs,
        };
        tx.id = tx.signing_hash(); // ID set before signing
        tx
    }

    // Hash used as the message each input signs (excludes signatures)
    pub fn signing_hash(&self) -> String {
        let mut clone = self.clone();
        clone.id = String::new();
        for input in &mut clone.vin {
            input.signature = vec![];
        }
        let serialized = serde_json::to_string(&clone).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        hex::encode(hasher.finalize())
    }

    // Final content hash (includes signatures, used for block inclusion)
    pub fn hash(&self) -> String {
        let mut clone = self.clone();
        clone.id = String::new();
        let serialized = serde_json::to_string(&clone).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        hex::encode(hasher.finalize())
    }

    // Sign all inputs with the given private key
    pub fn sign_inputs(&mut self, wallet: &crate::models::wallet::Wallet) {
        let msg_bytes = hex::decode(&self.id).unwrap();
        let sig = wallet.sign(&msg_bytes);
        for input in &mut self.vin {
            input.signature = sig.clone();
            input.pub_key = wallet.public_key.serialize().to_vec();
        }
        // Recalculate final hash after signing
        self.id = self.hash();
    }

    // Verify all input signatures
    pub fn verify_signatures(&self) -> bool {
        if self.is_coinbase() {
            return true;
        }

        let secp = Secp256k1::new();

        // Reconstruct signing hash (without signatures)
        let mut clone = self.clone();
        for input in &mut clone.vin {
            input.signature = vec![];
        }
        let signing_id_hex = clone.signing_hash();
        let signing_bytes = hex::decode(&signing_id_hex).unwrap();

        let mut hasher = Sha256::new();
        hasher.update(&signing_bytes);
        let hash = hasher.finalize();

        let msg = match Message::from_digest_slice(&hash) {
            Ok(m) => m,
            Err(_) => return false,
        };

        for input in &self.vin {
            if input.signature.is_empty() || input.pub_key.is_empty() {
                return false;
            }

            let pub_key = match PublicKey::from_slice(&input.pub_key) {
                Ok(k) => k,
                Err(_) => return false,
            };

            let sig = match Signature::from_der(&input.signature) {
                Ok(s) => s,
                Err(_) => return false,
            };

            if secp.verify_ecdsa(&msg, &sig, &pub_key).is_err() {
                return false;
            }
        }

        true
    }

    pub fn is_coinbase(&self) -> bool {
        self.vin.len() == 1
            && self.vin[0].previous_output.tx_id.is_empty()
            && self.vin[0].previous_output.out_index == u32::MAX
    }

    pub fn verify_basic(&self) -> bool {
        !self.vout.is_empty() && self.vout.iter().all(|o| o.value > 0)
    }
}
