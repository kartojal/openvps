#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# MPP Hosting — Host Setup Script
#
# Installs Firecracker, configures KVM, and prepares the host for running
# microVMs. Designed for Ubuntu 22.04+ / Debian 12+ on bare metal.
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="/usr/local/bin"
STATE_DIR="/var/lib/mpp-hosting"
ASSETS_DIR="${STATE_DIR}/assets"
VM_DIR="${STATE_DIR}/vms"

ARCH="$(uname -m)"
FC_VERSION="${FC_VERSION:-v1.11.0}"

echo "=== MPP Hosting: Host Setup ==="
echo "Architecture: ${ARCH}"
echo "Firecracker version: ${FC_VERSION}"
echo ""

# --- Check prerequisites ---
if [[ "$(uname)" != "Linux" ]]; then
    echo "ERROR: Firecracker requires Linux with KVM support."
    echo "This script is for bare-metal Linux hosts (not macOS)."
    exit 1
fi

if ! lsmod | grep -q kvm; then
    echo "ERROR: KVM module not loaded. Enable hardware virtualization in BIOS."
    exit 1
fi

# --- Install dependencies ---
echo "--- Installing system dependencies ---"
apt-get update -qq
apt-get install -y -qq curl jq iptables iproute2 bridge-utils

# --- Install Firecracker ---
echo "--- Installing Firecracker ${FC_VERSION} ---"
RELEASE_URL="https://github.com/firecracker-microvm/firecracker/releases"

if [[ -f "${INSTALL_DIR}/firecracker" ]]; then
    CURRENT=$("${INSTALL_DIR}/firecracker" --version 2>/dev/null | head -1 || echo "unknown")
    echo "Firecracker already installed: ${CURRENT}"
else
    echo "Downloading Firecracker ${FC_VERSION} for ${ARCH}..."
    curl -fsSL "${RELEASE_URL}/download/${FC_VERSION}/firecracker-${FC_VERSION}-${ARCH}.tgz" \
        | tar -xz -C /tmp

    mv "/tmp/release-${FC_VERSION}-${ARCH}/firecracker-${FC_VERSION}-${ARCH}" \
        "${INSTALL_DIR}/firecracker"
    mv "/tmp/release-${FC_VERSION}-${ARCH}/jailer-${FC_VERSION}-${ARCH}" \
        "${INSTALL_DIR}/jailer"
    chmod +x "${INSTALL_DIR}/firecracker" "${INSTALL_DIR}/jailer"
    rm -rf "/tmp/release-${FC_VERSION}-${ARCH}"

    echo "Firecracker installed: $(firecracker --version)"
fi

# --- Configure KVM access ---
echo "--- Configuring KVM access ---"
if [[ -e /dev/kvm ]]; then
    chmod 666 /dev/kvm
    echo "KVM access configured"
else
    echo "WARNING: /dev/kvm not found — VMs will not work"
fi

# --- Create state directories ---
echo "--- Creating state directories ---"
mkdir -p "${ASSETS_DIR}" "${VM_DIR}"

# --- Download kernel and rootfs ---
echo "--- Downloading kernel and rootfs ---"
"${SCRIPT_DIR}/download-assets.sh"

# --- Setup networking ---
echo "--- Configuring network ---"
"${SCRIPT_DIR}/setup-network.sh"

# --- Create systemd service ---
echo "--- Installing systemd service ---"
cat > /etc/systemd/system/mpp-hosting.service << 'UNIT'
[Unit]
Description=MPP Hosting API
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/mpp-hosting-api
Restart=always
RestartSec=5
Environment=RUST_LOG=mpp_hosting_api=info
EnvironmentFile=-/etc/mpp-hosting/env
WorkingDirectory=/var/lib/mpp-hosting

# Security hardening
NoNewPrivileges=false
ProtectSystem=strict
ReadWritePaths=/var/lib/mpp-hosting

[Install]
WantedBy=multi-user.target
UNIT

mkdir -p /etc/mpp-hosting
if [[ ! -f /etc/mpp-hosting/env ]]; then
    cat > /etc/mpp-hosting/env << 'ENV'
LISTEN_ADDR=0.0.0.0:8402
VM_STATE_DIR=/var/lib/mpp-hosting/vms
KERNEL_PATH=/var/lib/mpp-hosting/assets/vmlinux
ROOTFS_PATH=/var/lib/mpp-hosting/assets/rootfs.ext4
VM_SUBNET=172.16.0.0/16
BRIDGE_IFACE=mpp-br0
DB_PATH=/var/lib/mpp-hosting/mpp-hosting.db
TEMPO_RPC_URL=https://rpc.tempo.xyz
# IMPORTANT: Change this in production!
MPP_SECRET_KEY=dev-secret-change-me
PAYMENT_RECIPIENT=0x0000000000000000000000000000000000000000
USDC_CONTRACT=0x20c000000000000000000000b9537d11c60e8b50
ENV
    echo "Created /etc/mpp-hosting/env — EDIT THIS with your production values!"
fi

systemctl daemon-reload
echo "Systemd service installed (not started). Run: systemctl start mpp-hosting"

echo ""
echo "=== Host setup complete ==="
echo ""
echo "Next steps:"
echo "  1. Edit /etc/mpp-hosting/env with your wallet address and MPP secret"
echo "  2. Build and install the API: cargo build --release && cp target/release/mpp-hosting-api /usr/local/bin/"
echo "  3. Start the service: systemctl enable --now mpp-hosting"
