use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db::models::JobRecord;
use crate::vm::manager::ProvisionRequest;
use crate::AppState;

/// Request body for POST /v1/jobs.
#[derive(Debug, Deserialize)]
pub struct CreateJobInput {
    /// Command to run inside the VM.
    pub command: String,
    /// Optional setup script to run before the command.
    pub setup: Option<String>,
    /// Number of vCPUs (1-4, default 1).
    pub vcpus: Option<u32>,
    /// RAM in MB (256-4096, default 512).
    pub ram_mb: Option<u32>,
    /// Timeout in seconds (60-3600, default 300).
    pub timeout: Option<u32>,
    /// OS image (e.g., "ubuntu-24.04").
    pub image: Option<String>,
}

/// Response for POST /v1/jobs.
#[derive(Debug, Serialize)]
pub struct CreateJobOutput {
    pub job_id: String,
    pub status: String,
    pub poll_url: String,
    pub timeout: u32,
    pub expires_at: String,
}

/// Response for GET /v1/jobs/{id}.
#[derive(Debug, Serialize)]
pub struct GetJobOutput {
    pub job_id: String,
    pub status: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
    pub duration_secs: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// POST /v1/jobs — Create a job that provisions a VM, runs a command, captures output, and auto-terminates.
pub async fn create_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateJobInput>,
) -> Response {
    let vcpus = input.vcpus.unwrap_or(1).clamp(1, 4);
    let ram_mb = input.ram_mb.unwrap_or(512).clamp(256, 4096);
    let timeout_secs = input.timeout.unwrap_or(300).clamp(60, 3600);
    let image = input.image.unwrap_or_else(|| "ubuntu-24.04".to_string());

    // Validate image
    const ALLOWED_IMAGES: &[&str] = &["ubuntu-24.04"];
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

    let now = Utc::now();
    let expires_at = now + Duration::seconds(timeout_secs as i64);
    let job_id = Uuid::new_v4().to_string();

    // Calculate price (use disk_gb=10 as default for jobs)
    let disk_gb = 10u32;
    let price_micro = state
        .config
        .calculate_price_micro(vcpus, ram_mb, disk_gb, timeout_secs as u64);

    // Extract payment tx from credential (set by middleware)
    let payment_tx = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| {
            crate::mpp::credential::MppCredential::from_authorization(auth)
        })
        .map(|c| c.tx_hash);

    // Create job record
    let job = JobRecord {
        id: job_id.clone(),
        vm_id: None,
        status: "pending".to_string(),
        command: input.command.clone(),
        setup_script: input.setup.clone(),
        output: String::new(),
        exit_code: None,
        timeout_secs,
        vcpus,
        ram_mb,
        created_at: now,
        started_at: None,
        completed_at: None,
        expires_at,
        payment_tx,
        price_micro,
    };

    if let Err(e) = state.vm_manager.db().insert_job(&job) {
        error!(error = %e, "Failed to insert job record");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "internal_error",
                "message": "Failed to create job record.",
            })),
        )
            .into_response();
    }

    // Provision a VM
    let provision_req = ProvisionRequest {
        vcpus,
        ram_mb,
        disk_gb,
        image,
        duration_secs: timeout_secs as u64,
        payment_tx: job.payment_tx.clone(),
        price_micro,
    };

    let provision_result = match state.vm_manager.provision(provision_req).await {
        Ok(result) => result,
        Err(e) => {
            error!(job_id = %job_id, error = %e, "Failed to provision VM for job");
            // Mark job as failed
            if let Err(db_err) = state.vm_manager.db().update_job_completed(
                &job_id,
                Some(1),
                &format!("VM provisioning failed: {}", e),
                "failed",
            ) {
                error!(error = %db_err, "Failed to update job status");
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "provision_failed",
                    "message": "Failed to provision VM for job. Please try again.",
                })),
            )
                .into_response();
        }
    };

    let vm_id = provision_result.vm_id;
    // Use internal IP:22 for SSH from the server (DNAT only works for external traffic)
    let ssh_host = provision_result.ip.clone();
    let ssh_port: u16 = 22;
    let ssh_private_key = provision_result.ssh_private_key.clone();

    // Mark job as running
    if let Err(e) = state
        .vm_manager
        .db()
        .update_job_started(&job_id, &vm_id.to_string())
    {
        error!(error = %e, "Failed to update job as started");
    }

    info!(
        job_id = %job_id,
        vm_id = %vm_id,
        ssh_host = %ssh_host,
        ssh_port = ssh_port,
        timeout_secs = timeout_secs,
        "Job started, spawning command execution"
    );

    // Spawn background task to execute the command
    let state_bg = state.clone();
    let job_id_bg = job_id.clone();
    let command = input.command.clone();
    let setup_script = input.setup.clone();

    tokio::spawn(async move {
        execute_job(
            &state_bg,
            &job_id_bg,
            &vm_id,
            &ssh_host,
            ssh_port,
            &ssh_private_key,
            &command,
            setup_script.as_deref(),
            timeout_secs,
        )
        .await;
    });

    // Return immediately
    (
        StatusCode::CREATED,
        Json(CreateJobOutput {
            job_id,
            status: "running".to_string(),
            poll_url: format!("/v1/jobs/{}", job.id),
            timeout: timeout_secs,
            expires_at: expires_at.to_rfc3339(),
        }),
    )
        .into_response()
}

/// GET /v1/jobs/{id} — Poll for job status and output.
pub async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.vm_manager.db().get_job(&id) {
        Ok(Some(job)) => {
            let duration_secs = job.started_at.map(|started| {
                let end = job.completed_at.unwrap_or_else(Utc::now);
                (end - started).num_seconds()
            });

            (
                StatusCode::OK,
                Json(GetJobOutput {
                    job_id: job.id,
                    status: job.status,
                    command: job.command,
                    exit_code: job.exit_code,
                    output: job.output,
                    duration_secs,
                    created_at: job.created_at.to_rfc3339(),
                    started_at: job.started_at.map(|t| t.to_rfc3339()),
                    completed_at: job.completed_at.map(|t| t.to_rfc3339()),
                }),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "not_found",
                "message": "Job not found",
            })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get job");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal_error",
                    "message": "Failed to retrieve job.",
                })),
            )
                .into_response()
        }
    }
}

/// Execute a job: SSH into the VM, run setup + command, capture output, terminate VM.
async fn execute_job(
    state: &AppState,
    job_id: &str,
    vm_id: &Uuid,
    ssh_host: &str,
    ssh_port: u16,
    ssh_private_key: &str,
    command: &str,
    setup_script: Option<&str>,
    timeout_secs: u32,
) {
    let key_path = format!("/tmp/job_{}_key", job_id);

    // Write private key to temp file
    if let Err(e) = tokio::fs::write(&key_path, ssh_private_key).await {
        error!(job_id = %job_id, error = %e, "Failed to write SSH key file");
        update_job_failed(state, job_id, vm_id, &format!("Failed to write SSH key: {}", e)).await;
        return;
    }

    // Set key file permissions to 600
    if let Err(e) = tokio::fs::set_permissions(&key_path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).await {
        error!(job_id = %job_id, error = %e, "Failed to set key file permissions");
        cleanup_and_fail(state, job_id, vm_id, &key_path, &format!("Failed to set key permissions: {}", e)).await;
        return;
    }

    // Wait for VM to boot and SSH to become available
    // Try connecting every 2s for up to 30s
    let mut ssh_ready = false;
    for attempt in 1..=15 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match run_ssh_command(ssh_host, ssh_port, &key_path, "true").await {
            Ok((0, _)) => {
                info!(job_id = %job_id, attempt, "SSH ready");
                ssh_ready = true;
                break;
            }
            _ => {
                if attempt % 5 == 0 {
                    info!(job_id = %job_id, attempt, "Waiting for SSH...");
                }
            }
        }
    }
    if !ssh_ready {
        error!(job_id = %job_id, "SSH not available after 30s");
        cleanup_and_fail(state, job_id, vm_id, &key_path, "SSH not available after 30 seconds").await;
        return;
    }

    let timeout_duration = std::time::Duration::from_secs(timeout_secs as u64);

    let result = tokio::time::timeout(timeout_duration, async {
        let mut combined_output = String::new();

        // Run setup script if provided
        if let Some(setup) = setup_script {
            info!(job_id = %job_id, "Running setup script");
            match run_ssh_command(ssh_host, ssh_port, &key_path, setup).await {
                Ok((exit_code, output)) => {
                    combined_output.push_str(&format!("=== SETUP (exit {}) ===\n{}\n", exit_code, output));
                    if exit_code != 0 {
                        warn!(job_id = %job_id, exit_code, "Setup script failed");
                        return (Some(exit_code), combined_output);
                    }
                }
                Err(e) => {
                    combined_output.push_str(&format!("=== SETUP ERROR ===\n{}\n", e));
                    return (Some(1), combined_output);
                }
            }
        }

        // Run the main command
        info!(job_id = %job_id, "Running main command");
        match run_ssh_command(ssh_host, ssh_port, &key_path, command).await {
            Ok((exit_code, output)) => {
                combined_output.push_str(&output);
                (Some(exit_code), combined_output)
            }
            Err(e) => {
                combined_output.push_str(&format!("Command execution error: {}", e));
                (Some(1), combined_output)
            }
        }
    })
    .await;

    // Clean up key file
    if let Err(e) = tokio::fs::remove_file(&key_path).await {
        warn!(job_id = %job_id, error = %e, "Failed to remove temp key file");
    }

    match result {
        Ok((exit_code, output)) => {
            let status = if exit_code == Some(0) {
                "completed"
            } else {
                "failed"
            };
            info!(job_id = %job_id, status, exit_code = ?exit_code, "Job finished");
            if let Err(e) = state.vm_manager.db().update_job_completed(job_id, exit_code, &output, status) {
                error!(job_id = %job_id, error = %e, "Failed to update job completion");
            }
        }
        Err(_) => {
            warn!(job_id = %job_id, timeout_secs, "Job timed out");
            if let Err(e) = state.vm_manager.db().update_job_completed(
                job_id,
                None,
                "Job timed out",
                "timeout",
            ) {
                error!(job_id = %job_id, error = %e, "Failed to update job timeout");
            }
        }
    }

    // Terminate the VM
    if let Err(e) = state.vm_manager.terminate(vm_id).await {
        error!(job_id = %job_id, vm_id = %vm_id, error = %e, "Failed to terminate job VM");
    }
}

/// Run a single command via SSH and capture stdout+stderr.
async fn run_ssh_command(
    host: &str,
    port: u16,
    key_path: &str,
    command: &str,
) -> anyhow::Result<(i32, String)> {
    let output = tokio::process::Command::new("ssh")
        .arg("-p")
        .arg(port.to_string())
        .arg("-i")
        .arg(key_path)
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(format!("root@{}", host))
        .arg(command)
        .output()
        .await?;

    let exit_code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{}{}", stdout, stderr)
    };

    Ok((exit_code, combined))
}

/// Helper: mark a job as failed and terminate the VM.
async fn update_job_failed(state: &AppState, job_id: &str, vm_id: &Uuid, message: &str) {
    if let Err(e) = state.vm_manager.db().update_job_completed(job_id, Some(1), message, "failed") {
        error!(job_id = %job_id, error = %e, "Failed to update job as failed");
    }
    if let Err(e) = state.vm_manager.terminate(vm_id).await {
        error!(job_id = %job_id, vm_id = %vm_id, error = %e, "Failed to terminate job VM after failure");
    }
}

/// Helper: clean up key file, mark job as failed, terminate VM.
async fn cleanup_and_fail(state: &AppState, job_id: &str, vm_id: &Uuid, key_path: &str, message: &str) {
    let _ = tokio::fs::remove_file(key_path).await;
    update_job_failed(state, job_id, vm_id, message).await;
}
