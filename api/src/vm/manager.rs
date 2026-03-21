use anyhow::Result;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::db::models::{VmRecord, VmStatus};
use crate::db::Database;
use crate::firecracker::client::{cleanup_vm_dir, inject_ssh_key, prepare_rootfs, FirecrackerVm};
use crate::network::ip_pool::IpPool;
use crate::network::tap;

/// Request to provision a new VM.
#[derive(Debug)]
pub struct ProvisionRequest {
    pub vcpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub image: String,
    pub duration_secs: u64,
    pub payment_tx: Option<String>,
    pub price_micro: u64,
}

/// Result of a successful provision.
#[derive(Debug)]
pub struct ProvisionResult {
    pub vm_id: Uuid,
    pub ip: String,
    pub ssh_port: u16,
    pub expires_at: chrono::DateTime<Utc>,
    pub ssh_private_key: String,
}

/// Manages the lifecycle of all VMs.
pub struct VmManager {
    config: Config,
    db: Arc<Database>,
    ip_pool: Arc<IpPool>,
    /// Active Firecracker processes, keyed by VM ID
    active_vms: Mutex<HashMap<Uuid, FirecrackerVm>>,
}

impl VmManager {
    pub fn new(config: Config, db: Arc<Database>, ip_pool: Arc<IpPool>) -> Self {
        Self {
            config,
            db,
            ip_pool,
            active_vms: Mutex::new(HashMap::new()),
        }
    }

    /// Provision a new microVM.
    pub async fn provision(&self, req: ProvisionRequest) -> Result<ProvisionResult> {
        let vm_id = Uuid::new_v4();
        let now = Utc::now();
        let expires_at = now + Duration::seconds(req.duration_secs as i64);

        // Allocate IP
        let vm_ip = self.ip_pool.allocate()?;
        let vm_ip_str = vm_ip.to_string();

        // Create DB record (must come before allocate_ip due to FK constraint)
        let record = VmRecord {
            id: vm_id,
            status: VmStatus::Pending,
            vcpus: req.vcpus,
            ram_mb: req.ram_mb,
            disk_gb: req.disk_gb,
            image: req.image.clone(),
            ip_addr: Some(vm_ip_str.clone()),
            ssh_port: Some(22),
            tap_device: None,
            socket_path: None,
            pid: None,
            payment_tx: req.payment_tx.clone(),
            price_micro: req.price_micro,
            created_at: now,
            expires_at,
            terminated_at: None,
        };
        self.db.insert_vm(&record)?;
        self.db.allocate_ip(&vm_ip_str, &vm_id)?;

        // Prepare rootfs copy
        let rootfs_path =
            prepare_rootfs(&self.config.rootfs_path, &self.config.vm_state_dir, &vm_id.to_string())
                .await?;

        // Generate SSH keypair and inject public key into rootfs
        let ssh_private_key = inject_ssh_key(
            &rootfs_path,
            &self.config.vm_state_dir,
            &vm_id.to_string(),
        )
        .await?;

        // Create TAP device
        let tap_name = tap::tap_name(&vm_id.to_string());
        let guest_mac = tap::generate_mac(vm_ip);

        if let Err(e) = tap::create_tap(
            &tap_name,
            vm_ip,
            self.ip_pool.gateway(),
            self.ip_pool.prefix_len(),
            &self.config.bridge_iface,
        )
        .await
        {
            // Clean up on failure
            self.ip_pool.release(vm_ip);
            self.db.release_ip(&vm_ip_str)?;
            self.db.update_vm_status(&vm_id, VmStatus::Failed)?;
            cleanup_vm_dir(&self.config.vm_state_dir, &vm_id.to_string());
            return Err(e);
        }

        // Spawn Firecracker process
        let socket_path = format!(
            "{}/{}/firecracker.sock",
            self.config.vm_state_dir,
            vm_id
        );
        let log_path = format!("{}/{}/firecracker.log", self.config.vm_state_dir, vm_id);

        let fc = match FirecrackerVm::spawn(
            &self.config.firecracker_bin,
            std::path::Path::new(&socket_path),
            std::path::Path::new(&log_path),
        )
        .await
        {
            Ok(fc) => fc,
            Err(e) => {
                tap::destroy_tap(&tap_name).await.ok();
                self.ip_pool.release(vm_ip);
                self.db.release_ip(&vm_ip_str)?;
                self.db.update_vm_status(&vm_id, VmStatus::Failed)?;
                cleanup_vm_dir(&self.config.vm_state_dir, &vm_id.to_string());
                return Err(e);
            }
        };

        // Configure the microVM
        let gateway = self.ip_pool.gateway();
        let boot_args = format!(
            "console=ttyS0 reboot=k panic=1 ip={}::{}:{}::eth0:off",
            vm_ip, gateway, self.ip_pool.netmask()
        );

        let configure_result = async {
            fc.set_boot_source(&self.config.kernel_path, &boot_args)
                .await?;
            fc.set_machine_config(req.vcpus, req.ram_mb).await?;
            fc.set_rootfs("rootfs", &rootfs_path).await?;
            fc.set_network("net1", &guest_mac, &tap_name).await?;
            fc.start().await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(e) = configure_result {
            error!(vm_id = %vm_id, error = %e, "Failed to configure/start microVM");
            // fc will be dropped and process killed
            tap::destroy_tap(&tap_name).await.ok();
            self.ip_pool.release(vm_ip);
            self.db.release_ip(&vm_ip_str)?;
            self.db.update_vm_status(&vm_id, VmStatus::Failed)?;
            cleanup_vm_dir(&self.config.vm_state_dir, &vm_id.to_string());
            return Err(e);
        }

        let pid = fc.pid().unwrap_or(0);

        // Update DB with runtime info
        self.db
            .update_vm_runtime(&vm_id, &vm_ip_str, &tap_name, &socket_path, pid)?;

        // Store active VM handle
        self.active_vms.lock().await.insert(vm_id, fc);

        info!(
            vm_id = %vm_id,
            ip = %vm_ip,
            vcpus = req.vcpus,
            ram_mb = req.ram_mb,
            "MicroVM provisioned"
        );

        Ok(ProvisionResult {
            vm_id,
            ip: vm_ip_str,
            ssh_port: 22,
            expires_at,
            ssh_private_key,
        })
    }

    /// Terminate a running VM.
    pub async fn terminate(&self, vm_id: &Uuid) -> Result<()> {
        let vm = self
            .db
            .get_vm(vm_id)?
            .ok_or_else(|| anyhow::anyhow!("VM not found"))?;

        if vm.status != VmStatus::Running {
            anyhow::bail!("VM is not running (status: {})", vm.status.as_str());
        }

        // Kill the Firecracker process
        if let Some(mut fc) = self.active_vms.lock().await.remove(vm_id) {
            fc.terminate().await?;
        } else if let Some(pid) = vm.pid {
            // Process not tracked but we have PID — try to kill
            tokio::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .output()
                .await
                .ok();
        }

        // Destroy TAP device
        if let Some(ref tap_name) = vm.tap_device {
            tap::destroy_tap(tap_name).await.ok();
        }

        // Release IP
        if let Some(ref ip) = vm.ip_addr {
            self.db.release_ip(ip)?;
            if let Ok(addr) = ip.parse() {
                self.ip_pool.release(addr);
            }
        }

        // Update status
        self.db.update_vm_status(vm_id, VmStatus::Terminated)?;

        // Clean up VM directory
        cleanup_vm_dir(&self.config.vm_state_dir, &vm_id.to_string());

        info!(vm_id = %vm_id, "VM terminated");
        Ok(())
    }

    /// Background task: terminate expired VMs.
    pub async fn cleanup_expired(&self) {
        match self.db.list_expired_running_vms() {
            Ok(expired) => {
                for vm in expired {
                    warn!(vm_id = %vm.id, expires_at = %vm.expires_at, "Terminating expired VM");
                    if let Err(e) = self.terminate(&vm.id).await {
                        error!(vm_id = %vm.id, error = %e, "Failed to terminate expired VM");
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to list expired VMs");
            }
        }
    }

    /// Get VM info.
    pub fn get_vm(&self, vm_id: &Uuid) -> Result<Option<VmRecord>> {
        self.db.get_vm(vm_id)
    }
}
