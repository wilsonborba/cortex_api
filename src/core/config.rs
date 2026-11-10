use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Development listener. Production binding belongs to the cutover issue.
#[derive(Debug, Clone)]
pub struct Config {
    pub allowed_apps: Vec<&'static str>,
    pub memory_workspace: &'static str,
    pub run_database: String,
    pub run_test_database: String,
    pub document_timeout: std::time::Duration,
    pub database_connections: u32,
    pub cache_operation_timeout: std::time::Duration,
    pub listen: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allowed_apps: vec![
                "annotations",
                "api_for_apps",
                "auth_apps",
                "certifications",
                "cortex",
                "domain",
                "oryzaelo",
                "reports",
                "wilson_borba",
            ],
            memory_workspace: "cortex",
            run_database: "cortex_runs".into(),
            run_test_database: "cortex_runs_test".into(),
            document_timeout: std::time::Duration::from_secs(5),
            database_connections: 5,
            cache_operation_timeout: std::time::Duration::from_millis(250),
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18003),
        }
    }
}
