# =============================================================================
# MPP Hosting — Terraform Configuration
#
# Provisions a Hetzner dedicated server and bootstraps it for microVM hosting.
# Can be adapted for other providers (Vultr, OVH, etc.)
# =============================================================================

terraform {
  required_version = ">= 1.5"

  required_providers {
    hcloud = {
      source  = "hetznercloud/hcloud"
      version = "~> 1.48"
    }
  }
}

provider "hcloud" {
  token = var.hcloud_token
}

# --- SSH Key ---
resource "hcloud_ssh_key" "mpp" {
  name       = "mpp-hosting-${var.environment}"
  public_key = file(var.ssh_public_key_path)
}

# --- Server ---
resource "hcloud_server" "mpp_host" {
  name        = "mpp-host-${var.environment}"
  server_type = var.server_type
  image       = "ubuntu-24.04"
  location    = var.location
  ssh_keys    = [hcloud_ssh_key.mpp.id]

  labels = {
    service     = "mpp-hosting"
    environment = var.environment
  }

  user_data = templatefile("${path.module}/cloud-init.yaml", {
    environment = var.environment
  })
}

# --- Firewall ---
resource "hcloud_firewall" "mpp" {
  name = "mpp-hosting-${var.environment}"

  # SSH
  rule {
    direction = "in"
    protocol  = "tcp"
    port      = "22"
    source_ips = var.ssh_allowed_ips
  }

  # MPP API
  rule {
    direction = "in"
    protocol  = "tcp"
    port      = "8402"
    source_ips = ["0.0.0.0/0", "::/0"]
  }

  # Allow all outbound
  rule {
    direction       = "out"
    protocol        = "tcp"
    port            = "any"
    destination_ips = ["0.0.0.0/0", "::/0"]
  }

  rule {
    direction       = "out"
    protocol        = "udp"
    port            = "any"
    destination_ips = ["0.0.0.0/0", "::/0"]
  }
}

resource "hcloud_firewall_attachment" "mpp" {
  firewall_id = hcloud_firewall.mpp.id
  server_ids  = [hcloud_server.mpp_host.id]
}
