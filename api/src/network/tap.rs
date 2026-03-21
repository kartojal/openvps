use anyhow::{Context, Result};
use std::net::Ipv4Addr;
use tokio::process::Command;
use tracing::{error, info};

/// Create a TAP device for a VM, attached to the host bridge.
pub async fn create_tap(
    tap_name: &str,
    vm_ip: Ipv4Addr,
    _gateway: Ipv4Addr,
    _prefix_len: u8,
    bridge_iface: &str,
) -> Result<()> {
    // Create TAP device
    run_cmd("ip", &["tuntap", "add", "dev", tap_name, "mode", "tap"])
        .await
        .with_context(|| format!("Failed to create TAP device {}", tap_name))?;

    // Attach to bridge
    run_cmd("ip", &["link", "set", tap_name, "master", bridge_iface])
        .await
        .with_context(|| format!("Failed to attach {} to bridge {}", tap_name, bridge_iface))?;

    // Bring up
    run_cmd("ip", &["link", "set", tap_name, "up"])
        .await
        .with_context(|| format!("Failed to bring up {}", tap_name))?;

    info!(tap = tap_name, ip = %vm_ip, "TAP device created");
    Ok(())
}

/// Remove a TAP device.
pub async fn destroy_tap(tap_name: &str) -> Result<()> {
    run_cmd("ip", &["link", "del", tap_name]).await.ok();
    info!(tap = tap_name, "TAP device destroyed");
    Ok(())
}

/// Set up the host bridge interface and NAT rules.
/// Called once during initialization.
#[allow(dead_code)]
pub async fn setup_bridge(
    bridge_name: &str,
    gateway: Ipv4Addr,
    prefix_len: u8,
    host_iface: &str,
) -> Result<()> {
    // Create bridge if it doesn't exist
    run_cmd("ip", &["link", "add", bridge_name, "type", "bridge"])
        .await
        .ok(); // Ignore if already exists

    // Assign gateway IP to bridge
    let cidr = format!("{}/{}", gateway, prefix_len);
    run_cmd("ip", &["addr", "add", &cidr, "dev", bridge_name])
        .await
        .ok(); // Ignore if already assigned

    // Bring up bridge
    run_cmd("ip", &["link", "set", bridge_name, "up"]).await?;

    // Enable IP forwarding
    run_cmd("sysctl", &["-w", "net.ipv4.ip_forward=1"]).await?;

    // NAT for outbound traffic from VMs
    let subnet = format!("{}/{}", gateway, prefix_len);
    run_cmd(
        "iptables",
        &[
            "-t",
            "nat",
            "-A",
            "POSTROUTING",
            "-s",
            &subnet,
            "-o",
            host_iface,
            "-j",
            "MASQUERADE",
        ],
    )
    .await
    .ok(); // Ignore if rule already exists

    // Allow forwarding to/from bridge
    run_cmd(
        "iptables",
        &[
            "-A", "FORWARD", "-i", bridge_name, "-o", host_iface, "-j", "ACCEPT",
        ],
    )
    .await
    .ok();
    run_cmd(
        "iptables",
        &[
            "-A",
            "FORWARD",
            "-i",
            host_iface,
            "-o",
            bridge_name,
            "-m",
            "state",
            "--state",
            "RELATED,ESTABLISHED",
            "-j",
            "ACCEPT",
        ],
    )
    .await
    .ok();

    info!(bridge = bridge_name, gateway = %gateway, "Bridge and NAT configured");
    Ok(())
}

/// Set up DNAT port forwarding: public_ip:public_port → vm_ip:22
pub async fn setup_port_forward(
    host_iface: &str,
    public_port: u16,
    vm_ip: Ipv4Addr,
) -> Result<()> {
    let dnat_target = format!("{}:22", vm_ip);
    let port_str = public_port.to_string();

    // DNAT: incoming traffic on public_port → vm_ip:22
    run_cmd(
        "iptables",
        &[
            "-t", "nat", "-A", "PREROUTING",
            "-i", host_iface,
            "-p", "tcp",
            "--dport", &port_str,
            "-j", "DNAT",
            "--to-destination", &dnat_target,
        ],
    )
    .await?;

    // Allow forwarded traffic to the VM
    let vm_ip_str = vm_ip.to_string();
    run_cmd(
        "iptables",
        &[
            "-A", "FORWARD",
            "-p", "tcp",
            "-d", &vm_ip_str,
            "--dport", "22",
            "-j", "ACCEPT",
        ],
    )
    .await?;

    info!(public_port, vm_ip = %vm_ip, "Port forward configured");
    Ok(())
}

/// Remove DNAT port forwarding rules for a VM.
pub async fn remove_port_forward(
    host_iface: &str,
    public_port: u16,
    vm_ip: Ipv4Addr,
) -> Result<()> {
    let dnat_target = format!("{}:22", vm_ip);
    let port_str = public_port.to_string();
    let vm_ip_str = vm_ip.to_string();

    run_cmd(
        "iptables",
        &[
            "-t", "nat", "-D", "PREROUTING",
            "-i", host_iface,
            "-p", "tcp",
            "--dport", &port_str,
            "-j", "DNAT",
            "--to-destination", &dnat_target,
        ],
    )
    .await
    .ok();

    run_cmd(
        "iptables",
        &[
            "-D", "FORWARD",
            "-p", "tcp",
            "-d", &vm_ip_str,
            "--dport", "22",
            "-j", "ACCEPT",
        ],
    )
    .await
    .ok();

    info!(public_port, vm_ip = %vm_ip, "Port forward removed");
    Ok(())
}

/// Generate a unique MAC address for a VM based on its IP.
pub fn generate_mac(vm_ip: Ipv4Addr) -> String {
    let octets = vm_ip.octets();
    // Use 06:00 prefix (locally administered, unicast)
    format!(
        "06:00:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[0], octets[1], octets[2], octets[3]
    )
}

/// Generate a TAP device name from a VM ID.
pub fn tap_name(vm_id: &str) -> String {
    // TAP device names are limited to 15 chars on Linux
    format!("tap{}", &vm_id[..8.min(vm_id.len())])
}

async fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute: {} {}", program, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(cmd = %program, args = ?args, %stderr, "Command failed");
        anyhow::bail!("{} {} failed: {}", program, args.join(" "), stderr);
    }

    Ok(())
}
