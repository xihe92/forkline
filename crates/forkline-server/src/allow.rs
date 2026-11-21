//! IP networks for the admin allowlist: `10.0.0.0/8`, `::1/128`, or a bare
//! address (a single host). Standard library only.

use std::net::IpAddr;
use std::str::FromStr;

/// An IPv4 or IPv6 network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Network {
    addr: IpAddr,
    prefix: u8,
}

impl FromStr for Network {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = addr.trim().parse().map_err(|_| format!("`{s}` is not an IP address or network"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(p) => match p.trim().parse::<u8>() {
                Ok(p) if p <= max => p,
                _ => return Err(format!("`{s}`: prefix length must be 0 to {max}")),
            },
        };
        Ok(Self { addr: addr.to_canonical(), prefix })
    }
}

impl Network {
    /// True when `ip` is inside the network. IPv4-mapped IPv6 addresses
    /// count as IPv4.
    pub(crate) fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> Network {
        s.parse().unwrap()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv4() {
        assert!(net("10.0.0.0/8").contains(ip("10.255.1.2")));
        assert!(!net("10.0.0.0/8").contains(ip("11.0.0.1")));
        assert!(net("127.0.0.1").contains(ip("127.0.0.1")), "a bare address is a /32");
        assert!(!net("127.0.0.1").contains(ip("127.0.0.2")));
        assert!(net("0.0.0.0/0").contains(ip("8.8.8.8")), "/0 matches everything");
        assert!(net("192.168.1.0/24").contains(ip("::ffff:192.168.1.9")), "IPv4-mapped IPv6 counts as IPv4");
    }

    #[test]
    fn ipv6() {
        assert!(net("::1/128").contains(ip("::1")));
        assert!(!net("::1/128").contains(ip("::2")));
        assert!(net("fd00::/8").contains(ip("fd12:3456::1")));
        assert!(!net("fd00::/8").contains(ip("fe80::1")));
        assert!(!net("::/0").contains(ip("10.0.0.1")), "families never match each other");
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "10.0.0.0/33", "::1/129", "10.0.0.0/x", "not-an-ip", "10.0.0/8"] {
            assert!(bad.parse::<Network>().is_err(), "{bad}");
        }
    }
}
