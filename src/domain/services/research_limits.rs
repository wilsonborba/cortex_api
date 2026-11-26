use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::time::Instant;

use crate::core::config::Config;
use crate::dal::remote::cache::RedisCache;
use crate::domain::models::cache::{CacheKey, Lifetime};
use crate::domain::models::web_page::{PageConditionalMeta, WebPage};

/// Shared rate-limit and cooldown store at provider/account scope (backed by Valkey in prod,
/// in-memory test double only in isolated unit tests).
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

/// Production Valkey-backed shared rate-limit and cooldown store across users and instances.
#[derive(Clone)]
pub struct ValkeyLimitStore {
    cache: RedisCache,
}

impl ValkeyLimitStore {
    pub fn new(cache: RedisCache) -> Self {
        Self { cache }
    }
}

impl SharedLimitStore for ValkeyLimitStore {
    fn check_and_increment(
        &self,
        scope_key: &str,
        limit: u64,
        window_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        let key = CacheKey::ResearchLimit(scope_key.to_string());
        let win = window_secs.max(1);
        Box::pin(async move {
            let script = "local current=tonumber(redis.call('GET',KEYS[1]) or '0'); if current>=tonumber(ARGV[1]) then return 0 end; local next=redis.call('INCR',KEYS[1]); if next==1 then redis.call('EXPIRE',KEYS[1],tonumber(ARGV[2])) end; return 1";
            let allowed: i64 = self
                .cache
                .execute(
                    redis::cmd("EVAL")
                        .arg(script)
                        .arg(1)
                        .arg(key.name())
                        .arg(limit)
                        .arg(win),
                )
                .await
                .map_err(|_| "Valkey limit store unavailable".to_string())?;
            Ok(allowed == 1)
        })
    }

    fn set_cooldown(
        &self,
        scope_key: &str,
        ttl_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let key = CacheKey::ResearchCooldown(scope_key.to_string());
        let ttl = ttl_secs.max(1);
        Box::pin(async move {
            let _: String = self
                .cache
                .execute(
                    redis::cmd("SET")
                        .arg(key.name())
                        .arg("1")
                        .arg("EX")
                        .arg(ttl),
                )
                .await
                .map_err(|_| "Valkey cooldown store unavailable".to_string())?;
            Ok(())
        })
    }

    fn in_cooldown(
        &self,
        scope_key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        let key = CacheKey::ResearchCooldown(scope_key.to_string());
        Box::pin(async move {
            let exists: bool = self
                .cache
                .execute(redis::cmd("EXISTS").arg(key.name()))
                .await
                .map_err(|_| "Valkey cooldown store unavailable".to_string())?;
            Ok(exists)
        })
    }
}

/// In-memory test double for isolated unit tests without Valkey.
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
    pub provider_quota_per_minute: u64,
}

impl Default for PolitenessConfig {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl PolitenessConfig {
    pub fn from_config(config: &Config) -> Self {
        Self {
            max_global_concurrency: config.research_max_global_concurrency.max(1),
            default_host_spacing_ms: config.research_host_spacing.as_millis() as u64,
            request_timeout_ms: config.web_page_timeout.as_millis() as u64,
            max_response_bytes: config.web_html_bytes,
            max_backoff_secs: config.research_max_backoff.as_secs().max(1),
            provider_quota_per_minute: config.research_provider_quota_per_minute.max(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchDecision {
    Proceed,
    BlockedStop { reason: String },
    RateLimitedCooldown { wait_secs: u64 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CoverageReport {
    #[default]
    Complete,
    Partial {
        skipped_hosts: Vec<String>,
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

impl CoverageReport {
    pub fn record_skipped(&mut self, host: &str, reason: impl Into<String>) {
        let reason = reason.into();
        let host = host.to_ascii_lowercase();
        match self {
            Self::Complete => {
                *self = Self::Partial {
                    skipped_hosts: vec![host],
                    reason,
                };
            }
            Self::Partial {
                skipped_hosts,
                reason: existing,
            } => {
                if !skipped_hosts.contains(&host) {
                    skipped_hosts.push(host);
                }
                if existing.is_empty() {
                    *existing = reason;
                }
            }
            Self::Unavailable { .. } => {}
        }
    }
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

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedWebPage {
    pub page: WebPage,
    pub meta: PageConditionalMeta,
}

/// Caches permitted web pages for conditional `If-None-Match` / `If-Modified-Since` revalidation.
#[derive(Clone)]
pub struct ResearchPageCache {
    redis: RedisCache,
    ttl: Duration,
    memory: Arc<RwLock<HashMap<String, CachedWebPage>>>,
}

impl ResearchPageCache {
    pub fn new(redis: RedisCache, ttl: Duration) -> Self {
        Self {
            redis,
            ttl,
            memory: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn in_memory(ttl: Duration) -> Self {
        Self::new(RedisCache::new(None, Duration::from_millis(50)), ttl)
    }

    fn url_digest(url: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in url.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    pub async fn get(&self, url: &str) -> Option<CachedWebPage> {
        if self.redis.is_configured() {
            let key = CacheKey::ResearchPageCache(Self::url_digest(url));
            if let Ok(Some(val)) = self.redis.get(&key).await
                && let Ok(cached) = serde_json::from_value::<CachedWebPage>(val)
            {
                return Some(cached);
            }
        }
        self.memory.read().await.get(url).cloned()
    }

    pub async fn put(&self, url: &str, cached: CachedWebPage) {
        if self.redis.is_configured()
            && let Ok(ttl) = Lifetime::new(self.ttl)
            && let Ok(val) = serde_json::to_value(&cached)
        {
            let key = CacheKey::ResearchPageCache(Self::url_digest(url));
            let _ = self.redis.put(&key, &val, ttl).await;
        }
        self.memory.write().await.insert(url.to_string(), cached);
    }
}

/// Parses `Retry-After` or `X-RateLimit-Reset` integer seconds, bounded by [max_secs].
pub fn parse_bounded_retry_after(header_val: Option<&str>, max_secs: u64) -> Option<u64> {
    let raw = header_val?.trim();
    let secs = raw.parse::<u64>().ok()?;
    Some(secs.min(max_secs))
}

/// Parses `Retry-After` or `X-RateLimit-Remaining` + `X-RateLimit-Reset` (seconds or epoch),
/// returning a bounded cooldown duration in seconds.
pub fn parse_rate_limit_wait(
    retry_after: Option<&str>,
    remaining: Option<&str>,
    reset_header: Option<&str>,
    max_secs: u64,
) -> Option<u64> {
    if let Some(secs) = parse_bounded_retry_after(retry_after, max_secs) {
        return Some(secs);
    }
    let is_exhausted = remaining
        .and_then(|r| r.trim().parse::<u64>().ok())
        .is_some_and(|n| n == 0);
    if !is_exhausted {
        return None;
    }
    let reset_raw = reset_header?.trim().parse::<u64>().ok()?;
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let wait = if reset_raw > 1_000_000_000 {
        reset_raw.saturating_sub(now_epoch)
    } else {
        reset_raw
    };
    Some(wait.max(1).min(max_secs))
}

/// Detects common anti-bot / CAPTCHA / interstitial challenge gates without trying to bypass them.
pub fn is_challenge_response(status: u16, body: &str) -> bool {
    if status == 401 || status == 403 {
        return false;
    }
    let low = body.to_ascii_lowercase();
    low.contains("cf-challenge")
        || low.contains("challenge-platform")
        || low.contains("attention required! | cloudflare")
        || low.contains("verify you are human")
        || low.contains("g-recaptcha")
        || low.contains("hcaptcha")
}

/// Checks `Cache-Control` response headers: never caches `no-store` or `private` responses.
pub fn is_cache_permitted(cache_control: Option<&str>) -> bool {
    let Some(cc) = cache_control else {
        return true;
    };
    let low = cc.to_ascii_lowercase();
    !low.contains("no-store") && !low.contains("private")
}

/// Publisher policy check combining robots permission and configured source access mode.
pub fn evaluate_publisher_policy(
    host: &str,
    access_mode: Option<&str>,
    robots_allowed: bool,
) -> FetchDecision {
    if !robots_allowed {
        return FetchDecision::BlockedStop {
            reason: format!("{host}: disallowed by robots.txt"),
        };
    }
    if matches!(access_mode, Some("unavailable")) {
        return FetchDecision::BlockedStop {
            reason: format!("{host}: publisher policy marks source unavailable"),
        };
    }
    FetchDecision::Proceed
}

/// Enforces one direct request in flight per host, configurable per-host spacing,
/// bounded global concurrency, shared provider/account Valkey quotas, and clean stop
/// on 401/403/challenge or 429 cooldown.
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

    pub fn from_config(config: &Config, shared: Arc<dyn SharedLimitStore>) -> Self {
        Self::new(PolitenessConfig::from_config(config), shared)
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
        let host_lc = host.to_ascii_lowercase();
        if self
            .shared
            .in_cooldown(&format!("cooldown:{host_lc}"))
            .await?
        {
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
            map.entry(host_lc)
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut host_guard = host_mutex.lock_owned().await;

        let spacing =
            Duration::from_millis(host_spacing_ms.max(self.config.default_host_spacing_ms));
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
    /// - 429 -> sets bounded cooldown honoring Retry-After / reset headers.
    pub async fn handle_status(
        &self,
        host: &str,
        status: u16,
        retry_after_header: Option<&str>,
        is_challenge: bool,
    ) -> Result<FetchDecision, String> {
        let host_lc = host.to_ascii_lowercase();
        if status == 401 || status == 403 || is_challenge {
            self.shared
                .set_cooldown(&format!("cooldown:{host_lc}"), self.config.max_backoff_secs)
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
                .set_cooldown(&format!("cooldown:{host_lc}"), wait)
                .await?;
            return Ok(FetchDecision::RateLimitedCooldown { wait_secs: wait });
        }
        Ok(FetchDecision::Proceed)
    }

    /// Records a bounded cooldown directly (e.g. from `X-RateLimit-Reset` when remaining hits 0).
    pub async fn set_host_cooldown(&self, host: &str, wait_secs: u64) -> Result<(), String> {
        let host_lc = host.to_ascii_lowercase();
        let bounded = wait_secs.max(1).min(self.config.max_backoff_secs);
        self.shared
            .set_cooldown(&format!("cooldown:{host_lc}"), bounded)
            .await
    }
}
