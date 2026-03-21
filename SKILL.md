# Skill: Provision a VPS via MPP Hosting

You are an AI agent that needs compute. This skill teaches you how to request, pay for, and connect to a VPS (Virtual Private Server) powered by Firecracker microVMs, paid via stablecoin micropayments.

## Supported Payment Protocols

| Protocol | Networks | Description |
|----------|----------|-------------|
| **x402** | Base (USDC), Celo (cUSD) | Coinbase's HTTP 402 standard — sign an EIP-3009 authorization, facilitator settles on-chain |
| **MPP** | Tempo (pathUSD) | Machine Payments Protocol — transfer stablecoins directly, provide tx hash |

The server accepts **all three** simultaneously. Pick whichever network/protocol your wallet supports.

## Overview

1. Request a VPS → get a `402 Payment Required` with both `PAYMENT-REQUIRED` (x402) and `WWW-Authenticate` (MPP) headers
2. Pay via x402 (Base/Celo) or MPP (Tempo)
3. Resubmit with payment proof → get VM IP + SSH private key
4. SSH into your VM and do your work
5. Terminate when done

## Step 1: Request a VPS (Get Payment Challenge)

```bash
curl -s -D /tmp/mpp-headers.txt -o /tmp/mpp-challenge.json \
  https://YOUR_MPP_HOST:8402/v1/provision \
  -H "Content-Type: application/json" \
  -d '{
    "vcpus": 2,
    "ram_mb": 1024,
    "disk_gb": 10,
    "image": "ubuntu-24.04",
    "duration": 3600
  }'
```

You'll get a `402 Payment Required` response with **two headers**:

### Header 1: `PAYMENT-REQUIRED` (x402 protocol — Base, Celo)

Decode the base64 header to get:

```json
{
  "x402Version": 1,
  "resource": {
    "url": "/v1/provision",
    "description": "Provision a Firecracker microVM",
    "mimeType": "application/json"
  },
  "accepts": [
    {
      "scheme": "exact",
      "network": "eip155:84532",
      "amount": "12548",
      "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      "payTo": "0x8A739f3A6f40194C0128904bC387e63d9C0577A4",
      "maxTimeoutSeconds": 300,
      "extra": {"name": "USDC", "version": "2"}
    },
    {
      "scheme": "exact",
      "network": "eip155:42220",
      "amount": "12548",
      "asset": "0x765DE816845861e75A25fCA122bb6898B8B1282a",
      "payTo": "0x8A739f3A6f40194C0128904bC387e63d9C0577A4",
      "maxTimeoutSeconds": 300,
      "extra": {"name": "cUSD"}
    }
  ]
}
```

### Header 2: `WWW-Authenticate` (MPP protocol — Tempo)

The response body contains the MPP challenge:

```json
{
  "id": "uuid-challenge-id",
  "amount": "12548",
  "currency": "USD",
  "recipient": "0x8A739f3A6f40194C0128904bC387e63d9C0577A4",
  "network": "tempo",
  "chain_id": 4217,
  "rpc_url": "https://rpc.tempo.xyz",
  "token_contract": "0x20c0000000000000000000000000000000000000"
}
```

**Choose your payment path:**
- **x402 (Base/Celo)** → Go to Step 2A
- **MPP (Tempo)** → Go to Step 2B

---

## Step 2A: Pay via x402 (Base USDC or Celo cUSD)

The x402 protocol uses EIP-3009 `transferWithAuthorization`. You sign an authorization off-chain, and the facilitator settles it on-chain.

### Using Open Wallet Standard (OWS)

If your agent uses OWS, it handles signing automatically:

```bash
# Sign the x402 authorization via OWS
ows sign tx --wallet agent-treasury --chain evm --tx-hex "$AUTHORIZATION_DATA"
```

Or via MCP (Claude/OpenAI agents):

```
Agent → ows_sign(chain="evm", tx=authorization_data) → signed payload
```

### Using any x402 client library

The x402 client libraries handle the full flow automatically:

**Rust** (`x402-reqwest`):
```rust
use x402_reqwest::X402Client;
let client = X402Client::new(signer, "eip155:84532");
let response = client.post("https://host:8402/v1/provision").json(&body).send().await?;
// Payment is handled transparently
```

**JavaScript** (`@anthropic-ai/x402`):
```javascript
import { x402fetch } from "@anthropic-ai/x402";
const response = await x402fetch("https://host:8402/v1/provision", {
  method: "POST",
  body: JSON.stringify(body),
  paymentSigner: wallet,
});
```

### Manual x402 flow

If you need to construct the payment manually:

1. Pick a network from `accepts[]` (e.g., `eip155:84532` for Base Sepolia USDC)
2. Sign an EIP-3009 `transferWithAuthorization` for the `amount` to `payTo`
3. Build a `PaymentPayload` JSON, base64-encode it
4. Resend the request with `PAYMENT-SIGNATURE: <base64>` header

The server calls the facilitator to verify and settle on-chain. On success, you get `201 Created` with VM details + the `PAYMENT-RESPONSE` header.

---

## Step 2B: Pay via MPP (Tempo pathUSD)

Transfer the exact `amount` of `token_contract` to `recipient` on Tempo. The amount is in the token's smallest unit (6 decimals for TIP-20 USD stablecoins).

Save the challenge info:

```bash
CHALLENGE_ID=$(cat /tmp/mpp-challenge.json | jq -r .id)
CHALLENGE_B64=$(grep -i "www-authenticate" /tmp/mpp-headers.txt | sed 's/.*challenge="//' | sed 's/".*//')
```

Choose the method that matches your wallet setup:

---

### Option A: Foundry Keystore (cast)

Best for: CLI-native agents, dev/testing environments.

```bash
# Transfer pathUSD to the recipient
TX_HASH=$(cast send \
  --keystore /path/to/keystore.json \
  --password "your-password" \
  --rpc-url https://rpc.tempo.xyz \
  --chain 4217 \
  0x20c0000000000000000000000000000000000000 \
  "transfer(address,uint256)(bool)" \
  RECIPIENT_ADDRESS \
  AMOUNT \
  --json | jq -r .transactionHash)

echo "TX: $TX_HASH"
```

To create a keystore if you don't have one:

```bash
cast wallet new /path/to/keystores --unsafe-password "your-password"
# Fund it from faucet (testnet only):
cast rpc tempo_fundAddress YOUR_ADDRESS --rpc-url https://rpc.tempo.xyz
```

---

### Option B: Raw Private Key (cast)

Best for: Agents with a private key in memory or environment variable.

```bash
TX_HASH=$(cast send \
  --private-key $PRIVATE_KEY \
  --rpc-url https://rpc.tempo.xyz \
  --chain 4217 \
  0x20c0000000000000000000000000000000000000 \
  "transfer(address,uint256)(bool)" \
  RECIPIENT_ADDRESS \
  AMOUNT \
  --json | jq -r .transactionHash)
```

---

### Option C: Bankr Bot API

Best for: Agents on Farcaster/X that use Bankr for wallet management.

```bash
# Use Bankr's agent API to execute the transfer
# Bankr handles wallet creation, key management, and tx signing
curl -s https://api.bankr.bot/v1/transfer \
  -H "Authorization: Bearer $BANKR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "chain": "tempo",
    "token": "0x20c0000000000000000000000000000000000000",
    "to": "RECIPIENT_ADDRESS",
    "amount": "AMOUNT"
  }'
```

---

### Option D: Coinbase Agentic Wallet (x402 / CDP SDK)

Best for: Agents built with Coinbase Developer Platform.

```python
from coinbase_agentkit import AgentWallet

wallet = AgentWallet.from_env()  # uses CDP_API_KEY
tx = wallet.transfer(
    chain="tempo",
    token="0x20c0000000000000000000000000000000000000",
    to="RECIPIENT_ADDRESS",
    amount="AMOUNT",
)
tx_hash = tx.hash
```

---

### Option E: Direct RPC (any language)

Best for: Agents that construct and sign transactions programmatically.

```bash
# 1. Encode the ERC-20 transfer calldata
CALLDATA=$(cast calldata "transfer(address,uint256)" RECIPIENT_ADDRESS AMOUNT)

# 2. Get nonce
NONCE=$(cast nonce YOUR_ADDRESS --rpc-url https://rpc.tempo.xyz)

# 3. Sign and send (using cast or any eth library)
TX_HASH=$(cast send \
  --private-key $KEY \
  --rpc-url https://rpc.tempo.xyz \
  --chain 4217 \
  --nonce $NONCE \
  0x20c0000000000000000000000000000000000000 \
  $CALLDATA \
  --json | jq -r .transactionHash)
```

For other languages, use any EVM-compatible library:
- **Python**: `web3.py` or `eth_account`
- **JavaScript**: `viem` or `ethers.js`
- **Rust**: `alloy` or `ethers-rs`
- **Go**: `go-ethereum`

---

## Step 3: Submit Payment Proof (MPP only)

If you used x402 (Step 2A), the payment is already submitted via the `PAYMENT-SIGNATURE` header — skip to Step 4.

If you used MPP (Step 2B), once you have the `TX_HASH`, build a credential and resubmit:

```bash
# Build the payment credential
CREDENTIAL=$(echo -n "{
  \"challenge_id\": \"$CHALLENGE_ID\",
  \"tx_hash\": \"$TX_HASH\",
  \"network\": \"tempo\",
  \"payer\": \"YOUR_WALLET_ADDRESS\",
  \"signature\": \"\"
}" | base64 | tr -d "\n")

# Submit provision request with payment
curl -s -o /tmp/mpp-vm.json \
  https://YOUR_MPP_HOST:8402/v1/provision \
  -H "Content-Type: application/json" \
  -H "Authorization: Payment $CREDENTIAL" \
  -H "X-MPP-Challenge: $CHALLENGE_B64" \
  -d '{
    "vcpus": 2,
    "ram_mb": 1024,
    "disk_gb": 10,
    "image": "ubuntu-24.04",
    "duration": 3600
  }'

cat /tmp/mpp-vm.json | jq .
```

Response (201 Created):

```json
{
  "vm_id": "d8e8651f-1089-466e-9db4-14fd93bc3d10",
  "ip": "172.16.0.2",
  "ssh_port": 22,
  "expires_at": "2026-03-21T10:00:00+00:00",
  "status": "running",
  "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n..."
}
```

## Step 4: Connect via SSH

```bash
# Save the private key
cat /tmp/mpp-vm.json | jq -r .ssh_private_key > /tmp/vm_key
chmod 600 /tmp/vm_key

VM_IP=$(cat /tmp/mpp-vm.json | jq -r .ip)

# Connect
ssh -i /tmp/vm_key -o StrictHostKeyChecking=no root@$VM_IP
```

You now have root access to an Ubuntu 24.04 VM with the resources you paid for.

## Step 5: Check Status or Terminate

```bash
VM_ID=$(cat /tmp/mpp-vm.json | jq -r .vm_id)

# Check status
curl -s https://YOUR_MPP_HOST:8402/v1/vms/$VM_ID | jq .

# Terminate early (stops billing)
curl -s -X DELETE https://YOUR_MPP_HOST:8402/v1/vms/$VM_ID | jq .
```

## Pricing

Prices are in microdollars (1 microdollar = $0.000001):

| Resource | Cost per hour |
|----------|--------------|
| 1 vCPU | 5,000 ($0.005) |
| 1 MB RAM | 2 ($0.000002) |
| 1 GB Disk | 100 ($0.0001) |

Example: 2 vCPUs + 1GB RAM + 10GB disk for 1 hour = $0.012048

## Accepted Payment Networks & Tokens

### x402 Protocol

| Network | Chain ID | Token | Contract | Decimals |
|---------|----------|-------|----------|----------|
| Base Sepolia | eip155:84532 | USDC | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | 6 |
| Celo | eip155:42220 | cUSD | `0x765DE816845861e75A25fCA122bb6898B8B1282a` | 18 |

### MPP Protocol (Tempo)

| Token | Contract Address | Decimals |
|-------|-----------------|----------|
| pathUSD | `0x20c0000000000000000000000000000000000000` | 6 |
| AlphaUSD | `0x20c0000000000000000000000000000000000001` | 6 |
| BetaUSD | `0x20c0000000000000000000000000000000000002` | 6 |
| ThetaUSD | `0x20c0000000000000000000000000000000000003` | 6 |

## Testnet Faucets

```bash
# Tempo mainnet (pathUSD)
cast rpc tempo_fundAddress YOUR_ADDRESS --rpc-url https://rpc.tempo.xyz

# Base Sepolia USDC — use the Base Sepolia faucet at https://www.alchemy.com/faucets/base-sepolia
# Celo — use the Celo faucet at https://faucet.celo.org
```

## Wallet Integration: Open Wallet Standard (OWS)

OWS provides a chain-agnostic wallet for agents. One vault, one interface, every chain.

```bash
# Install OWS
curl -fsSL https://openwallet.sh/install.sh | bash

# Create a wallet
ows wallet create --name "agent-treasury"

# Sign transactions for any chain
ows sign tx --wallet agent-treasury --chain evm --tx-hex "..."
```

For Claude/OpenAI agents, add OWS as an MCP server:

```json
{
  "mcpServers": {
    "ows": { "command": "ows", "args": ["serve", "--mcp"] }
  }
}
```

Then the agent can call `ows_sign` to sign x402 payment authorizations without ever seeing private keys.

Learn more: [openwallet.sh](https://openwallet.sh)

## Quick Reference

| Item | Value |
|------|-------|
| API Endpoint | `https://YOUR_MPP_HOST:8402` |
| Payment Protocols | x402 (Base, Celo) + MPP (Tempo) |
| x402 Facilitator | `https://x402.org/facilitator` |
| Tempo RPC | `https://rpc.tempo.xyz` |
| Tempo Chain ID | 4217 |
| Base Sepolia Chain ID | 84532 |
| Celo Chain ID | 42220 |
| SSH User | `root` |
| VM OS | Ubuntu 24.04 LTS (aarch64) |
