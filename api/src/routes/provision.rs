use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::vm::manager::ProvisionRequest;
use crate::AppState;

/// Provision request from an AI agent.
#[derive(Debug, Deserialize)]
pub struct ProvisionInput {
    /// Number of vCPUs (1-8)
    pub vcpus: Option<u32>,
    /// RAM in MB (256-16384)
    pub ram_mb: Option<u32>,
    /// Disk in GB (1-100)
    pub disk_gb: Option<u32>,
    /// OS image (e.g., "ubuntu-24.04")
    pub image: Option<String>,
    /// Duration in seconds (60-86400)
    pub duration: Option<u64>,
}

/// Provision response returned to the agent.
#[derive(Debug, Serialize)]
pub struct ProvisionOutput {
    pub vm_id: String,
    pub ip: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_command: String,
    pub expires_at: String,
    pub status: String,
    pub ssh_private_key: String,
}

/// POST /v1/provision
///
/// If called without MPP payment credential, returns 402 (handled by middleware).
/// If payment is verified, provisions a microVM and returns connection details.
pub async fn provision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ProvisionInput>,
) -> impl IntoResponse {
    let vcpus = input.vcpus.unwrap_or(1).clamp(1, 4);
    let ram_mb = input.ram_mb.unwrap_or(512).clamp(256, 4096);
    let disk_gb = input.disk_gb.unwrap_or(10).clamp(1, 20);
    let duration = input.duration.unwrap_or(3600).clamp(60, 86400);

    // Validate image against whitelist
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

    // Extract payment tx from credential (set by middleware)
    let payment_tx = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| {
            crate::mpp::credential::MppCredential::from_authorization(auth)
        })
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
            let ssh_command = format!(
                "ssh -p {} -i vm_key root@{}",
                result.ssh_port, result.ssh_host
            );
            (
                StatusCode::CREATED,
                Json(ProvisionOutput {
                    vm_id: result.vm_id.to_string(),
                    ip: result.ip,
                    ssh_host: result.ssh_host,
                    ssh_port: result.ssh_port,
                    ssh_command,
                    expires_at: result.expires_at.to_rfc3339(),
                    status: "running".to_string(),
                    ssh_private_key: result.ssh_private_key,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Provision failed");
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
