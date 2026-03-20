output "server_ip" {
  description = "Public IP of the MPP hosting server"
  value       = hcloud_server.mpp_host.ipv4_address
}

output "server_id" {
  description = "Hetzner server ID"
  value       = hcloud_server.mpp_host.id
}

output "api_url" {
  description = "MPP Hosting API URL"
  value       = "http://${hcloud_server.mpp_host.ipv4_address}:8402"
}

output "ssh_command" {
  description = "SSH command to connect to the server"
  value       = "ssh root@${hcloud_server.mpp_host.ipv4_address}"
}
