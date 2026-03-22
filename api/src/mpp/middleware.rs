use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;
use tracing::{info, warn};

use super::challenge::MppChallenge;
use super::credential::MppCredential;
use super::receipt::MppReceipt;
use crate::x402::facilitator::FacilitatorClient;
use crate::x402::types::{
    PaymentPayload, PaymentRequired, PaymentRequirements, ResourceInfo, SettlementResponse,
};
use crate::AppState;

/// Request body for provision endpoint — parsed to calculate price.
#[derive(Debug, Deserialize)]
struct ProvisionBody {
    vcpus: Option<u32>,
    ram_mb: Option<u32>,
    disk_gb: Option<u32>,
    duration: Option<u64>,
}

/// Unified payment middleware supporting both x402 and MPP protocols.
///
/// Detection order:
/// 1. `PAYMENT-SIGNATURE` header → x402 flow
/// 2. `Authorization: Payment` header → MPP flow
/// 3. Neither → return 402 with both PAYMENT-REQUIRED and WWW-Authenticate headers
pub async fn mpp_payment_gate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    // --- x402 flow: check for PAYMENT-SIGNATURE header ---
    if let Some(payment_sig) = headers
        .get("PAYMENT-SIGNATURE")
        .or_else(|| headers.get("payment-signature"))
        .and_then(|v| v.to_str().ok())
    {
        return handle_x402_payment(&state, payment_sig, request, next).await;
    }

    // --- MPP flow: check for Authorization: Payment header ---
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

                            // Prevent challenge replay: mark as consumed
                            match state.vm_manager.db().consume_challenge(&challenge.id) {
                                Ok(true) => {} // First use — proceed
                                Ok(false) => {
                                    warn!(challenge_id = %challenge.id, "Challenge already used (replay attempt)");
                                    return (
                                        StatusCode::CONFLICT,
                                        "Challenge already used",
                                    )
                                        .into_response();
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to check challenge usage");
                                    return (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "Internal error",
                                    )
                                        .into_response();
                                }
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
            // --- No payment credential — return 402 with both x402 and MPP challenges ---
            let (_parts, body) = request.into_parts();

            let body_bytes = match axum::body::to_bytes(body, 1024 * 64).await {
                Ok(b) => b,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid request body").into_response()
                }
            };

            let provision: ProvisionBody = serde_json::from_slice(&body_bytes)
                .unwrap_or(ProvisionBody {
                    vcpus: None,
                    ram_mb: None,
                    disk_gb: None,
                    duration: None,
                });

            let vcpus = provision.vcpus.unwrap_or(1);
            let ram_mb = provision.ram_mb.unwrap_or(512);
            let disk_gb = provision.disk_gb.unwrap_or(10);
            let duration = provision.duration.unwrap_or(3600);

            let price_micro = state
                .config
                .calculate_price_micro(vcpus, ram_mb, disk_gb, duration);

            // Build MPP challenge
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

            // Build x402 PaymentRequired
            let price_str = price_micro.to_string();
            let payment_required = build_payment_required(&state, &price_str);
            let payment_required_json = serde_json::to_string(&payment_required).unwrap_or_default();
            let payment_required_b64 = base64::engine::general_purpose::STANDARD.encode(&payment_required_json);

            info!(
                price_micro,
                vcpus, ram_mb, disk_gb, duration, "Issuing payment challenge (MPP + x402)"
            );

            Response::builder()
                .status(StatusCode::PAYMENT_REQUIRED)
                .header("www-authenticate", &www_auth)
                .header("PAYMENT-REQUIRED", &payment_required_b64)
                .header("content-type", "application/json")
                .body(Body::from(challenge_json))
                .unwrap()
        }
    }
}

/// Handle x402 payment flow: decode PAYMENT-SIGNATURE, verify, settle, and continue.
async fn handle_x402_payment(
    state: &AppState,
    payment_sig_b64: &str,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Decode the base64 PAYMENT-SIGNATURE header into PaymentPayload
    let payload: PaymentPayload = match decode_payment_signature(payment_sig_b64) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Invalid PAYMENT-SIGNATURE header");
            return (StatusCode::BAD_REQUEST, format!("Invalid PAYMENT-SIGNATURE: {}", e))
                .into_response();
        }
    };

    // Find matching PaymentRequirements from our config
    let requirements = match find_matching_requirements(state, &payload) {
        Some(r) => r,
        None => {
            warn!(
                network = %payload.accepted.network,
                asset = %payload.accepted.asset,
                "No matching payment requirements for x402 payload"
            );
            return (
                StatusCode::BAD_REQUEST,
                "No matching payment requirements for the provided network/asset",
            )
                .into_response();
        }
    };

    let facilitator = FacilitatorClient::new(&state.config.x402_facilitator_url);

    // Step 1: Verify the payment with the facilitator
    match facilitator.verify(&payload, &requirements).await {
        Ok(verify_resp) => {
            if !verify_resp.is_valid {
                let reason = verify_resp.invalid_reason.unwrap_or_else(|| "unknown".to_string());
                warn!(reason = %reason, "x402 payment verification failed");

                let error_required = build_payment_required_with_error(
                    state,
                    &payload.accepted.amount,
                    &format!("Payment verification failed: {}", reason),
                );
                let error_b64 = base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_string(&error_required).unwrap_or_default());

                return Response::builder()
                    .status(StatusCode::PAYMENT_REQUIRED)
                    .header("PAYMENT-REQUIRED", &error_b64)
                    .body(Body::from(format!(
                        "{{\"error\":\"payment_verification_failed\",\"reason\":\"{}\"}}",
                        reason
                    )))
                    .unwrap();
            }
        }
        Err(e) => {
            warn!(error = %e, "x402 facilitator /verify error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Facilitator verification error: {}", e),
            )
                .into_response();
        }
    }

    // Step 2: Settle the payment with the facilitator
    let settlement: SettlementResponse = match facilitator.settle(&payload, &requirements).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "x402 facilitator /settle error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Facilitator settlement error: {}", e),
            )
                .into_response();
        }
    };

    if !settlement.success {
        let reason = settlement
            .error_reason
            .clone()
            .unwrap_or_else(|| "settlement failed".to_string());
        warn!(reason = %reason, "x402 settlement failed");

        let error_required = build_payment_required_with_error(
            state,
            &payload.accepted.amount,
            &format!("Settlement failed: {}", reason),
        );
        let error_b64 = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&error_required).unwrap_or_default());

        return Response::builder()
            .status(StatusCode::PAYMENT_REQUIRED)
            .header("PAYMENT-REQUIRED", &error_b64)
            .body(Body::from(format!(
                "{{\"error\":\"settlement_failed\",\"reason\":\"{}\"}}",
                reason
            )))
            .unwrap();
    }

    info!(
        tx = %settlement.transaction,
        network = %settlement.network,
        payer = ?settlement.payer,
        "x402 payment settled successfully"
    );

    // Encode settlement response as base64 for PAYMENT-RESPONSE header
    let settlement_b64 = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_string(&settlement).unwrap_or_default());

    // Pass through to handler
    let mut response = next.run(request).await;

    // Attach PAYMENT-RESPONSE header
    response
        .headers_mut()
        .insert("PAYMENT-RESPONSE", settlement_b64.parse().unwrap());

    response
}

/// Decode a base64-encoded PAYMENT-SIGNATURE header into a PaymentPayload.
fn decode_payment_signature(b64: &str) -> anyhow::Result<PaymentPayload> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| anyhow::anyhow!("base64 decode failed: {}", e))?;
    let payload: PaymentPayload = serde_json::from_slice(&decoded)
        .map_err(|e| anyhow::anyhow!("JSON parse failed: {}", e))?;
    Ok(payload)
}

/// Find matching PaymentRequirements from our config for the client's accepted terms.
fn find_matching_requirements(
    state: &AppState,
    payload: &PaymentPayload,
) -> Option<PaymentRequirements> {
    let accepted = &payload.accepted;

    // Match on network + asset
    let base_match = accepted.network == "eip155:8453"
        && accepted.asset.to_lowercase() == state.config.x402_base_asset.to_lowercase();
    let celo_match = accepted.network == "eip155:42220"
        && accepted.asset.to_lowercase() == state.config.x402_celo_asset.to_lowercase();

    if base_match {
        Some(PaymentRequirements {
            scheme: "exact".to_string(),
            network: "eip155:8453".to_string(),
            amount: accepted.amount.clone(),
            asset: state.config.x402_base_asset.clone(),
            pay_to: state.config.payment_recipient.clone(),
            max_timeout_seconds: 300,
            extra: Some(serde_json::json!({"name": "USDC", "version": "2"})),
        })
    } else if celo_match {
        Some(PaymentRequirements {
            scheme: "exact".to_string(),
            network: "eip155:42220".to_string(),
            amount: accepted.amount.clone(),
            asset: state.config.x402_celo_asset.clone(),
            pay_to: state.config.payment_recipient.clone(),
            max_timeout_seconds: 300,
            extra: Some(serde_json::json!({"name": "USDC"})),
        })
    } else {
        None
    }
}

/// Build the x402 PaymentRequired structure for the 402 response.
fn build_payment_required(state: &AppState, amount: &str) -> PaymentRequired {
    build_payment_required_inner(state, amount, None)
}

/// Build a PaymentRequired with an error message (for failed payment retries).
fn build_payment_required_with_error(
    state: &AppState,
    amount: &str,
    error: &str,
) -> PaymentRequired {
    build_payment_required_inner(state, amount, Some(error.to_string()))
}

fn build_payment_required_inner(
    state: &AppState,
    amount: &str,
    error: Option<String>,
) -> PaymentRequired {
    let accepts = vec![
        PaymentRequirements {
            scheme: "exact".to_string(),
            network: "eip155:8453".to_string(),
            amount: amount.to_string(),
            asset: state.config.x402_base_asset.clone(),
            pay_to: state.config.payment_recipient.clone(),
            max_timeout_seconds: 300,
            extra: Some(serde_json::json!({"name": "USDC", "version": "2"})),
        },
        PaymentRequirements {
            scheme: "exact".to_string(),
            network: "eip155:42220".to_string(),
            amount: amount.to_string(),
            asset: state.config.x402_celo_asset.clone(),
            pay_to: state.config.payment_recipient.clone(),
            max_timeout_seconds: 300,
            extra: Some(serde_json::json!({"name": "USDC"})),
        },
    ];

    let input_schema = serde_json::json!({
        "type": "object",
        "properties": {
            "vcpus": { "type": "integer", "minimum": 1, "maximum": 4, "default": 1, "description": "Number of vCPUs" },
            "ram_mb": { "type": "integer", "minimum": 256, "maximum": 4096, "default": 512, "description": "RAM in megabytes" },
            "disk_gb": { "type": "integer", "minimum": 1, "maximum": 20, "default": 10, "description": "Disk in gigabytes" },
            "image": { "type": "string", "default": "ubuntu-24.04", "description": "OS image" },
            "duration": { "type": "integer", "minimum": 60, "maximum": 86400, "default": 3600, "description": "Duration in seconds" }
        }
    });

    PaymentRequired {
        x402_version: 2,
        error,
        resource: ResourceInfo {
            url: "/v1/provision".to_string(),
            description: "Provision a Firecracker microVM with SSH access".to_string(),
            mime_type: "application/json".to_string(),
            method: Some("POST".to_string()),
            input_schema: Some(input_schema.clone()),
        },
        accepts,
        extensions: Some(serde_json::json!({
            "bazaar": {
                "info": {
                    "title": "OpenVPS",
                    "description": "AI-agent VPS hosting. Pay with stablecoins, get root SSH to Ubuntu 24.04 microVMs in seconds.",
                    "skillUrl": "https://openvps.sh/skill.md"
                },
                "schema": {
                    "properties": {
                        "input": {
                            "properties": {
                                "body": input_schema
                            }
                        },
                        "output": {
                            "properties": {
                                "example": {
                                    "type": "object",
                                    "properties": {
                                        "vm_id": { "type": "string" },
                                        "ssh_host": { "type": "string" },
                                        "ssh_port": { "type": "integer" },
                                        "ssh_command": { "type": "string" },
                                        "ssh_private_key": { "type": "string" },
                                        "expires_at": { "type": "string" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })),
    }
}

/// Known USD-denominated stablecoins on Tempo.
/// Any of these are accepted as payment.
const TEMPO_USD_STABLECOINS: &[&str] = &[
    "0x20c000000000000000000000b9537d11c60e8b50", // USDC.e (Bridged USDC Stargate)
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

    // Development mode: only enabled via explicit MPP_DEV_MODE=true env var
    if state.config.mpp_dev_mode {
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
