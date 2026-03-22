# OpenVPS

AI-agent VPS hosting. Pay with stablecoins. SSH in seconds.

Firecracker microVMs provisioned via HTTP 402. Agents pay with USDC on Base, Celo, or Tempo — get root SSH access to Ubuntu 24.04 VMs in under 3 seconds. No accounts. No API keys. Just pay and compute.

**Live at [openvps.sh](https://openvps.sh)**

## Quick Start

Point your AI agent here:

```bash
curl https://openvps.sh/skill.md
```

Your agent reads the skill, pays, and SSHs in. That's it.

## How It Works

```
1. POST /v1/provision        → 402 Payment Required (with price)
2. Pay stablecoins on-chain  → Base, Celo, or Tempo
3. Resubmit with proof       → 201 Created + SSH key + public IP
4. ssh -p PORT root@HOST     → Root access to Ubuntu 24.04
```

## Supported Payment Methods

| Protocol | Network | Token | Chain ID |
|----------|---------|-------|----------|
| **x402** | Base | USDC | 8453 |
| **x402** | Celo | USDC | 42220 |
| **MPP** | Tempo | USDC.e | 4217 |
| **MPP** | Tempo | pathUSD | 4217 |

Wallets: Foundry keystore, Open Wallet Standard, Coinbase Agentic Wallet, Bankr, or any EVM signer.

## Pricing

| Resource | Per hour |
|----------|---------|
| 1 vCPU | $0.005 |
| 1 GB RAM | $0.002 |
| 1 GB Disk | $0.0001 |

Example: 2 vCPUs + 1GB RAM + 10GB disk for 1 hour = **$0.013**

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│  AI Agent   │────▶│  Rust API    │────▶│  Firecracker    │
│  (client)   │◀────│  (axum)      │◀────│  microVM        │
└─────────────┘     └──────────────┘     └─────────────────┘
       │                    │                      │
       │ Pay USDC      Verify on-chain       SSH access
       │ (x402/MPP)    (Tempo/Base/Celo)     (public IP)
       ▼                    ▼                      ▼
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│  Blockchain │     │  SQLite DB   │     │  Ubuntu 24.04   │
│  (payment)  │     │  (state)     │     │  (rootfs)       │
└─────────────┘     └──────────────┘     └─────────────────┘
```

- **API** — Rust (axum), dual-protocol payment gate (x402 + MPP)
- **VMs** — Firecracker microVMs, KVM-isolated, sub-second boot
- **Networking** — TAP devices, bridge + NAT, per-VM port forwarding
- **SSH** — Ed25519 keypair generated per VM, injected into rootfs
- **DB** — SQLite (VMs, IP allocations, challenge tracking)

## Self-Hosting Guide

### Requirements

- Linux server with KVM support (`/dev/kvm`)
- 4+ CPU cores, 8+ GB RAM
- Public IP address
- Rust toolchain

### 1. Install Dependencies

```bash
# Firecracker
ARCH=$(uname -m)
curl -fsSL "https://github.com/firecracker-microvm/firecracker/releases/download/v1.11.0/firecracker-v1.11.0-${ARCH}.tgz" | tar -xz -C /tmp
sudo mv /tmp/release-*/firecracker-* /usr/local/bin/firecracker
sudo mv /tmp/release-*/jailer-* /usr/local/bin/jailer
sudo chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer

# Build tools
sudo apt-get install -y build-essential pkg-config libssl-dev sqlite3 \
  iproute2 iptables bridge-utils dnsmasq debootstrap
```

### 2. Clone and Build

```bash
git clone https://github.com/kartojal/openvps.git
cd openvps
cargo build --release
```

### 3. Set Up Networking

```bash
# Create bridge
sudo ip link add mpp-br0 type bridge
sudo ip addr add 172.16.0.1/16 dev mpp-br0
sudo ip link set mpp-br0 up

# Enable NAT
sudo sysctl -w net.ipv4.ip_forward=1
HOST_IFACE=$(ip route | grep default | awk '{print $5}')
sudo iptables -t nat -A POSTROUTING -s 172.16.0.0/16 -o $HOST_IFACE -j MASQUERADE
sudo iptables -A FORWARD -i mpp-br0 -o $HOST_IFACE -j ACCEPT
sudo iptables -A FORWARD -i $HOST_IFACE -o mpp-br0 -m state --state RELATED,ESTABLISHED -j ACCEPT

# VM isolation
sudo iptables -A FORWARD -i mpp-br0 -o mpp-br0 -j DROP

# DNS forwarder for VMs
echo -e "interface=mpp-br0\nbind-interfaces\nno-dhcp-interface=mpp-br0\nserver=8.8.8.8" | \
  sudo tee /etc/dnsmasq.d/mpp.conf
sudo systemctl restart dnsmasq
```

### 4. Prepare VM Assets

```bash
sudo mkdir -p /var/lib/mpp-hosting/assets /var/lib/mpp-hosting/vms

# Download kernel
curl -fsSL "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.11/$(uname -m)/vmlinux-6.1.102" \
  -o /var/lib/mpp-hosting/assets/vmlinux

# Build Ubuntu 24.04 rootfs (or use the included script)
sudo bash infra/download-assets.sh
```

### 5. Configure

```bash
cp api/.env.testnet .env
# Edit .env with your values:
#   PAYMENT_RECIPIENT=0xYourWallet
#   MPP_SECRET_KEY=$(openssl rand -hex 32)
#   PUBLIC_IP=your.server.ip
#   HOST_IFACE=eth0
```

### 6. Run

```bash
source .env
sudo ./target/release/mpp-hosting-api
```

The API starts on port 8402. Put nginx/caddy in front for TLS.

## API Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/health` | None | Health check |
| `GET` | `/status` | None | Live capacity (slots, vCPUs, RAM) |
| `POST` | `/v1/provision` | x402 / MPP | Provision a VM |
| `GET` | `/v1/vms/{id}` | None | VM status |
| `DELETE` | `/v1/vms/{id}` | None | Terminate VM |
| `GET` | `/.well-known/x402` | None | x402 discovery |
| `GET` | `/openapi.json` | None | OpenAPI spec |

## Discovery

- **x402scan**: [x402scan.com/server/a7ad4651-cc49-4448-bbb7-b7cf18c29893](https://www.x402scan.com/server/a7ad4651-cc49-4448-bbb7-b7cf18c29893)
- **OpenAPI**: [openvps.sh/openapi.json](https://openvps.sh/openapi.json)
- **x402 well-known**: [openvps.sh/.well-known/x402](https://openvps.sh/.well-known/x402)
- **LLM context**: [openvps.sh/llms.txt](https://openvps.sh/llms.txt)
- **Agent skill**: [openvps.sh/skill.md](https://openvps.sh/skill.md)

## License

MIT
