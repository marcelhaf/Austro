use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use secp256k1::rand::rngs::OsRng;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Wallet {
    pub private_key: SecretKey,
    pub public_key: PublicKey,
}

impl Wallet {
    pub fn new() -> Self {
        let secp = Secp256k1::new();
        let (private_key, public_key) = secp.generate_keypair(&mut OsRng);
        Wallet { private_key, public_key }
    }

    pub fn from_raw_key(bytes: &[u8]) -> Result<Self, secp256k1::Error> {
        let secp = Secp256k1::new();
        let private_key = SecretKey::from_slice(bytes)?;
        let public_key = PublicKey::from_secret_key(&secp, &private_key);
        Ok(Wallet { private_key, public_key })
    }

    pub fn pub_key_hash(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.public_key.serialize());
        hasher.finalize().to_vec()
    }

    pub fn address(&self) -> String {
        hex::encode(self.pub_key_hash())
    }

    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        let secp = Secp256k1::new();
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hasher.finalize();
        let msg = Message::from_digest_slice(&hash).unwrap();
        secp.sign_ecdsa(&msg, &self.private_key)
            .serialize_der()
            .to_vec()
    }
}
