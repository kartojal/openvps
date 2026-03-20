use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::Mutex;

/// Manages a pool of IP addresses from a subnet, allocating them to VMs.
pub struct IpPool {
    subnet: Ipv4Net,
    /// Gateway IP (first usable address in subnet)
    gateway: Ipv4Addr,
    /// IPs currently allocated
    allocated: Mutex<HashSet<Ipv4Addr>>,
}

impl IpPool {
    /// Create a new IP pool from a CIDR subnet string (e.g., "172.16.0.0/16").
    /// Reserves .1 as the gateway address.
    pub fn new(subnet_cidr: &str, existing_allocations: &[String]) -> Result<Self> {
        let subnet: Ipv4Net = subnet_cidr
            .parse()
            .with_context(|| format!("Invalid subnet CIDR: {}", subnet_cidr))?;

        let network = subnet.network();
        let gateway = Ipv4Addr::new(
            network.octets()[0],
            network.octets()[1],
            network.octets()[2],
            network.octets()[3].wrapping_add(1),
        );

        let mut allocated = HashSet::new();
        // Reserve network address and gateway
        allocated.insert(network);
        allocated.insert(gateway);
        allocated.insert(subnet.broadcast());

        // Load existing allocations
        for ip_str in existing_allocations {
            if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                allocated.insert(ip);
            }
        }

        Ok(Self {
            subnet,
            gateway,
            allocated: Mutex::new(allocated),
        })
    }

    /// Allocate the next available IP from the pool.
    pub fn allocate(&self) -> Result<Ipv4Addr> {
        let mut allocated = self.allocated.lock().unwrap();

        for ip in self.subnet.hosts() {
            if !allocated.contains(&ip) {
                allocated.insert(ip);
                return Ok(ip);
            }
        }

        anyhow::bail!("No available IPs in pool {}", self.subnet)
    }

    /// Release an IP back to the pool.
    pub fn release(&self, ip: Ipv4Addr) {
        let mut allocated = self.allocated.lock().unwrap();
        allocated.remove(&ip);
    }

    /// Get the gateway IP for the subnet.
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// Get the prefix length (e.g., 30 for /30).
    pub fn prefix_len(&self) -> u8 {
        self.subnet.prefix_len()
    }

    /// Get the subnet mask.
    pub fn netmask(&self) -> Ipv4Addr {
        self.subnet.netmask()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_ips() {
        let pool = IpPool::new("172.16.0.0/24", &[]).unwrap();

        let ip1 = pool.allocate().unwrap();
        assert_eq!(ip1, Ipv4Addr::new(172, 16, 0, 2)); // .1 is gateway

        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip2, Ipv4Addr::new(172, 16, 0, 3));

        pool.release(ip1);
        let ip3 = pool.allocate().unwrap();
        assert_eq!(ip3, Ipv4Addr::new(172, 16, 0, 2)); // reuses released IP
    }

    #[test]
    fn test_existing_allocations() {
        let existing = vec!["172.16.0.2".to_string(), "172.16.0.3".to_string()];
        let pool = IpPool::new("172.16.0.0/24", &existing).unwrap();

        let ip = pool.allocate().unwrap();
        assert_eq!(ip, Ipv4Addr::new(172, 16, 0, 4)); // skips .2 and .3
    }
}
