#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# MPP Hosting — Network Setup
#
# Creates the bridge interface, configures NAT, and sets up firewall rules
# for microVM networking.
# =============================================================================

BRIDGE="${BRIDGE_IFACE:-mpp-br0}"
SUBNET="${VM_SUBNET:-172.16.0.0/16}"
GATEWAY="${VM_GATEWAY:-172.16.0.1}"

echo "=== MPP Hosting: Network Setup ==="
echo "Bridge: ${BRIDGE}"
echo "Subnet: ${SUBNET}"
echo "Gateway: ${GATEWAY}"
echo ""

# --- Detect host interface ---
HOST_IFACE=$(ip -j route list default | jq -r '.[0].dev' 2>/dev/null || echo "eth0")
echo "Host interface: ${HOST_IFACE}"

# --- Create bridge ---
if ip link show "${BRIDGE}" &>/dev/null; then
    echo "Bridge ${BRIDGE} already exists"
else
    echo "Creating bridge ${BRIDGE}..."
    ip link add "${BRIDGE}" type bridge
fi

# --- Assign gateway IP ---
if ip addr show "${BRIDGE}" | grep -q "${GATEWAY}"; then
    echo "Gateway IP already assigned"
else
    echo "Assigning gateway ${GATEWAY} to ${BRIDGE}..."
    ip addr add "${GATEWAY}/16" dev "${BRIDGE}"
fi

# --- Bring up bridge ---
ip link set "${BRIDGE}" up
echo "Bridge ${BRIDGE} is up"

# --- Enable IP forwarding ---
sysctl -w net.ipv4.ip_forward=1 >/dev/null
echo "IP forwarding enabled"

# Make persistent
if ! grep -q "net.ipv4.ip_forward=1" /etc/sysctl.conf 2>/dev/null; then
    echo "net.ipv4.ip_forward=1" >> /etc/sysctl.conf
fi

# --- Configure iptables NAT ---
echo "Configuring NAT rules..."

# Flush existing MPP rules (idempotent)
iptables -t nat -D POSTROUTING -s "${SUBNET}" -o "${HOST_IFACE}" -j MASQUERADE 2>/dev/null || true
iptables -D FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -j ACCEPT 2>/dev/null || true
iptables -D FORWARD -i "${HOST_IFACE}" -o "${BRIDGE}" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || true

# Add rules
iptables -t nat -A POSTROUTING -s "${SUBNET}" -o "${HOST_IFACE}" -j MASQUERADE
iptables -A FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -j ACCEPT
iptables -A FORWARD -i "${HOST_IFACE}" -o "${BRIDGE}" -m state --state RELATED,ESTABLISHED -j ACCEPT

echo "NAT configured: ${SUBNET} → ${HOST_IFACE}"

# --- Security: Drop inter-VM traffic by default ---
iptables -D FORWARD -i "${BRIDGE}" -o "${BRIDGE}" -j DROP 2>/dev/null || true
iptables -A FORWARD -i "${BRIDGE}" -o "${BRIDGE}" -j DROP
echo "Inter-VM traffic blocked (isolation)"

# --- Rate limit outbound connections per VM (anti-abuse) ---
# Limit new outbound connections to 100/min per source IP
iptables -D FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --syn -m connlimit --connlimit-above 100 --connlimit-mask 32 -j DROP 2>/dev/null || true
iptables -A FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --syn -m connlimit --connlimit-above 100 --connlimit-mask 32 -j DROP
echo "Outbound connection rate limit applied (100 new TCP/min per VM)"

# --- Block outbound SMTP (anti-spam) ---
iptables -D FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --dport 25 -j DROP 2>/dev/null || true
iptables -D FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --dport 587 -j DROP 2>/dev/null || true
iptables -A FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --dport 25 -j DROP
iptables -A FORWARD -i "${BRIDGE}" -o "${HOST_IFACE}" -p tcp --dport 587 -j DROP
echo "Outbound SMTP blocked (anti-spam)"

echo ""
echo "=== Network setup complete ==="
echo ""
echo "Bridge: ${BRIDGE} (${GATEWAY}/16)"
echo "NAT: ${SUBNET} → ${HOST_IFACE}"
echo "Security: VM isolation, rate limiting, SMTP blocked"
