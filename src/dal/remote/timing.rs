use crate::{
    core::config::Config,
    dal::remote::cache::RedisCache,
    domain::models::{
        cache::{CacheError, CacheKey, Lifetime},
        run_record::{RunMode, WebMode},
    },
};
use serde::Deserialize;
use uuid::Uuid;
#[derive(Clone)]
pub struct TimingStore {
    cache: RedisCache,
    config: Config,
}
pub fn kind(mode: &RunMode) -> String {
    format!(
        "{}:{}",
        match mode.web_mode {
            WebMode::Fast => "fast",
            WebMode::Medium => "medium",
            WebMode::Deep => "deep",
        },
        mode.thinking
    )
}
#[derive(Deserialize)]
struct Sample {
    id: Uuid,
    time: u64,
}
impl TimingStore {
    pub fn new(config: &Config, cache: RedisCache) -> Self {
        Self {
            cache,
            config: config.clone(),
        }
    }
    pub async fn completed(
        &self,
        mode: &RunMode,
        output_id: Uuid,
        total_ms: u64,
    ) -> Result<(), CacheError> {
        let key = CacheKey::Timing(kind(mode));
        let ttl = Lifetime::new(self.config.timing_cache_ttl)?;
        let script = "local raw=redis.call('GET',KEYS[1]); local v={}; if raw then v=cjson.decode(raw) end; for _,s in ipairs(v) do if s.id==ARGV[1] then return 0 end end; table.insert(v,{id=ARGV[1],time=tonumber(ARGV[2])}); while #v>tonumber(ARGV[3]) do table.remove(v,1) end; redis.call('SET',KEYS[1],cjson.encode(v),'PX',ARGV[4]); return 1";
        let _: i64 = self
            .cache
            .execute(
                redis::cmd("EVAL")
                    .arg(script)
                    .arg(1)
                    .arg(key.name())
                    .arg(output_id.to_string())
                    .arg(total_ms)
                    .arg(self.config.timing_sample_count.max(1))
                    .arg(ttl.milliseconds()),
            )
            .await?;
        Ok(())
    }
    pub async fn expected(&self, mode: &RunMode) -> Result<Option<u64>, CacheError> {
        let Some(value) = self.cache.get(&CacheKey::Timing(kind(mode))).await? else {
            return Ok(None);
        };
        let samples: Vec<Sample> =
            serde_json::from_value(value).map_err(|_| CacheError::Unavailable)?;
        if samples.is_empty() || samples.len() > self.config.timing_sample_count {
            return Err(CacheError::Unavailable);
        }
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u128;
        for sample in &samples {
            if !seen.insert(sample.id) {
                return Err(CacheError::Unavailable);
            }
            total += u128::from(sample.time);
        }
        Ok(Some((total / samples.len() as u128) as u64))
    }
}
