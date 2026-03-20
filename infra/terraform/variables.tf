variable "hcloud_token" {
  description = "Hetzner Cloud API token"
  type        = string
  sensitive   = true
}

variable "environment" {
  description = "Environment name (e.g., prod, staging)"
  type        = string
  default     = "prod"
}

variable "server_type" {
  description = "Hetzner server type (e.g., cx42 for cloud, AX162 for dedicated)"
  type        = string
  default     = "cx42" # 8 vCPU, 16GB RAM — good for testing. Use dedicated for production.
}

variable "location" {
  description = "Hetzner datacenter location"
  type        = string
  default     = "nbg1" # Nuremberg
}

variable "ssh_public_key_path" {
  description = "Path to SSH public key for server access"
  type        = string
  default     = "~/.ssh/id_ed25519.pub"
}

variable "ssh_allowed_ips" {
  description = "CIDR ranges allowed to SSH into the server"
  type        = list(string)
  default     = ["0.0.0.0/0", "::/0"] # Restrict in production
}
