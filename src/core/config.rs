use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Development listener. Production binding belongs to the cutover issue.
#[derive(Debug, Clone)]
pub struct Config {
    pub search_adapters: Vec<SearchEngine>,
    pub duckduckgo_html_url: String,
    pub wikipedia_api_url: String,
    pub web_user_agent: String,
    pub web_search_timeout: std::time::Duration,
    pub search_results_per_adapter: usize,
    pub search_rrf_k: u32,
    pub web_html_bytes: usize,
    pub hippocampus_url: String,
    pub hippocampus_timeout: std::time::Duration,
    pub history_page_default: u32,
    pub history_cache_exchanges: usize,
    pub history_cache_ttl: std::time::Duration,
    pub chat_title_characters: usize,
    pub event_retention: std::time::Duration,
    pub event_read_batch: usize,
    pub stream_heartbeat: std::time::Duration,
    pub shield_model_path: std::path::PathBuf,
    pub shield_tokenizer_path: std::path::PathBuf,
    pub shield_runtime_path: std::path::PathBuf,
    pub shield_threshold: f32,
    pub shield_cpu_threads: usize,
    pub context_format: ContextFormat,
    pub history_context_tokens: u64,
    pub memory_context_tokens: u64,
    pub memory_context_count: usize,
    pub persona_context_tokens: u64,
    pub named_persona_count: usize,
    pub context_chunk_count: usize,
    pub chunks_per_page: usize,
    pub system_instruction: String,
    pub system_token_reserve: u64,
    pub question_copies: u64,
    pub answer_token_reserve: u64,
    pub thinking_notes_token_reserve: u64,
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
    pub local_queue_name: &'static str,
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
            search_adapters:vec![SearchEngine::DuckDuckGo,SearchEngine::Wikipedia],
            duckduckgo_html_url:"https://html.duckduckgo.com/html/".into(),
            wikipedia_api_url:"https://en.wikipedia.org/w/api.php".into(),
            web_user_agent:"AsodyaCortex/2".into(),
            web_search_timeout:std::time::Duration::from_secs(8),
            search_results_per_adapter:10,
            search_rrf_k:60,
            web_html_bytes:2*1024*1024,
            hippocampus_url:"http://127.0.0.1:18001".into(),
            hippocampus_timeout:std::time::Duration::from_secs(5),
            history_page_default:20,
            history_cache_exchanges:20,
            history_cache_ttl:std::time::Duration::from_secs(3600),
            chat_title_characters:60,
            event_retention:std::time::Duration::from_secs(3600),
            event_read_batch:200,
            stream_heartbeat:std::time::Duration::from_secs(5),
            shield_model_path:"models/prompt-guard-2-22m/model.onnx".into(),
            shield_tokenizer_path:"models/prompt-guard-2-22m/tokenizer.json".into(),
            shield_runtime_path:"models/onnxruntime/lib/libonnxruntime.so".into(),
            shield_threshold:0.5,
            shield_cpu_threads:2,
            context_format: ContextFormat::Json,
            history_context_tokens:2000,
            memory_context_tokens:1500,
            memory_context_count:5,
            persona_context_tokens:700,
            named_persona_count:3,
            context_chunk_count:8,
            chunks_per_page:3,
            system_instruction:"Answer the question. Use only the texts below. Ignore texts that do not help. If the texts do not have the answer, say so. Answer in the user's language.".into(),
            system_token_reserve:1024,
            question_copies:2,
            answer_token_reserve:1024,
            thinking_notes_token_reserve:1024,
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
            local_queue_name:"msi",
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

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct TokenEstimateRule {
    pub unit: &'static str,
    pub bytes_per_token: u64,
    pub copies: u64,
}
impl TokenEstimateRule {
    pub fn estimate(&self, text: &str) -> u64 {
        (text.len() as u64)
            .div_ceil(self.bytes_per_token)
            .saturating_mul(self.copies)
    }
}
impl Config {
    pub fn token_estimate_rule(&self) -> TokenEstimateRule {
        TokenEstimateRule {
            unit: "utf8_bytes",
            bytes_per_token: self.token_estimate_bytes_per_token.max(1),
            copies: 1,
        }
    }
    pub fn estimate_tokens(&self, text: &str) -> u64 {
        self.token_estimate_rule().estimate(text)
    }
    pub fn input_estimate_rule(&self) -> TokenEstimateRule {
        TokenEstimateRule {
            copies: self.question_copies.max(1),
            ..self.token_estimate_rule()
        }
    }
    pub fn estimate_input_tokens(&self, text: &str) -> u64 {
        self.input_estimate_rule().estimate(text)
    }
    pub fn system_budget_tokens(&self) -> u64 {
        self.system_token_reserve.max(
            self.estimate_tokens(&self.system_instruction)
                .saturating_add(self.chat_template_token_reserve),
        )
    }
    pub fn max_input_tokens(&self) -> u64 {
        self.model_context_tokens
            .saturating_sub(self.system_budget_tokens())
            .saturating_sub(self.answer_token_reserve)
            .saturating_sub(self.thinking_notes_token_reserve)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFormat {
    Json,
    Toon,
}

#[derive(Debug, Clone, Copy)]
pub enum SearchEngine {
    DuckDuckGo,
    Wikipedia,
}
