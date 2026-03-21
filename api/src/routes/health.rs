use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::AppState;

pub async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "mpp-hosting",
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

/// GET /status — returns live capacity info for the landing page.
pub async fn status(State(state): State<AppState>) -> impl IntoResponse {
    // Server specs (hardcoded for now — single node)
    let total_vcpus: u32 = 8;
    let total_ram_mb: u32 = 64000;

    // Count running VMs and their resource usage
    let (running_vms, used_vcpus, used_ram_mb) = match state.vm_manager.running_stats() {
        Ok(stats) => stats,
        Err(_) => (0u32, 0u32, 0u32),
    };

    let available_vcpus = total_vcpus.saturating_sub(used_vcpus);
    let available_ram_mb = total_ram_mb.saturating_sub(used_ram_mb);
    // Rough estimate: each VM can use 1-8 vCPUs, so max slots = available_vcpus
    let max_slots = available_vcpus.max(1);

    (
        StatusCode::OK,
        Json(json!({
            "available_slots": max_slots,
            "running_vms": running_vms,
            "total_vcpus": total_vcpus,
            "available_vcpus": available_vcpus,
            "total_ram_mb": total_ram_mb,
            "available_ram_mb": available_ram_mb,
        })),
    )
}
