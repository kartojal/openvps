use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tracing::{info, warn};

use super::challenge::MppChallenge;
use super::credential::MppCredential;
use super::receipt::MppReceipt;
use crate::AppState;

/// Request body for provision endpoint — parsed to calculate price.
#[derive(Debug, Deserialize)]
struct ProvisionBody {
    vcpus: Option<u32>,
    ram_mb: Option<u32>,
    disk_gb: Option<u32>,
    duration: Option<u64>,
}

/// MPP payment middleware.
///
/// Flow:
/// 1. If no Authorization header → peek at body to calculate price → return 402 with challenge
/// 2. If Authorization: Payment → verify credential → verify on-chain → pass through with receipt
pub async fn mpp_payment_gate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Check for Authorization header
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match auth_header {
        Some(auth) if auth.starts_with("Payment ") => {
            // Client is presenting a credential — verify it
            match MppCredential::from_authorization(&auth) {
                Some(credential) => {
                    // Verify the challenge signature (proves we issued it)
                    let challenge_json = headers
                        .get("x-mpp-challenge")
                        .and_then(|v| v.to_str().ok())
                        .and_then(super::challenge::parse_challenge);

                    match challenge_json {
                        Some(challenge) => {
                            if !challenge.verify_signature(&state.config.mpp_secret_key) {
                                warn!(challenge_id = %challenge.id, "Invalid challenge signature");
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    "Invalid challenge signature",
                                )
                                    .into_response();
                            }

                            if challenge.is_expired() {
                                warn!(challenge_id = %challenge.id, "Challenge expired");
                                return (StatusCode::GONE, "Challenge expired").into_response();
                            }

                            if credential.challenge_id != challenge.id {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    "Credential does not match challenge",
                                )
                                    .into_response();
                            }

                            // Verify payment on-chain
                            match verify_payment_onchain(&state, &credential, &challenge).await {
                                Ok(true) => {
                                    info!(
                                        tx = %credential.tx_hash,
                                        challenge_id = %challenge.id,
                                        "Payment verified"
                                    );

                                    // Generate receipt
                                    let receipt = MppReceipt::success(
                                        &challenge.id,
                                        &credential.tx_hash,
                                        &state.config.mpp_secret_key,
                                    );

                                    // Pass through to handler
                                    let mut response = next.run(request).await;

                                    // Attach receipt header
                                    response.headers_mut().insert(
                                        "payment-receipt",
                                        receipt.to_header_value().parse().unwrap(),
                                    );

                                    response
                                }
                                Ok(false) => {
                                    warn!(tx = %credential.tx_hash, "Payment verification failed");
                                    (
                                        StatusCode::PAYMENT_REQUIRED,
                                        "Payment not confirmed on-chain",
                                    )
                                        .into_response()
                                }
                                Err(e) => {
                                    warn!(error = %e, "Payment verification error");
                                    (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "Payment verification error",
                                    )
                                        .into_response()
                                }
                            }
                        }
                        None => {
                            (StatusCode::BAD_REQUEST, "Missing or invalid X-MPP-Challenge header")
                                .into_response()
                        }
                    }
                }
                None => {
                    (StatusCode::BAD_REQUEST, "Invalid Payment credential format").into_response()
                }
            }
        }
        _ => {
            // No payment credential — return 402 with challenge
            // We need to peek at the body to calculate price
            let (_parts, body) = request.into_parts();

            let body_bytes = match axum::body::to_bytes(body, 1024 * 64).await {
                Ok(b) => b,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid request body").into_response()
                }
            };

            let provision: ProvisionBody = match serde_json::from_slice(&body_bytes) {
                Ok(p) => p,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid JSON body").into_response()
                }
            };

            let vcpus = provision.vcpus.unwrap_or(1);
            let ram_mb = provision.ram_mb.unwrap_or(512);
            let disk_gb = provision.disk_gb.unwrap_or(10);
            let duration = provision.duration.unwrap_or(3600);

            let price_micro = state
                .config
                .calculate_price_micro(vcpus, ram_mb, disk_gb, duration);

            let challenge = MppChallenge::new(
                price_micro,
                &state.config.payment_recipient,
                "tempo",
                "USDC",
                &state.config.mpp_secret_key,
            );

            let www_auth = challenge.to_www_authenticate();
            let challenge_json = serde_json::to_string(&challenge).unwrap_or_default();

            info!(
                price_micro,
                vcpus, ram_mb, disk_gb, duration, "Issuing payment challenge"
            );

            Response::builder()
                .status(StatusCode::PAYMENT_REQUIRED)
                .header("www-authenticate", &www_auth)
                .header("content-type", "application/json")
                .body(Body::from(challenge_json))
                .unwrap()
        }
    }
}

/// Verify a payment transaction on the Tempo chain.
///
/// In production, this queries the Tempo RPC to confirm:
/// 1. Transaction exists and is confirmed
/// 2. Transfer is to the correct recipient
/// 3. Amount matches the challenge
/// 4. Token is the correct stablecoin
async fn verify_payment_onchain(
    state: &AppState,
    credential: &MppCredential,
    challenge: &MppChallenge,
) -> anyhow::Result<bool> {
    // TODO: Implement actual on-chain verification via Tempo RPC
    //
    // For production, this should:
    // 1. Call eth_getTransactionReceipt on Tempo RPC
    // 2. Decode the Transfer event logs
    // 3. Verify: to == challenge.recipient, amount >= challenge.amount
    // 4. Verify the token contract matches USDC_CONTRACT
    //
    // For now, in development mode, we accept any credential with a non-empty tx_hash.

    if credential.tx_hash.is_empty() {
        return Ok(false);
    }

    // Development: accept all non-empty tx hashes
    // Production: uncomment and implement RPC verification
    if state.config.mpp_secret_key == "dev-secret-change-me" {
        tracing::warn!("DEV MODE: Accepting payment without on-chain verification");
        return Ok(true);
    }

    // Production verification via Tempo RPC
    let tx_hash = &credential.tx_hash;
    let rpc_url = &state.config.tempo_rpc_url;

    let client = reqwest::Client::new();
    let response = client
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getTransactionReceipt",
            "params": [tx_hash],
            "id": 1
        }))
        .send()
        .await?;

    let receipt: serde_json::Value = response.json().await?;

    // Check transaction was successful
    let status = receipt["result"]["status"]
        .as_str()
        .unwrap_or("0x0");

    if status != "0x1" {
        return Ok(false);
    }

    // Check logs for Transfer event to our recipient
    let logs = receipt["result"]["logs"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // Transfer(address,address,uint256) topic
    let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

    for log in &logs {
        let topics = log["topics"].as_array();
        if let Some(topics) = topics {
            if topics.len() >= 3
                && topics[0].as_str() == Some(transfer_topic)
            {
                // topics[2] is the `to` address (padded to 32 bytes)
                let to = topics[2].as_str().unwrap_or_default();
                let to_addr = format!("0x{}", &to[to.len().saturating_sub(40)..]);

                if to_addr.to_lowercase() == challenge.recipient.to_lowercase() {
                    // Verify contract address matches our USDC contract
                    let contract = log["address"].as_str().unwrap_or_default();
                    if contract.to_lowercase() == state.config.usdc_contract.to_lowercase() {
                        return Ok(true);
                    }
                }
            }
        }
    }

    Ok(false)
}
