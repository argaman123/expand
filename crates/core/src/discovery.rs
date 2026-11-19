//! Finding the Mac on the local network with mDNS. A convenience only: the app always
//! lets the address be typed, because discovery often fails between Wi-Fi and Ethernet.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;
use std::time::Duration;

pub const SERVICE: &str = "_sidescreen._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub id: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub struct DiscoveryError(pub String);

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "network discovery failed: {}", self.0)
    }
}

impl std::error::Error for DiscoveryError {}

/// Keeps the service announced until dropped.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

pub fn advertise(name: &str, id: &str, port: u16) -> Result<Advertisement, DiscoveryError> {
    let err = |e: mdns_sd::Error| DiscoveryError(e.to_string());
    let daemon = ServiceDaemon::new().map_err(err)?;
    let host = format!("sidescreen-{id}.local.");
    let info = ServiceInfo::new(SERVICE, name, &host, "", port, &[("id", id)][..])
        .map_err(err)?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(err)?;
    Ok(Advertisement { daemon, fullname })
}

pub async fn find_first(timeout: Duration) -> Option<Found> {
    find_first_of(SERVICE, timeout).await
}

/// The address to reach a found computer at. Only IPv4 will do: the app and its engines
/// listen on IPv4 only.
fn usable_address<'a>(mut addrs: impl Iterator<Item = &'a IpAddr>) -> Option<IpAddr> {
    addrs.find(|a| a.is_ipv4()).copied()
}

/// Browses for `service` and returns the first instance that resolves with an IPv4 address.
pub async fn find_first_of(service: &str, timeout: Duration) -> Option<Found> {
    let daemon = ServiceDaemon::new().ok()?;
    let events = daemon.browse(service).ok()?;
    let search = async {
        loop {
            match events.recv_async().await {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    // An answer without an IPv4 address is followed by another once it is known.
                    let Some(addr) = usable_address(info.get_addresses().iter()) else {
                        continue;
                    };
                    let suffix = format!(".{service}");
                    let name = info.get_fullname().strip_suffix(&suffix).unwrap_or(info.get_fullname());
                    return Some(Found {
                        name: name.to_string(),
                        id: info.get_property_val_str("id").unwrap_or_default().to_string(),
                        host: addr.to_string(),
                        port: info.get_port(),
                    });
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    };
    let found = tokio::time::timeout(timeout, search).await.ok().flatten();
    let _ = daemon.shutdown();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    // Needs multicast on the local machine: run with `cargo test discovery -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn advertise_then_find_on_same_machine() {
        let _ad = advertise("test-mac", "abc", 47800).unwrap();
        let found = find_first(Duration::from_secs(3)).await.expect("service not found");
        assert_eq!(found.id, "abc");
        assert_eq!(found.name, "test-mac");
        assert_eq!(found.port, 47800);
        assert!(found.host.parse::<std::net::IpAddr>().is_ok(), "host was {}", found.host);
    }

    // The Mac listens on IPv4 only, and so do the engines. On 2026-11-11 discovery handed the PC
    // the Mac's IPv6 address, which nothing could reach.
    #[test]
    fn an_ipv6_only_answer_is_not_usable() {
        let addrs: Vec<IpAddr> = vec!["2001:db8::1".parse().unwrap(), "fe80::1".parse().unwrap()];
        assert_eq!(usable_address(addrs.iter()), None);
    }

    #[test]
    fn the_ipv4_address_is_picked_from_a_mixed_answer() {
        let addrs: Vec<IpAddr> = vec!["2001:db8::1".parse().unwrap(), "192.168.1.50".parse().unwrap()];
        assert_eq!(usable_address(addrs.iter()), Some("192.168.1.50".parse().unwrap()));
    }

    #[tokio::test]
    async fn nothing_advertised_gives_none() {
        // A service type nobody advertises.
        assert!(find_first_of("_sidescreen-none._tcp.local.", Duration::from_millis(300)).await.is_none());
    }
}
