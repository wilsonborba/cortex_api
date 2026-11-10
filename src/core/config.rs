use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Development listener. Production binding belongs to the cutover issue.
#[derive(Debug, Clone)]
pub struct Config {
    pub text_model: String,
    pub vision_model: String,
    pub ollama_url: String,
    pub model_context_tokens: u64,
    pub ollama_keep_alive: String,
    pub ollama_connect_timeout: std::time::Duration,
    pub ollama_max_frame_bytes: usize,
    pub token_estimate_bytes_per_token: u64,
    pub chat_template_token_reserve: u64,
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
            text_model: "hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0".into(),
            vision_model: "qwen2.5vl:7b".into(),
            ollama_url: "http://127.0.0.1:11434".into(),
            model_context_tokens: 8192,
            ollama_keep_alive: "2m".into(),
            ollama_connect_timeout: std::time::Duration::from_secs(5),
            ollama_max_frame_bytes: 1024 * 1024,
            token_estimate_bytes_per_token: 1,
            chat_template_token_reserve: 64,
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

impl Config {
    pub fn estimate_tokens(&self, text: &str) -> u64 {
        (text.len() as u64).div_ceil(self.token_estimate_bytes_per_token.max(1))
    }
}
