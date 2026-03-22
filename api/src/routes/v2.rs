use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db::models::VmStatus;
use crate::vm::manager::ProvisionRequest;
use crate::AppState;

// ---------------------------------------------------------------------------
// POST /v2/provision — payment-gated, returns challenge instead of SSH key
// ---------------------------------------------------------------------------

/// Input body for v2 provision (same fields as v1).
#[derive(Debug, Deserialize)]
pub struct V2ProvisionInput {
    pub vcpus: Option<u32>,
    pub ram_mb: Option<u32>,
    pub disk_gb: Option<u32>,
    pub image: Option<String>,
    pub duration: Option<u64>,
}

/// Auth section of the v2 provision response.
#[derive(Debug, Serialize)]
pub struct V2AuthInfo {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub challenge: String,
    pub message: String,
}

/// V2 provision response — no SSH key, includes challenge.
#[derive(Debug, Serialize)]
pub struct V2ProvisionOutput {
    pub vm_id: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub status: String,
    pub expires_at: String,
    pub auth: V2AuthInfo,
}

/// Generate a random hex string of `n` bytes.
fn random_hex(n: usize) -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..n).map(|_| rng.gen()).collect();
    hex::encode(bytes)
}

pub async fn provision_v2(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<V2ProvisionInput>,
) -> impl IntoResponse {
    let vcpus = input.vcpus.unwrap_or(1).clamp(1, 4);
    let ram_mb = input.ram_mb.unwrap_or(512).clamp(256, 4096);
    let disk_gb = input.disk_gb.unwrap_or(10).clamp(1, 20);
    let duration = input.duration.unwrap_or(3600).clamp(60, 86400);

    // Validate image
    const ALLOWED_IMAGES: &[&str] = &["ubuntu-24.04"];
    let image = input.image.unwrap_or_else(|| "ubuntu-24.04".to_string());
    if !ALLOWED_IMAGES.contains(&image.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_image",
                "message": format!("Unsupported image. Available: {:?}", ALLOWED_IMAGES),
            })),
        )
            .into_response();
    }

    let price_micro = state
        .config
        .calculate_price_micro(vcpus, ram_mb, disk_gb, duration);

    // Extract payer address from MPP credential (set by payment middleware)
    let payer_address = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| crate::mpp::credential::MppCredential::from_authorization(auth))
        .map(|c| c.payer.clone());

    let payment_tx = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| crate::mpp::credential::MppCredential::from_authorization(auth))
        .map(|c| c.tx_hash);

    let req = ProvisionRequest {
        vcpus,
        ram_mb,
        disk_gb,
        image,
        duration_secs: duration,
        payment_tx,
        price_micro,
    };

    match state.vm_manager.provision(req).await {
        Ok(result) => {
            let vm_id = result.vm_id.to_string();
            let nonce = random_hex(16);
            let challenge = format!("openvps:{}:{}", vm_id, nonce);

            // Store payer address in the VM's payment_tx field if we have one.
            // We use a dedicated field approach: store "addr:<address>" so we can
            // distinguish it later. For PoC, we store the challenge in the DB
            // by updating payment_tx to include both tx_hash and payer info.
            if let Some(ref addr) = payer_address {
                // Update the VM record to store the payer address and challenge.
                // We piggyback on the existing payment_tx field with a compound value.
                if let Err(e) = state.vm_manager.db().update_vm_v2_auth(
                    &result.vm_id,
                    addr,
                    &challenge,
                ) {
                    warn!(vm_id = %vm_id, error = %e, "Failed to store v2 auth data (non-fatal)");
                }
            }

            info!(
                vm_id = %vm_id,
                ssh_host = %result.ssh_host,
                ssh_port = result.ssh_port,
                "V2 VM provisioned with wallet auth"
            );

            (
                StatusCode::CREATED,
                Json(V2ProvisionOutput {
                    vm_id,
                    ssh_host: result.ssh_host,
                    ssh_port: result.ssh_port,
                    status: "running".to_string(),
                    expires_at: result.expires_at.to_rfc3339(),
                    auth: V2AuthInfo {
                        auth_type: "wallet".to_string(),
                        challenge,
                        message: "Sign this challenge with the wallet that paid, then POST to /v2/session to get an SSH token".to_string(),
                    },
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "V2 provision failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "provision_failed",
                    "message": "VM provisioning failed. Please try again.",
                })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /v2/session — exchange signed challenge for one-time SSH token
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSessionInput {
    pub vm_id: String,
    pub signature: String,
    pub address: String,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionOutput {
    pub token: String,
    pub ssh_command: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub expires_in_seconds: i64,
    pub message: String,
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(input): Json<CreateSessionInput>,
) -> Response {
    // Parse VM ID
    let vm_id = match input.vm_id.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_vm_id"})),
            )
                .into_response()
        }
    };

    // Verify VM exists and is running
    let vm = match state.vm_manager.get_vm(&vm_id) {
        Ok(Some(vm)) => vm,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "vm_not_found"})),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to look up VM");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal_error"})),
            )
                .into_response()
        }
    };

    if vm.status != VmStatus::Running {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "vm_not_running",
                "status": vm.status.as_str(),
            })),
        )
            .into_response();
    }

    // Verify the address matches the payer on record.
    // For PoC: check address matches what we stored as v2 auth data.
    let (stored_payer, _stored_challenge) =
        match state.vm_manager.db().get_vm_v2_auth(&vm_id) {
            Ok(Some(data)) => data,
            Ok(None) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "no_v2_auth",
                        "message": "This VM was not provisioned via /v2/provision",
                    })),
                )
                    .into_response()
            }
            Err(e) => {
                error!(error = %e, "Failed to look up v2 auth data");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "internal_error"})),
                )
                    .into_response()
            }
        };

    // Verify the claimed address matches the payer (case-insensitive for hex addresses)
    if input.address.to_lowercase() != stored_payer.to_lowercase() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "address_mismatch",
                "message": "The provided address does not match the payer on record",
            })),
        )
            .into_response();
    }

    // PoC signature validation: just verify it's a non-empty hex-like string.
    // Real implementation would use ecrecover to verify EIP-191 signature over
    // the stored challenge matches the claimed address.
    if input.signature.is_empty()
        || (!input.signature.starts_with("0x") && !input.signature.starts_with("0X"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_signature",
                "message": "Signature must be a hex string starting with 0x",
            })),
        )
            .into_response();
    }

    // Verify the hex portion is valid
    let sig_hex = &input.signature[2..];
    if sig_hex.is_empty() || hex::decode(sig_hex).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_signature",
                "message": "Signature is not valid hex",
            })),
        )
            .into_response();
    }

    // Generate one-time token (32 bytes = 64 hex chars)
    let token = random_hex(32);

    // Token expiry: min(VM expiry, now + 1 hour)
    let now = Utc::now();
    let one_hour = now + Duration::hours(1);
    let token_expires = if vm.expires_at < one_hour {
        vm.expires_at
    } else {
        one_hour
    };
    let expires_in_seconds = (token_expires - now).num_seconds().max(0);

    // Store the token
    if let Err(e) = state.vm_manager.db().create_session_token(
        &token,
        &vm_id.to_string(),
        &input.address,
        &token_expires,
    ) {
        error!(error = %e, "Failed to create session token");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal_error"})),
        )
            .into_response();
    }

    let ssh_host = if state.config.public_ip != "0.0.0.0" {
        state.config.public_ip.clone()
    } else {
        vm.ip_addr.clone().unwrap_or_default()
    };
    let ssh_port = vm.ssh_port.unwrap_or(22);

    info!(
        vm_id = %vm_id,
        expires_in = expires_in_seconds,
        "Session token created for wallet-auth SSH"
    );

    (
        StatusCode::OK,
        Json(CreateSessionOutput {
            ssh_command: format!("sshpass -p '{}' ssh root@{} -p {}", token, ssh_host, ssh_port),
            ssh_host,
            ssh_port,
            token,
            expires_in_seconds,
            message: "Use this token as the SSH password. It can only be used once.".to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /v2/auth/verify — called by PAM on the VM to verify a token
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VerifyAuthInput {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyAuthOutput {
    pub authorized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
}

pub async fn verify_auth(
    State(state): State<AppState>,
    Json(input): Json<VerifyAuthInput>,
) -> Response {
    match state.vm_manager.db().verify_and_consume_token(&input.token) {
        Ok(Some((vm_id, _payer))) => {
            info!(vm_id = %vm_id, "Session token verified and consumed");
            (
                StatusCode::OK,
                Json(VerifyAuthOutput {
                    authorized: true,
                    vm_id: Some(vm_id),
                }),
            )
                .into_response()
        }
        Ok(None) => {
            warn!("Invalid, used, or expired session token presented");
            (
                StatusCode::OK,
                Json(VerifyAuthOutput {
                    authorized: false,
                    vm_id: None,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Token verification error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(VerifyAuthOutput {
                    authorized: false,
                    vm_id: None,
                }),
            )
                .into_response()
        }
    }
}
