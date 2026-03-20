use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

/// VM status response.
#[derive(Debug, Serialize)]
pub struct VmStatusResponse {
    pub vm_id: String,
    pub status: String,
    pub vcpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub image: String,
    pub ip: Option<String>,
    pub ssh_port: Option<u16>,
    pub created_at: String,
    pub expires_at: String,
    pub terminated_at: Option<String>,
}

/// GET /v1/vms/:id
pub async fn get_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let vm_id = match id.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_vm_id"})),
            )
                .into_response()
        }
    };

    match state.vm_manager.get_vm(&vm_id) {
        Ok(Some(vm)) => (
            StatusCode::OK,
            Json(VmStatusResponse {
                vm_id: vm.id.to_string(),
                status: vm.status.as_str().to_string(),
                vcpus: vm.vcpus,
                ram_mb: vm.ram_mb,
                disk_gb: vm.disk_gb,
                image: vm.image,
                ip: vm.ip_addr,
                ssh_port: vm.ssh_port,
                created_at: vm.created_at.to_rfc3339(),
                expires_at: vm.expires_at.to_rfc3339(),
                terminated_at: vm.terminated_at.map(|t| t.to_rfc3339()),
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "vm_not_found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /v1/vms/:id
pub async fn delete_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let vm_id = match id.parse::<Uuid>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_vm_id"})),
            )
                .into_response()
        }
    };

    match state.vm_manager.terminate(&vm_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "terminated", "vm_id": vm_id.to_string()})),
        )
            .into_response(),
        Err(e) => {
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else if e.to_string().contains("not running") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (
                status,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}
