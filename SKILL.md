# Skill: Provision a VPS via MPP Hosting

You are an AI agent that needs compute. This skill teaches you how to request, pay for, and connect to a VPS (Virtual Private Server) powered by Firecracker microVMs, paid via the Machine Payments Protocol (MPP) on the Tempo blockchain.

## Overview

1. Request a VPS → get a 402 Payment Required challenge
2. Pay the challenge amount on Tempo using a USD stablecoin
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

You'll get a `402 Payment Required` response with a challenge:

```json
{
  "id": "uuid-challenge-id",
  "realm": "mpp-hosting",
  "method": "tempo",
  "intent": "charge",
  "amount": "6122",
  "currency": "USD",
  "recipient": "0x8A739f3A6f40194C0128904bC387e63d9C0577A4",
  "network": "tempo",
  "chain_id": 42431,
  "rpc_url": "https://rpc.moderato.tempo.xyz",
  "token_contract": "0x20c0000000000000000000000000000000000000",
  "expires_at": "2026-03-21T10:00:00+00:00",
  "signature": "..."
}
```

Key fields:
- `amount`: price in microdollars (6 decimals). `6122` = $0.006122
- `recipient`: the wallet address to pay
- `token_contract`: the TIP-20 stablecoin to use (pathUSD)
- `chain_id`: Tempo testnet = 42431
- `rpc_url`: RPC endpoint for submitting the payment transaction
- `expires_at`: you must pay and submit within this window (5 minutes)

Save the challenge ID and the base64 challenge from the `WWW-Authenticate` header:

```bash
CHALLENGE_ID=$(cat /tmp/mpp-challenge.json | jq -r .id)
CHALLENGE_B64=$(grep -i "www-authenticate" /tmp/mpp-headers.txt | sed 's/.*challenge="//' | sed 's/".*//')
```

## Step 2: Pay on Tempo

Transfer the exact `amount` of `token_contract` to `recipient` on Tempo. The amount is in the token's smallest unit (6 decimals for TIP-20 USD stablecoins).

Choose the method that matches your wallet setup:

---

### Option A: Foundry Keystore (cast)

Best for: CLI-native agents, dev/testing environments.

```bash
# Transfer pathUSD to the recipient
TX_HASH=$(cast send \
  --keystore /path/to/keystore.json \
  --password "your-password" \
  --rpc-url https://rpc.moderato.tempo.xyz \
  --chain 42431 \
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
cast rpc tempo_fundAddress YOUR_ADDRESS --rpc-url https://rpc.moderato.tempo.xyz
```

---

### Option B: Raw Private Key (cast)

Best for: Agents with a private key in memory or environment variable.

```bash
TX_HASH=$(cast send \
  --private-key $PRIVATE_KEY \
  --rpc-url https://rpc.moderato.tempo.xyz \
  --chain 42431 \
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
NONCE=$(cast nonce YOUR_ADDRESS --rpc-url https://rpc.moderato.tempo.xyz)

# 3. Sign and send (using cast or any eth library)
TX_HASH=$(cast send \
  --private-key $KEY \
  --rpc-url https://rpc.moderato.tempo.xyz \
  --chain 42431 \
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

## Step 3: Submit Payment Proof

Once you have the `TX_HASH`, build a credential and resubmit:

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

## Accepted Payment Tokens

Any USD-denominated TIP-20 stablecoin on Tempo:

| Token | Contract Address |
|-------|-----------------|
| pathUSD | `0x20c0000000000000000000000000000000000000` |
| AlphaUSD | `0x20c0000000000000000000000000000000000001` |
| BetaUSD | `0x20c0000000000000000000000000000000000002` |
| ThetaUSD | `0x20c0000000000000000000000000000000000003` |

## Testnet Faucet

Get free testnet stablecoins:

```bash
cast rpc tempo_fundAddress YOUR_ADDRESS --rpc-url https://rpc.moderato.tempo.xyz
```

## Quick Reference

| Item | Value |
|------|-------|
| API Endpoint | `https://YOUR_MPP_HOST:8402` |
| Network | Tempo (testnet: Moderato) |
| Chain ID | 42431 |
| RPC URL | `https://rpc.moderato.tempo.xyz` |
| Default Token | pathUSD (`0x20c0...0000`) |
| Token Decimals | 6 |
| SSH User | `root` |
| VM OS | Ubuntu 24.04 LTS (aarch64) |
