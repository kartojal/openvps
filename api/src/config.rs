use std::net::SocketAddr;

/// Application configuration, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the API server listens on
    pub listen_addr: SocketAddr,

    /// Path to the Firecracker binary
    pub firecracker_bin: String,

    /// Directory to store VM sockets and state
    pub vm_state_dir: String,

    /// Path to the guest kernel image
    pub kernel_path: String,

    /// Path to the base rootfs image (will be copied per-VM)
    pub rootfs_path: String,

    /// Subnet for VM networking (e.g., "172.16.0.0/16")
    pub vm_subnet: String,

    /// Host bridge interface name
    pub bridge_iface: String,

    /// SQLite database path
    pub db_path: String,

    /// MPP secret key for HMAC signing of challenges/receipts
    pub mpp_secret_key: String,

    /// Tempo RPC endpoint for payment verification
    pub tempo_rpc_url: String,

    /// Recipient address for payments (our wallet)
    pub payment_recipient: String,

    /// USDC/pathUSD contract address on Tempo
    pub usdc_contract: String,

    /// Tempo chain ID (42431 for testnet)
    pub chain_id: u64,

    /// Price per vCPU per hour in microdollars (e.g., 5000 = $0.005)
    pub price_vcpu_hour_micro: u64,

    /// Price per MB RAM per hour in microdollars
    pub price_ram_mb_hour_micro: u64,

    /// Price per GB disk per hour in microdollars
    pub price_disk_gb_hour_micro: u64,

    /// x402 facilitator URL for payment verification and settlement
    pub x402_facilitator_url: String,

    /// USDC contract address on Base Sepolia (ERC-20, 6 decimals)
    pub x402_base_asset: String,

    /// cUSD contract address on Celo mainnet (ERC-20, 18 decimals — but amount strings are in atomic units)
    pub x402_celo_asset: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            listen_addr: env_or("LISTEN_ADDR", "0.0.0.0:8402")
                .parse()?,
            firecracker_bin: env_or("FIRECRACKER_BIN", "/usr/local/bin/firecracker"),
            vm_state_dir: env_or("VM_STATE_DIR", "/var/lib/mpp-hosting/vms"),
            kernel_path: env_or("KERNEL_PATH", "/var/lib/mpp-hosting/assets/vmlinux"),
            rootfs_path: env_or("ROOTFS_PATH", "/var/lib/mpp-hosting/assets/rootfs.ext4"),
            vm_subnet: env_or("VM_SUBNET", "172.16.0.0/16"),
            bridge_iface: env_or("BRIDGE_IFACE", "mpp-br0"),
            db_path: env_or("DB_PATH", "/var/lib/mpp-hosting/mpp-hosting.db"),
            mpp_secret_key: env_or("MPP_SECRET_KEY", "dev-secret-change-me"),
            tempo_rpc_url: env_or("TEMPO_RPC_URL", "https://rpc.moderato.tempo.xyz"),
            payment_recipient: env_or(
                "PAYMENT_RECIPIENT",
                "0x0000000000000000000000000000000000000000",
            ),
            usdc_contract: env_or(
                "USDC_CONTRACT",
                // pathUSD on Tempo (testnet & mainnet)
                "0x20c0000000000000000000000000000000000000",
            ),
            chain_id: env_or("CHAIN_ID", "42431").parse()?,
            price_vcpu_hour_micro: env_or("PRICE_VCPU_HOUR_MICRO", "5000")
                .parse()?,
            price_ram_mb_hour_micro: env_or("PRICE_RAM_MB_HOUR_MICRO", "2")
                .parse()?,
            price_disk_gb_hour_micro: env_or("PRICE_DISK_GB_HOUR_MICRO", "100")
                .parse()?,
            x402_facilitator_url: env_or(
                "X402_FACILITATOR_URL",
                "https://x402.org/facilitator",
            ),
            x402_base_asset: env_or(
                "X402_BASE_ASSET",
                // USDC on Base Sepolia
                "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            ),
            x402_celo_asset: env_or(
                "X402_CELO_ASSET",
                // cUSD on Celo mainnet
                "0x765DE816845861e75A25fCA122bb6898B8B1282a",
            ),
        })
    }

    /// Calculate price in microdollars for a given resource allocation and duration.
    pub fn calculate_price_micro(&self, vcpus: u32, ram_mb: u32, disk_gb: u32, duration_secs: u64) -> u64 {
        let hours = duration_secs as f64 / 3600.0;
        let cpu_cost = self.price_vcpu_hour_micro as f64 * vcpus as f64 * hours;
        let ram_cost = self.price_ram_mb_hour_micro as f64 * ram_mb as f64 * hours;
        let disk_cost = self.price_disk_gb_hour_micro as f64 * disk_gb as f64 * hours;
        (cpu_cost + ram_cost + disk_cost).ceil() as u64
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
