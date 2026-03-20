# MPP Hosting

AI-agent-payable VPS hosting powered by Firecracker microVMs and the Machine Payments Protocol (MPP).

## Architecture

- **api/** — Rust (axum) API server
  - `firecracker/` — Firecracker microVM client (Unix socket REST API)
  - `mpp/` — MPP protocol implementation (HTTP 402 challenge-credential-receipt)
  - `network/` — IP pool allocation, TAP device management
  - `vm/` — VM lifecycle manager (provision, terminate, auto-expire)
  - `db/` — SQLite state (VMs, IP allocations)
  - `routes/` — HTTP endpoints
- **infra/** — Host provisioning scripts and Terraform

## Build & Test

```bash
cargo check    # type check
cargo test     # run tests
cargo build --release  # production build
```

## Key Design Decisions

- Firecracker over Proxmox/Docker for sub-second VM provisioning (~125ms boot)
- Direct REST API via curl to Firecracker socket (no heavy SDK dependency)
- SQLite for simplicity (single-node deployment)
- MPP payment verification: dev mode accepts any tx_hash, production verifies on Tempo chain
- TAP devices + bridge + NAT for VM networking
- Background task terminates expired VMs every 30s

## API Flow

1. `POST /v1/provision` without payment → `402` with `WWW-Authenticate: Payment` challenge
2. Agent pays on Tempo chain
3. `POST /v1/provision` with `Authorization: Payment` credential → `201` with VM details
4. `GET /v1/vms/:id` — check VM status
5. `DELETE /v1/vms/:id` — terminate early
