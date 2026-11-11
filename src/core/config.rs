use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Runtime configuration. Production uses an explicit --production launch.
/// Where one file job runs (7.10). Remote is the base URL of the file worker
/// binary (`src/bin/file-worker.rs`); for vision it is another Ollama URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLocation {
    Local,
    Remote(String),
}
#[derive(Debug, Clone)]
pub struct Config {
    pub timing_sample_count: usize,
    pub timing_cache_ttl: std::time::Duration,
    pub telemetry_page_default: usize,
    pub telemetry_page_max: usize,
    pub run_retention_days: i64,
    pub run_purge_interval: std::time::Duration,
    pub development: bool,
    pub log_stream_capacity: usize,
    pub log_line_bytes: usize,
    pub thinking_timeout: std::time::Duration,
    pub thinking_output_tokens: u32,
    pub thinking_plan_steps: usize,
    pub thinking_plan_instruction: String,
    pub thinking_work_instruction: String,
    pub thinking_check_instruction: String,
    pub thinking_final_instruction: String,
    pub temporary_ttl: std::time::Duration,
    pub file_bytes: usize,
    pub file_form_overhead: usize,
    pub ocr_page_limit: usize,
    pub pdfium_path: std::path::PathBuf,
    pub ocr_languages: String,
    pub ocr_render_width: i32,
    pub ocr_render_height: i32,
    pub medium_web_timeout: std::time::Duration,
    pub deep_web_timeout: std::time::Duration,
    pub medium_web_depth: usize,
    pub deep_web_depth: usize,
    pub web_pages_per_level: usize,
    pub web_robots_bytes: usize,
    pub web_file_extensions: Vec<&'static str>,
    pub medium_query_count: usize,
    pub deep_query_count: usize,
    pub query_output_tokens: u32,
    pub query_temperature: f64,
    pub query_instruction: String,
    pub fast_web_pages: usize,
    pub fast_web_timeout: std::time::Duration,
    pub web_page_timeout: std::time::Duration,
    pub web_parallel_pages: usize,
    pub web_chunk_tokens: usize,
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
    pub ffmpeg_path: std::path::PathBuf,
    pub whisper_model_path: std::path::PathBuf,
    pub whisper_threads: i32,
    pub audio_max_seconds: f64,
    pub extract_location: JobLocation,
    pub ocr_location: JobLocation,
    pub transcribe_location: JobLocation,
    pub vision_location: JobLocation,
    pub worker_listen: SocketAddr,
    pub worker_timeout: std::time::Duration,
    pub ollama_url: String,
    pub model_context_tokens: u64,
    pub ollama_keep_alive: String,
    pub ollama_connect_timeout: std::time::Duration,
    pub ollama_max_frame_bytes: usize,
    pub token_estimate_bytes_per_token: u64,
    pub chat_template_token_reserve: u64,
    pub usage_window_hours: i64,
    pub usage_bucket_ttl: std::time::Duration,
    pub loyalty_api_base: &'static str,
    pub loyalty_source_timeout: std::time::Duration,
    pub loyalty_refresh_interval: std::time::Duration,
    pub loyalty_audit_request_spacing: std::time::Duration,
    pub loyalty_audit_batch_users: usize,
    pub loyalty_audit_row_limit: usize,
    pub loyalty_snapshot_ttl: std::time::Duration,
    pub queue_wait_timeout: std::time::Duration,
    pub local_queue_name: &'static str,
    pub queue_snapshot_ttl: std::time::Duration,
    pub run_read_batch: usize,
    pub allowed_apps: Vec<&'static str>,
    pub memory_workspace: &'static str,
    pub memory_graph_max_depth: usize,
    pub run_database: String,
    pub run_test_database: String,
    pub document_timeout: std::time::Duration,
    pub database_connections: u32,
    pub cache_operation_timeout: std::time::Duration,
    pub listen: SocketAddr,
}

impl Config {
    pub fn production() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8003),
            hippocampus_url: "http://127.0.0.1:8001".into(),
            ..Self::default()
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timing_sample_count:20,
            timing_cache_ttl:std::time::Duration::from_secs(30*86400),
            telemetry_page_default:50,
            telemetry_page_max:200,
            run_retention_days:30,
            run_purge_interval:std::time::Duration::from_secs(24*3600),
            development:false,
            log_stream_capacity:1024,
            log_line_bytes:16_384,
            thinking_timeout:std::time::Duration::from_secs(180),
            thinking_output_tokens:512,
            thinking_plan_steps:4,
            thinking_plan_instruction:"Your only task is to plan how to answer the question. Write at most 4 steps, one step per line. Use plain text.".into(),
            thinking_work_instruction:"Your only task is to work through the plan with the given texts. Write free text notes. Use the earlier check to correct unsupported claims. Do not invent support.".into(),
            thinking_check_instruction:"Your only task is to compare the work notes with the given texts. List each claim the texts do not support, one per line. If every claim is supported, write NONE. Use plain text.".into(),
            thinking_final_instruction:"Write the final answer from the given texts, work notes and check. Remove unsupported claims. Answer in the user's language. Use plain text.".into(),
            temporary_ttl:std::time::Duration::from_secs(3600),
            file_bytes:25 * 1024 * 1024,
            file_form_overhead: 64 * 1024,
            ocr_page_limit:20,
            pdfium_path:"models/pdfium/lib/libpdfium.so".into(),
            ocr_languages:"eng+por+tha".into(),
            ocr_render_width:2480,
            ocr_render_height:3508,
            medium_web_timeout:std::time::Duration::from_secs(30),
            deep_web_timeout:std::time::Duration::from_secs(60),
            medium_web_depth:1,
            deep_web_depth:2,
            web_pages_per_level:5,
            web_robots_bytes:500_000,
            web_file_extensions:vec!["pdf","jpg","jpeg","png","gif","webp","svg","ico","bmp","tif","tiff","avif","zip","rar","7z","tar","gz","bz2","xz","exe","dmg","iso","mp3","wav","ogg","mp4","mov","webm","doc","docx","xls","xlsx","ppt","pptx","csv","json","xml","txt"],
            medium_query_count:3,
            deep_query_count:4,
            query_output_tokens:256,
            query_temperature:0.0,
            query_instruction:"Write up to {limit} short web search queries for the question. Write one query per line. Use plain text only. Do not answer the question.".into(),
            fast_web_pages:5,
            fast_web_timeout:std::time::Duration::from_secs(15),
            web_page_timeout:std::time::Duration::from_secs(8),
            web_parallel_pages:5,
            web_chunk_tokens:500,
            search_adapters:vec![SearchEngine::DuckDuckGo,SearchEngine::Wikipedia],
            duckduckgo_html_url:"https://html.duckduckgo.com/html/".into(),
            wikipedia_api_url:"https://en.wikipedia.org/w/api.php".into(),
            web_user_agent:"AsodyaCortex/2".into(),
            web_search_timeout:std::time::Duration::from_secs(8),
            search_results_per_adapter:10,
            search_rrf_k:60,
            web_html_bytes:2_000_000,
            hippocampus_url:"http://127.0.0.1:18001".into(),
            hippocampus_timeout:std::time::Duration::from_secs(5),
            history_page_default:20,
            history_cache_exchanges:20,
            history_cache_ttl:std::time::Duration::from_secs(3600),
            chat_title_characters:60,
            event_retention:std::time::Duration::from_secs(3600),
            event_read_batch:200,
            stream_heartbeat:std::time::Duration::from_secs(5),
            shield_model_path:"models/protectai-deberta-v3-base-prompt-injection-v2/model.onnx".into(),
            shield_tokenizer_path:"models/protectai-deberta-v3-base-prompt-injection-v2/tokenizer.json".into(),
            shield_runtime_path:"models/onnxruntime/lib/libonnxruntime.so".into(),
            shield_threshold:0.5,
            shield_cpu_threads:2,
            context_format: ContextFormat::Plain,
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
            ffmpeg_path: "/usr/bin/ffmpeg".into(),
            // large-v3-turbo q5_0 GGML (7.6.5), shared with the canonical checkout.
            whisper_model_path:
                "/home/main-server/Documents/Others/Asodya/cortex_api/var/whisper-models/ggml-large-v3-turbo-q5_0.bin"
                    .into(),
            whisper_threads: 4,
            audio_max_seconds: 600.0,
            // Owner decision (7.10.8): everything runs on the MSI now.
            extract_location: JobLocation::Local,
            ocr_location: JobLocation::Local,
            transcribe_location: JobLocation::Local,
            vision_location: JobLocation::Local,
            worker_listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 18090),
            worker_timeout: std::time::Duration::from_secs(900),
            ollama_url: "http://127.0.0.1:11434".into(),
            model_context_tokens: 8192,
            ollama_keep_alive: "2m".into(),
            ollama_connect_timeout: std::time::Duration::from_secs(5),
            ollama_max_frame_bytes: 1024 * 1024,
            token_estimate_bytes_per_token: 1,
            chat_template_token_reserve: 64,
            usage_window_hours: 24,
            usage_bucket_ttl: std::time::Duration::from_secs(24 * 3600),
            loyalty_api_base: "https://api.supabase.com/",
            loyalty_source_timeout: std::time::Duration::from_secs(30),
            loyalty_refresh_interval: std::time::Duration::from_secs(86400),
            loyalty_audit_request_spacing: std::time::Duration::from_secs(7),
            loyalty_audit_batch_users: 10,
            loyalty_audit_row_limit: 1000,
            loyalty_snapshot_ttl: std::time::Duration::from_secs(172800),
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
            memory_graph_max_depth: 3,
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
    Plain,
}

#[derive(Debug, Clone, Copy)]
pub enum SearchEngine {
    DuckDuckGo,
    Wikipedia,
}

// Owner night-run rule: no connection to Pong, including from web URLs.
pub const FORBIDDEN_WEB_IP: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 168, 1, 104);
