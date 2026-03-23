# Skill: OpenVPS — Buy and use cloud servers with crypto

You are an AI agent that needs compute. OpenVPS lets you provision Ubuntu 24.04 servers, pay with USDC, and get SSH access in seconds. No accounts, no API keys.

## What you can do

| Capability | Endpoint | Description |
|-----------|----------|-------------|
| **Provision a VPS** | `POST /v1/provision` | Get a server with SSH key access |
| **Wallet-auth VPS** | `POST /v2/provision` | Get a server with wallet-signed SSH (no keys) |
| **Run a job** | `POST /v1/jobs` | Run any command on a fresh VM, get output back |
| **Check VM status** | `GET /v1/vms/{id}` | See if a VM is still running |
| **Terminate VM** | `DELETE /v1/vms/{id}` | Shut down a VM early |
| **Poll job results** | `GET /v1/jobs/{id}` | Get job output and exit code |

## Payment Networks

| Protocol | Network | Token | Contract | Chain ID |
|----------|---------|-------|----------|----------|
| **x402** | Base | USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | 8453 |
| **x402** | Celo | USDC | `0xcebA9300f2b948710d2653dD7B07f33A8B32118C` | 42220 |
| **MPP** | Tempo | USDC.e | `0x20c000000000000000000000b9537d11c60e8b50` | 4217 |
| **MPP** | Tempo | pathUSD | `0x20c0000000000000000000000000000000000000` | 4217 |

All endpoints accept both x402 and MPP simultaneously. The 402 response includes both `PAYMENT-REQUIRED` (x402) and `WWW-Authenticate` (MPP) headers.

## Pricing

| Resource | Per hour |
|----------|---------|
| 1 vCPU | $0.005 |
| 1 GB RAM | $0.002 |
| 1 GB Disk | $0.0001 |

Example: 2 vCPUs + 1GB RAM + 10GB disk for 1 hour = **$0.013**

---

## Option A: Provision a VPS (SSH key access)

### Step 1: Request

```bash
curl -s -D /tmp/headers.txt -o /tmp/challenge.json \
  https://openvps.sh/v1/provision \
  -H "Content-Type: application/json" \
  -d '{"vcpus": 2, "ram_mb": 1024, "disk_gb": 10, "duration": 3600}'
```

Response: `402 Payment Required` with pricing in the body and both payment headers.

### Step 2: Pay (MPP / Tempo)

```bash
# Extract challenge
CHALLENGE_ID=$(cat /tmp/challenge.json | jq -r .id)
CHALLENGE_B64=$(cat /tmp/challenge.json | base64 | tr -d "\n")

# Transfer USDC.e on Tempo
TX_HASH=$(cast send \
  --keystore wallet.json --password "pw" \
  --rpc-url https://rpc.tempo.xyz --chain 4217 \
  0x20c000000000000000000000b9537d11c60e8b50 \
  "transfer(address,uint256)(bool)" RECIPIENT AMOUNT \
  --json | jq -r .transactionHash)

# Build credential
CREDENTIAL=$(echo -n "{\"challenge_id\":\"$CHALLENGE_ID\",\"tx_hash\":\"$TX_HASH\",\"network\":\"tempo\",\"payer\":\"$MY_ADDRESS\",\"signature\":\"\"}" | base64 | tr -d "\n")

# Submit
curl -s https://openvps.sh/v1/provision \
  -H "Content-Type: application/json" \
  -H "Authorization: Payment $CREDENTIAL" \
  -H "X-MPP-Challenge: $CHALLENGE_B64" \
  -d '{"vcpus": 2, "ram_mb": 1024, "disk_gb": 10, "duration": 3600}'
```

### Step 3: Connect

Response (201 Created):
```json
{
  "vm_id": "uuid",
  "ssh_host": "95.216.14.126",
  "ssh_port": 2201,
  "ssh_command": "ssh -p 2201 -i vm_key root@95.216.14.126",
  "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n...",
  "expires_at": "2026-03-23T12:00:00+00:00",
  "status": "running"
}
```

```bash
echo "$SSH_PRIVATE_KEY" > /tmp/vm_key && chmod 600 /tmp/vm_key
ssh -p 2201 -i /tmp/vm_key root@95.216.14.126
```

### Step 4: Terminate

```bash
curl -s -X DELETE https://openvps.sh/v1/vms/VM_ID
```

---

## Option B: Run a Job (command execution, auto-cleanup)

Submit a command, get results back. VM auto-terminates when done. Max 60 minutes.

### Request

```bash
curl -s https://openvps.sh/v1/jobs \
  -H "Content-Type: application/json" \
  -d '{
    "command": "echo hello && uname -a",
    "setup": "apt-get update && apt-get install -y python3",
    "files": {
      "/root/script.py": "print(\"hello from python\")",
      "/root/data.json": "{\"key\": \"value\"}"
    },
    "vcpus": 2,
    "ram_mb": 2048,
    "timeout": 300
  }'
```

After payment, returns:
```json
{
  "job_id": "uuid",
  "status": "running",
  "ssh_host": "95.216.14.126",
  "ssh_port": 2201,
  "poll_url": "/v1/jobs/uuid",
  "timeout": 300
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| command | yes | — | Shell command to run |
| setup | no | — | Setup script (runs first) |
| files | no | — | File path → content map (uploaded before running) |
| vcpus | no | 1 | CPUs (1-4) |
| ram_mb | no | 512 | RAM in MB (256-4096) |
| timeout | no | 300 | Max seconds (60-3600) |

### Poll results

```bash
curl -s https://openvps.sh/v1/jobs/JOB_ID
```

```json
{
  "status": "completed",
  "exit_code": 0,
  "output": "hello\nLinux mpp-vm 6.1.102 x86_64...",
  "duration_secs": 5
}
```

Status values: `pending`, `running`, `completed`, `failed`, `timeout`

### Use with rsync

The `ssh_host` and `ssh_port` in the job response allow manual file upload:

```bash
rsync -avz -e "ssh -p $SSH_PORT -i vm_key" ./project/ root@$SSH_HOST:/root/project/
```

---

## Option C: Wallet-Auth VPS (v2 — no SSH keys)

VPS access authenticated by your crypto wallet instead of SSH keys.

### Step 1: Provision (same payment as v1)

```bash
curl -s https://openvps.sh/v2/provision \
  -H "Content-Type: application/json" \
  -d '{"vcpus": 2, "ram_mb": 1024, "duration": 3600}'
# → 402 → pay → 201
```

Response includes a `challenge` to sign (no SSH key):
```json
{
  "vm_id": "uuid",
  "ssh_host": "...",
  "ssh_port": 2201,
  "auth": {
    "type": "wallet",
    "challenge": "openvps:VM_ID:NONCE",
    "message": "Sign this challenge, then POST to /v2/session"
  }
}
```

### Step 2: Get SSH token

```bash
curl -s https://openvps.sh/v2/session \
  -H "Content-Type: application/json" \
  -d '{"vm_id": "...", "signature": "0x...", "address": "0xYourWallet"}'
```

Returns a one-time SSH token:
```json
{
  "token": "abc123...",
  "ssh_command": "sshpass -p 'abc123...' ssh root@HOST -p PORT"
}
```

### Step 3: SSH with token

```bash
sshpass -p "$TOKEN" ssh -p 2201 root@95.216.14.126
```

Token is single-use. Request a new one via `/v2/session` if your session closes.

---

## Wallet Options

| Wallet | How to sign |
|--------|-------------|
| Foundry keystore | `cast send --keystore key.json ...` |
| Raw private key | `cast send --private-key $KEY ...` |
| Open Wallet Standard | `ows sign tx --wallet agent-treasury --chain evm ...` |
| Coinbase Agentic Wallet | CDP SDK / x402 client |
| Any EVM library | viem, ethers.js, web3.py, alloy |

## Quick Reference

| Item | Value |
|------|-------|
| API | `https://openvps.sh` |
| Payments | x402 (Base, Celo) + MPP (Tempo) |
| Tempo RPC | `https://rpc.tempo.xyz` |
| Tempo Chain ID | 4217 |
| Base Chain ID | 8453 |
| Celo Chain ID | 42220 |
| SSH User | `root` |
| VM OS | Ubuntu 24.04 LTS (x86_64) |
| OpenAPI | `https://openvps.sh/openapi.json` |
| x402 discovery | `https://openvps.sh/.well-known/x402` |
| Source | `https://github.com/kartojal/openvps` |
