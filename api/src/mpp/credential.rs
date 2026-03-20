use serde::{Deserialize, Serialize};

/// MPP Credential — sent in the Authorization header after the client pays.
///
/// The client submits this to prove payment was made for a specific challenge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MppCredential {
    /// The challenge ID this credential responds to
    pub challenge_id: String,
    /// Transaction hash on the payment network
    pub tx_hash: String,
    /// The payment network (e.g., "tempo")
    pub network: String,
    /// Payer address
    pub payer: String,
    /// Signature from the payer over the credential
    pub signature: String,
}

impl MppCredential {
    /// Parse from the Authorization header value.
    /// Expected format: `Payment <base64-encoded JSON>`
    pub fn from_authorization(header_value: &str) -> Option<Self> {
        let token = header_value.strip_prefix("Payment ")?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(token.trim())
            .ok()?;
        serde_json::from_slice(&decoded).ok()
    }
}

use base64::Engine;
