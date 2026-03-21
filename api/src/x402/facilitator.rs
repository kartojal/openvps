use anyhow::{Context, Result};
use tracing::{info, warn};

use super::types::{
    FacilitatorRequest, PaymentPayload, PaymentRequirements, SettlementResponse, VerifyResponse,
};

/// Client for communicating with an x402 facilitator service.
pub struct FacilitatorClient {
    url: String,
    http: reqwest::Client,
}

impl FacilitatorClient {
    /// Create a new facilitator client pointed at the given base URL.
    pub fn new(url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            url: url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Build the request body for facilitator endpoints.
    fn build_request(
        payload: &PaymentPayload,
        requirements: &PaymentRequirements,
    ) -> Result<FacilitatorRequest> {
        let payment_payload = serde_json::to_value(payload)
            .context("failed to serialize payment payload")?;
        let payment_requirements = serde_json::to_value(requirements)
            .context("failed to serialize payment requirements")?;

        Ok(FacilitatorRequest {
            x402_version: payload.x402_version,
            payment_payload,
            payment_requirements,
        })
    }

    /// Call the facilitator's /verify endpoint to check payment validity.
    pub async fn verify(
        &self,
        payload: &PaymentPayload,
        requirements: &PaymentRequirements,
    ) -> Result<VerifyResponse> {
        let body = Self::build_request(payload, requirements)?;
        let url = format!("{}/verify", self.url);

        info!(url = %url, "Calling facilitator /verify");

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("facilitator /verify request failed")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "Facilitator /verify returned error");
            anyhow::bail!("facilitator /verify returned {}: {}", status, text);
        }

        response
            .json::<VerifyResponse>()
            .await
            .context("failed to parse facilitator /verify response")
    }

    /// Call the facilitator's /settle endpoint to execute the payment.
    pub async fn settle(
        &self,
        payload: &PaymentPayload,
        requirements: &PaymentRequirements,
    ) -> Result<SettlementResponse> {
        let body = Self::build_request(payload, requirements)?;
        let url = format!("{}/settle", self.url);

        info!(url = %url, "Calling facilitator /settle");

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("facilitator /settle request failed")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "Facilitator /settle returned error");
            anyhow::bail!("facilitator /settle returned {}: {}", status, text);
        }

        response
            .json::<SettlementResponse>()
            .await
            .context("failed to parse facilitator /settle response")
    }
}
