use serde::{Deserialize, Serialize};

/// Boot source configuration for a microVM.
#[derive(Debug, Serialize, Deserialize)]
pub struct BootSource {
    pub kernel_image_path: String,
    pub boot_args: String,
}

/// Block device (drive) configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct Drive {
    pub drive_id: String,
    pub path_on_host: String,
    pub is_root_device: bool,
    pub is_read_only: bool,
}

/// Network interface configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub iface_id: String,
    pub guest_mac: String,
    pub host_dev_name: String,
}

/// Machine configuration (vCPUs, memory).
#[derive(Debug, Serialize, Deserialize)]
pub struct MachineConfig {
    pub vcpu_count: u32,
    pub mem_size_mib: u32,
}

/// Action request (e.g., start the VM).
#[derive(Debug, Serialize, Deserialize)]
pub struct InstanceAction {
    pub action_type: String,
}

/// Logger configuration.
#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Logger {
    pub log_path: String,
    pub level: String,
    pub show_level: bool,
    pub show_log_origin: bool,
}

/// Full VM configuration for config-file based launching.
#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct VmConfig {
    #[serde(rename = "boot-source")]
    pub boot_source: BootSource,
    pub drives: Vec<Drive>,
    #[serde(rename = "network-interfaces")]
    pub network_interfaces: Vec<NetworkInterface>,
    #[serde(rename = "machine-config")]
    pub machine_config: MachineConfig,
}

/// Instance info returned by GET /.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct InstanceInfo {
    pub id: String,
    pub state: String,
    pub vmm_version: String,
}

/// Error body from Firecracker API.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ApiError {
    pub fault_message: String,
}
