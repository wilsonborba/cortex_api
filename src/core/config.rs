use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Development listener. Production binding belongs to the cutover issue.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18003),
        }
    }
}
