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
                "USD",
                &state.config.mpp_secret_key,
                Some(state.config.chain_id),
                Some(&state.config.tempo_rpc_url),
                Some(&state.config.usdc_contract),
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

/// Known USD-denominated TIP-20 stablecoins on Tempo.
/// Any of these are accepted as payment.
const TEMPO_USD_STABLECOINS: &[&str] = &[
    "0x20c0000000000000000000000000000000000000", // pathUSD
    "0x20c0000000000000000000000000000000000001", // AlphaUSD
    "0x20c0000000000000000000000000000000000002", // BetaUSD
    "0x20c0000000000000000000000000000000000003", // ThetaUSD
];

/// Verify a payment transaction on the Tempo chain.
///
/// Queries the Tempo RPC to confirm:
/// 1. Transaction exists and is confirmed
/// 2. Transfer is to the correct recipient
/// 3. Amount matches or exceeds the challenge amount
/// 4. Token is a recognized USD stablecoin (TIP-20)
async fn verify_payment_onchain(
    state: &AppState,
    credential: &MppCredential,
    challenge: &MppChallenge,
) -> anyhow::Result<bool> {
    if credential.tx_hash.is_empty() {
        return Ok(false);
    }

    // Development mode: accept any non-empty tx_hash
    if state.config.mpp_secret_key == "dev-secret-change-me" {
        tracing::warn!("DEV MODE: Accepting payment without on-chain verification");
        return Ok(true);
    }

    let tx_hash = &credential.tx_hash;
    let rpc_url = &state.config.tempo_rpc_url;

    info!(tx = %tx_hash, rpc = %rpc_url, "Verifying payment on Tempo");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

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

    // Check for RPC errors
    if let Some(error) = receipt.get("error") {
        warn!(error = %error, "Tempo RPC error");
        anyhow::bail!("Tempo RPC error: {}", error);
    }

    // Check receipt exists (tx might be pending or invalid)
    if receipt["result"].is_null() {
        warn!(tx = %tx_hash, "Transaction receipt not found (pending or invalid)");
        return Ok(false);
    }

    // Check transaction was successful
    let status = receipt["result"]["status"]
        .as_str()
        .unwrap_or("0x0");

    if status != "0x1" {
        warn!(tx = %tx_hash, status = %status, "Transaction failed on-chain");
        return Ok(false);
    }

    // Parse challenge amount (microdollars = TIP-20 smallest units, both 6 decimals)
    let required_amount: u64 = challenge.amount.parse().unwrap_or(0);

    // Check logs for Transfer event to our recipient
    let logs = receipt["result"]["logs"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // Transfer(address,address,uint256) event signature
    let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

    for log in &logs {
        let topics = match log["topics"].as_array() {
            Some(t) if t.len() >= 3 => t,
            _ => continue,
        };

        // Check this is a Transfer event
        if topics[0].as_str() != Some(transfer_topic) {
            continue;
        }

        // topics[2] is the `to` address (padded to 32 bytes)
        let to_raw = topics[2].as_str().unwrap_or_default();
        let to_addr = format!("0x{}", &to_raw[to_raw.len().saturating_sub(40)..]);

        if to_addr.to_lowercase() != challenge.recipient.to_lowercase() {
            continue;
        }

        // Verify the token contract is a recognized USD stablecoin
        let contract = log["address"].as_str().unwrap_or_default().to_lowercase();
        let is_accepted_stablecoin = contract == state.config.usdc_contract.to_lowercase()
            || TEMPO_USD_STABLECOINS
                .iter()
                .any(|s| s.to_lowercase() == contract);

        if !is_accepted_stablecoin {
            continue;
        }

        // Verify amount (data field contains the uint256 transfer amount)
        let data = log["data"].as_str().unwrap_or("0x0");
        let amount = u64::from_str_radix(data.trim_start_matches("0x").trim_start_matches('0'), 16)
            .unwrap_or(0);

        if amount >= required_amount {
            info!(
                tx = %tx_hash,
                amount,
                required = required_amount,
                token = %contract,
                "Payment verified on Tempo"
            );
            return Ok(true);
        } else {
            warn!(
                tx = %tx_hash,
                amount,
                required = required_amount,
                "Transfer amount insufficient"
            );
        }
    }

    warn!(tx = %tx_hash, "No matching Transfer event found in receipt");
    Ok(false)
}
