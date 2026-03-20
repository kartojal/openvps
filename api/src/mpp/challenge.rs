use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// MPP Challenge — sent in the WWW-Authenticate header when payment is required.
///
/// Per the MPP spec, the server responds with HTTP 402 and a challenge that tells
/// the client what to pay, how much, and where.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppChallenge {
    /// Unique challenge ID for correlation
    pub id: String,
    /// Realm describing the service
    pub realm: String,
    /// Payment method (e.g., "tempo")
    pub method: String,
    /// Intent type (e.g., "charge")
    pub intent: String,
    /// Amount in the smallest unit of the currency
    pub amount: String,
    /// Currency identifier
    pub currency: String,
    /// Recipient address
    pub recipient: String,
    /// Network/chain identifier
    pub network: String,
    /// Challenge expiry (ISO 8601)
    pub expires_at: String,
    /// HMAC signature over the challenge fields
    pub signature: String,
}

impl MppChallenge {
    /// Create a new challenge for a given price.
    pub fn new(
        amount_micro: u64,
        recipient: &str,
        network: &str,
        currency: &str,
        secret_key: &str,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let expires_at = (Utc::now() + Duration::minutes(5)).to_rfc3339();

        let mut challenge = Self {
            id,
            realm: "mpp-hosting".to_string(),
            method: "tempo".to_string(),
            intent: "charge".to_string(),
            amount: amount_micro.to_string(),
            currency: currency.to_string(),
            recipient: recipient.to_string(),
            network: network.to_string(),
            expires_at,
            signature: String::new(),
        };

        challenge.signature = challenge.sign(secret_key);
        challenge
    }

    /// Sign the challenge fields with HMAC-SHA256.
    fn sign(&self, secret_key: &str) -> String {
        let message = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            self.id,
            self.realm,
            self.method,
            self.intent,
            self.amount,
            self.currency,
            self.recipient,
            self.expires_at
        );

        let mut mac =
            HmacSha256::new_from_slice(secret_key.as_bytes()).expect("HMAC key length is valid");
        mac.update(message.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Verify the challenge signature.
    pub fn verify_signature(&self, secret_key: &str) -> bool {
        let expected = self.sign(secret_key);
        self.signature == expected
    }

    /// Check if the challenge has expired.
    pub fn is_expired(&self) -> bool {
        if let Ok(expires) = self.expires_at.parse::<chrono::DateTime<Utc>>() {
            Utc::now() > expires
        } else {
            true
        }
    }

    /// Encode as the WWW-Authenticate header value.
    pub fn to_www_authenticate(&self) -> String {
        format!(
            "Payment realm=\"{}\", challenge=\"{}\"",
            self.realm,
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_string(self).unwrap_or_default())
        )
    }
}

use base64::Engine;

/// Parse a challenge from the base64-encoded value in a header.
pub fn parse_challenge(encoded: &str) -> Option<MppChallenge> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}
