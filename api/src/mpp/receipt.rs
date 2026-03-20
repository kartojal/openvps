use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// MPP Receipt — returned in the Payment-Receipt header after successful payment verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppReceipt {
    /// The challenge ID this receipt is for
    pub challenge_id: String,
    /// Status: "success" or "failed"
    pub status: String,
    /// Transaction hash
    pub tx_hash: String,
    /// Server signature over the receipt
    pub signature: String,
}

impl MppReceipt {
    pub fn success(challenge_id: &str, tx_hash: &str, secret_key: &str) -> Self {
        let mut receipt = Self {
            challenge_id: challenge_id.to_string(),
            status: "success".to_string(),
            tx_hash: tx_hash.to_string(),
            signature: String::new(),
        };
        receipt.signature = receipt.sign(secret_key);
        receipt
    }

    fn sign(&self, secret_key: &str) -> String {
        let message = format!("{}:{}:{}", self.challenge_id, self.status, self.tx_hash);
        let mut mac =
            HmacSha256::new_from_slice(secret_key.as_bytes()).expect("HMAC key length is valid");
        mac.update(message.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Encode as the Payment-Receipt header value.
    pub fn to_header_value(&self) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(self).unwrap_or_default())
    }
}

use base64::Engine;
