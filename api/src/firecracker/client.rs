use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tracing::{error, info};

use super::models::*;

/// Manages a single Firecracker microVM process and its API.
pub struct FirecrackerVm {
    socket_path: PathBuf,
    process: Option<Child>,
    _firecracker_bin: String,
}

impl FirecrackerVm {
    /// Spawn a new Firecracker process with the given socket path.
    pub async fn spawn(
        firecracker_bin: &str,
        socket_path: &Path,
        _log_path: &Path,
    ) -> Result<Self> {
        // Remove stale socket
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }

        // Ensure parent dirs exist
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let process = Command::new(firecracker_bin)
            .arg("--api-sock")
            .arg(socket_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn firecracker at {}", firecracker_bin))?;

        let vm = Self {
            socket_path: socket_path.to_path_buf(),
            process: Some(process),
            _firecracker_bin: firecracker_bin.to_string(),
        };

        // Wait for socket to appear
        for _ in 0..50 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        if !socket_path.exists() {
            anyhow::bail!(
                "Firecracker socket did not appear at {}",
                socket_path.display()
            );
        }

        info!(socket = %socket_path.display(), "Firecracker process started");
        Ok(vm)
    }

    /// Send a PUT request to the Firecracker API via Unix socket.
    async fn api_put(&self, path: &str, body: &impl serde::Serialize) -> Result<()> {
        let body_json = serde_json::to_string(body)?;

        let output = Command::new("curl")
            .arg("-s")
            .arg("-X")
            .arg("PUT")
            .arg("--unix-socket")
            .arg(&self.socket_path)
            .arg("--data")
            .arg(&body_json)
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg(format!("http://localhost{}", path))
            .output()
            .await
            .with_context(|| format!("Failed to call Firecracker API: PUT {}", path))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!(path, %stderr, %stdout, "Firecracker API error");
            anyhow::bail!("Firecracker API PUT {} failed: {}", path, stdout);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("fault_message") {
            anyhow::bail!("Firecracker API PUT {} error: {}", path, stdout);
        }

        Ok(())
    }

    /// Configure the boot source.
    pub async fn set_boot_source(&self, kernel_path: &str, boot_args: &str) -> Result<()> {
        let boot = BootSource {
            kernel_image_path: kernel_path.to_string(),
            boot_args: boot_args.to_string(),
        };
        self.api_put("/boot-source", &boot).await
    }

    /// Configure machine resources.
    pub async fn set_machine_config(&self, vcpus: u32, mem_mib: u32) -> Result<()> {
        let config = MachineConfig {
            vcpu_count: vcpus,
            mem_size_mib: mem_mib,
        };
        self.api_put("/machine-config", &config).await
    }

    /// Attach a root filesystem drive.
    pub async fn set_rootfs(&self, drive_id: &str, path: &str) -> Result<()> {
        let drive = Drive {
            drive_id: drive_id.to_string(),
            path_on_host: path.to_string(),
            is_root_device: true,
            is_read_only: false,
        };
        self.api_put(&format!("/drives/{}", drive_id), &drive).await
    }

    /// Configure a network interface.
    pub async fn set_network(
        &self,
        iface_id: &str,
        guest_mac: &str,
        tap_device: &str,
    ) -> Result<()> {
        let iface = NetworkInterface {
            iface_id: iface_id.to_string(),
            guest_mac: guest_mac.to_string(),
            host_dev_name: tap_device.to_string(),
        };
        self.api_put(&format!("/network-interfaces/{}", iface_id), &iface)
            .await
    }

    /// Start the microVM instance.
    pub async fn start(&self) -> Result<()> {
        let action = InstanceAction {
            action_type: "InstanceStart".to_string(),
        };
        self.api_put("/actions", &action).await?;
        info!(socket = %self.socket_path.display(), "MicroVM started");
        Ok(())
    }

    /// Get the PID of the Firecracker process.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|p| p.id())
    }

    /// Get the socket path.
    #[allow(dead_code)]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Kill the Firecracker process and clean up.
    pub async fn terminate(&mut self) -> Result<()> {
        if let Some(ref mut process) = self.process {
            process.kill().await.ok();
            process.wait().await.ok();
            info!(socket = %self.socket_path.display(), "MicroVM terminated");
        }

        // Clean up socket
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path).ok();
        }

        self.process = None;
        Ok(())
    }
}

impl Drop for FirecrackerVm {
    fn drop(&mut self) {
        // Best-effort sync cleanup
        if let Some(ref mut process) = self.process {
            process.start_kill().ok();
        }
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path).ok();
        }
    }
}

/// Prepare a copy-on-write rootfs for a VM by copying the base image.
pub async fn prepare_rootfs(base_rootfs: &str, vm_dir: &str, vm_id: &str) -> Result<String> {
    let dest = format!("{}/{}/rootfs.ext4", vm_dir, vm_id);

    // Create VM directory
    std::fs::create_dir_all(format!("{}/{}", vm_dir, vm_id))?;

    // Copy the base rootfs (in production, use reflink/cp --reflink=auto for CoW)
    let output = Command::new("cp")
        .arg("--reflink=auto")
        .arg(base_rootfs)
        .arg(&dest)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {}
        _ => {
            // Fallback: regular copy (macOS doesn't support reflink)
            Command::new("cp")
                .arg(base_rootfs)
                .arg(&dest)
                .output()
                .await
                .with_context(|| "Failed to copy rootfs")?;
        }
    }

    Ok(dest)
}

/// Clean up VM state directory.
pub fn cleanup_vm_dir(vm_dir: &str, vm_id: &str) {
    let path = format!("{}/{}", vm_dir, vm_id);
    std::fs::remove_dir_all(&path).ok();
}
