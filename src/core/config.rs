use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Development listener. Production binding belongs to the cutover issue.
#[derive(Debug, Clone)]
pub struct Config {
    pub usage_window_hours: i64,
    pub usage_bucket_ttl: std::time::Duration,
    pub loyalty_snapshot_ttl: std::time::Duration,
    pub queue_wait_timeout: std::time::Duration,
    pub queue_snapshot_ttl: std::time::Duration,
    pub run_read_batch: usize,
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
            usage_window_hours: 24,
            usage_bucket_ttl: std::time::Duration::from_secs(24 * 3600),
            loyalty_snapshot_ttl: std::time::Duration::from_secs(3600),
            queue_wait_timeout: std::time::Duration::from_secs(300),
            queue_snapshot_ttl: std::time::Duration::from_secs(301),
            run_read_batch: 200,
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
