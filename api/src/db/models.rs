use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmStatus {
    Pending,
    Running,
    Terminated,
    Failed,
}

impl VmStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "terminated" => Self::Terminated,
            "failed" => Self::Failed,
            _ => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmRecord {
    pub id: Uuid,
    pub status: VmStatus,
    pub vcpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub image: String,
    pub ip_addr: Option<String>,
    pub ssh_port: Option<u16>,
    pub tap_device: Option<String>,
    pub socket_path: Option<String>,
    pub pid: Option<u32>,
    pub payment_tx: Option<String>,
    pub price_micro: u64,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub terminated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub vm_id: Option<String>,
    /// pending | running | completed | failed | timeout
    pub status: String,
    pub command: String,
    pub setup_script: Option<String>,
    pub output: String,
    pub exit_code: Option<i32>,
    pub timeout_secs: u32,
    pub vcpus: u32,
    pub ram_mb: u32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub payment_tx: Option<String>,
    pub price_micro: u64,
}
