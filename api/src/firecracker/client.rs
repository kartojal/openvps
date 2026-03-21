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

/// Generate an Ed25519 SSH keypair and inject the public key into the rootfs.
/// Returns the private key in PEM format.
pub async fn inject_ssh_key(rootfs_path: &str, vm_dir: &str, vm_id: &str) -> Result<String> {
    let key_dir = format!("{}/{}", vm_dir, vm_id);
    let private_key_path = format!("{}/ssh_key", key_dir);
    let public_key_path = format!("{}/ssh_key.pub", key_dir);

    // Generate Ed25519 keypair
    let output = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-f")
        .arg(&private_key_path)
        .arg("-N")
        .arg("") // no passphrase
        .arg("-C")
        .arg(format!("mpp-vm-{}", vm_id))
        .output()
        .await
        .with_context(|| "Failed to generate SSH key")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ssh-keygen failed: {}", stderr);
    }

    // Read the public key
    let public_key = tokio::fs::read_to_string(&public_key_path)
        .await
        .with_context(|| "Failed to read public key")?;

    // Mount rootfs, inject key, unmount
    let mount_point = format!("{}/rootfs_mount", key_dir);
    std::fs::create_dir_all(&mount_point)?;

    let mount_result = Command::new("mount")
        .arg("-o")
        .arg("loop")
        .arg(rootfs_path)
        .arg(&mount_point)
        .output()
        .await
        .with_context(|| "Failed to mount rootfs")?;

    if !mount_result.status.success() {
        let stderr = String::from_utf8_lossy(&mount_result.stderr);
        anyhow::bail!("Failed to mount rootfs: {}", stderr);
    }

    // Ensure .ssh directory exists for root
    let ssh_dir = format!("{}/root/.ssh", mount_point);
    std::fs::create_dir_all(&ssh_dir)?;

    // Write authorized_keys
    tokio::fs::write(format!("{}/authorized_keys", ssh_dir), public_key.trim())
        .await
        .with_context(|| "Failed to write authorized_keys")?;

    // Set permissions
    Command::new("chmod")
        .arg("700")
        .arg(&ssh_dir)
        .output()
        .await?;
    Command::new("chmod")
        .arg("600")
        .arg(format!("{}/authorized_keys", ssh_dir))
        .output()
        .await?;

    // Enable root login and pubkey auth in sshd_config if it exists
    let sshd_config = format!("{}/etc/ssh/sshd_config", mount_point);
    if std::path::Path::new(&sshd_config).exists() {
        let config_content = tokio::fs::read_to_string(&sshd_config).await?;
        let mut new_config = config_content.clone();

        // Ensure PermitRootLogin is set to yes
        if new_config.contains("PermitRootLogin") {
            new_config = new_config
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("PermitRootLogin")
                        || line.trim_start().starts_with("#PermitRootLogin")
                    {
                        "PermitRootLogin prohibit-password"
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
        } else {
            new_config.push_str("\nPermitRootLogin prohibit-password\n");
        }

        // Ensure PubkeyAuthentication is enabled
        if !new_config.contains("PubkeyAuthentication yes") {
            new_config.push_str("PubkeyAuthentication yes\n");
        }

        tokio::fs::write(&sshd_config, new_config).await?;
    }

    // Unmount
    let umount_result = Command::new("umount")
        .arg(&mount_point)
        .output()
        .await
        .with_context(|| "Failed to unmount rootfs")?;

    if !umount_result.status.success() {
        let stderr = String::from_utf8_lossy(&umount_result.stderr);
        anyhow::bail!("Failed to unmount rootfs: {}", stderr);
    }

    // Clean up mount point
    std::fs::remove_dir(&mount_point).ok();

    // Read and return the private key
    let private_key = tokio::fs::read_to_string(&private_key_path)
        .await
        .with_context(|| "Failed to read private key")?;

    // Remove key files from disk (private key is returned to user, not stored)
    tokio::fs::remove_file(&private_key_path).await.ok();
    tokio::fs::remove_file(&public_key_path).await.ok();

    Ok(private_key)
}

/// Clean up VM state directory.
pub fn cleanup_vm_dir(vm_dir: &str, vm_id: &str) {
    let path = format!("{}/{}", vm_dir, vm_id);
    std::fs::remove_dir_all(&path).ok();
}
