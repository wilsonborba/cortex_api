use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// Shared rate-limit and cooldown store at provider/account scope (backed by Valkey in prod,
/// in-memory in isolated tests).
pub trait SharedLimitStore: Send + Sync {
    fn check_and_increment(
        &self,
        scope_key: &str,
        limit: u64,
        window_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;

    fn set_cooldown(
        &self,
        scope_key: &str,
        ttl_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn in_cooldown(
        &self,
        scope_key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;
}

#[derive(Default)]
pub struct InMemoryLimitStore {
    state: Mutex<InMemoryLimitState>,
}

#[derive(Default)]
struct InMemoryLimitState {
    counts: HashMap<String, (u64, Instant)>,
    cooldowns: HashMap<String, Instant>,
}

impl SharedLimitStore for InMemoryLimitStore {
    fn check_and_increment(
        &self,
        scope_key: &str,
        limit: u64,
        window_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        let key = scope_key.to_string();
        Box::pin(async move {
            let mut guard = self.state.lock().await;
            let now = Instant::now();
            let entry = guard
                .counts
                .entry(key)
                .or_insert((0, now + Duration::from_secs(window_secs)));
            if now >= entry.1 {
                *entry = (0, now + Duration::from_secs(window_secs));
            }
            if entry.0 >= limit {
                return Ok(false);
            }
            entry.0 += 1;
            Ok(true)
        })
    }

    fn set_cooldown(
        &self,
        scope_key: &str,
        ttl_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let key = scope_key.to_string();
        Box::pin(async move {
            let mut guard = self.state.lock().await;
            guard
                .cooldowns
                .insert(key, Instant::now() + Duration::from_secs(ttl_secs.max(1)));
            Ok(())
        })
    }

    fn in_cooldown(
        &self,
        scope_key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        let key = scope_key.to_string();
        Box::pin(async move {
            let guard = self.state.lock().await;
            Ok(guard
                .cooldowns
                .get(&key)
                .map(|until| Instant::now() < *until)
                .unwrap_or(false))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolitenessConfig {
    pub max_global_concurrency: usize,
    pub default_host_spacing_ms: u64,
    pub request_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub max_backoff_secs: u64,
}

impl Default for PolitenessConfig {
    fn default() -> Self {
        Self {
            max_global_concurrency: 4,
            default_host_spacing_ms: 500,
            request_timeout_ms: 10_000,
            max_response_bytes: 2 * 1024 * 1024,
            max_backoff_secs: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchDecision {
    Proceed,
    BlockedStop { reason: String },
    RateLimitedCooldown { wait_secs: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CoverageReport {
    Complete,
    Partial { skipped_hosts: Vec<String>, reason: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionalCacheMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl ConditionalCacheMeta {
    pub fn request_headers(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(ref etag) = self.etag {
            out.push(("If-None-Match", etag.clone()));
        }
        if let Some(ref lm) = self.last_modified {
            out.push(("If-Modified-Since", lm.clone()));
        }
        out
    }
}

/// Parses `Retry-After` or `X-RateLimit-Reset` integer seconds, bounded by [max_secs].
pub fn parse_bounded_retry_after(header_val: Option<&str>, max_secs: u64) -> Option<u64> {
    let raw = header_val?.trim();
    let secs = raw.parse::<u64>().ok()?;
    Some(secs.min(max_secs))
}

/// Enforces one direct request in flight per host, configurable per-host spacing,
/// bounded global concurrency, and clean stop on 401/403/challenge or 429 cooldown.
pub struct ResearchPolitenessGuard {
    config: PolitenessConfig,
    global_sem: Arc<Semaphore>,
    host_locks: Mutex<HashMap<String, Arc<Mutex<Option<Instant>>>>>,
    shared: Arc<dyn SharedLimitStore>,
}

pub struct HostPermit {
    _global: OwnedSemaphorePermit,
    _host: tokio::sync::OwnedMutexGuard<Option<Instant>>,
}

impl ResearchPolitenessGuard {
    pub fn new(config: PolitenessConfig, shared: Arc<dyn SharedLimitStore>) -> Self {
        let conc = config.max_global_concurrency.max(1);
        Self {
            global_sem: Arc::new(Semaphore::new(conc)),
            host_locks: Mutex::new(HashMap::new()),
            config,
            shared,
        }
    }

    pub fn config(&self) -> &PolitenessConfig {
        &self.config
    }

    pub async fn acquire_host(
        &self,
        host: &str,
        host_spacing_ms: u64,
        scope_key: &str,
        quota_limit: u64,
        window_secs: u64,
    ) -> Result<Result<HostPermit, FetchDecision>, String> {
        if self.shared.in_cooldown(&format!("cooldown:{host}")).await? {
            return Ok(Err(FetchDecision::RateLimitedCooldown {
                wait_secs: self.config.max_backoff_secs,
            }));
        }
        if !self
            .shared
            .check_and_increment(scope_key, quota_limit, window_secs)
            .await?
        {
            return Ok(Err(FetchDecision::RateLimitedCooldown {
                wait_secs: window_secs.min(self.config.max_backoff_secs),
            }));
        }

        let global_permit = self
            .global_sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("Global concurrency semaphore closed: {e}"))?;

        let host_mutex = {
            let mut map = self.host_locks.lock().await;
            map.entry(host.to_ascii_lowercase())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut host_guard = host_mutex.lock_owned().await;

        let spacing = Duration::from_millis(
            host_spacing_ms.max(self.config.default_host_spacing_ms),
        );
        if let Some(last) = *host_guard {
            let elapsed = last.elapsed();
            if elapsed < spacing {
                tokio::time::sleep(spacing - elapsed).await;
            }
        }
        *host_guard = Some(Instant::now());

        Ok(Ok(HostPermit {
            _global: global_permit,
            _host: host_guard,
        }))
    }

    /// Evaluates HTTP response status and headers:
    /// - 401/403 or challenge marker -> stops that path cleanly (no retry loops).
    /// - 429 -> sets bounded cooldown honoring Retry-After.
    pub async fn handle_status(
        &self,
        host: &str,
        status: u16,
        retry_after_header: Option<&str>,
        is_challenge: bool,
    ) -> Result<FetchDecision, String> {
        if status == 401 || status == 403 || is_challenge {
            self.shared
                .set_cooldown(&format!("cooldown:{host}"), self.config.max_backoff_secs)
                .await?;
            return Ok(FetchDecision::BlockedStop {
                reason: if is_challenge {
                    format!("{host}: challenge gate encountered; stopping cleanly")
                } else {
                    format!("{host}: HTTP {status}; stopping cleanly")
                },
            });
        }
        if status == 429 {
            let wait = parse_bounded_retry_after(retry_after_header, self.config.max_backoff_secs)
                .unwrap_or(5.min(self.config.max_backoff_secs));
            self.shared
                .set_cooldown(&format!("cooldown:{host}"), wait)
                .await?;
            return Ok(FetchDecision::RateLimitedCooldown { wait_secs: wait });
        }
        Ok(FetchDecision::Proceed)
    }
}
