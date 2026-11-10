use crate::{
    dal::remote::cache::RedisCache,
    domain::models::{
        cache::{CacheError, CacheKey, Lifetime},
        temporary::TemporaryData,
    },
};
use uuid::Uuid;
pub struct Snapshot {
    pub data: TemporaryData,
    raw: Vec<u8>,
}
#[derive(Clone)]
pub struct TemporaryStore {
    cache: RedisCache,
}
impl TemporaryStore {
    pub fn new(cache: RedisCache) -> Self {
        Self { cache }
    }
    pub async fn get(&self, id: Uuid) -> Result<Option<Snapshot>, CacheError> {
        let raw: Option<Vec<u8>> = self
            .cache
            .execute(redis::cmd("GET").arg(CacheKey::TemporaryChat(id.to_string()).name()))
            .await?;
        raw.map(|raw| {
            Ok(Snapshot {
                data: serde_json::from_slice(&raw).map_err(|_| CacheError::Unavailable)?,
                raw,
            })
        })
        .transpose()
    }
    pub async fn create(
        &self,
        id: Uuid,
        data: &TemporaryData,
        lifetime: Lifetime,
    ) -> Result<bool, CacheError> {
        let result: Option<String> = self
            .cache
            .execute(
                redis::cmd("SET")
                    .arg(CacheKey::TemporaryChat(id.to_string()).name())
                    .arg(serde_json::to_vec(data).map_err(|_| CacheError::Unavailable)?)
                    .arg("PX")
                    .arg(lifetime.milliseconds())
                    .arg("NX"),
            )
            .await?;
        Ok(result.is_some())
    }
    /// A completion preserves the last input's TTL. An accepted input renews it.
    pub async fn replace(
        &self,
        id: Uuid,
        previous: &Snapshot,
        data: &TemporaryData,
        lifetime: Option<Lifetime>,
    ) -> Result<bool, CacheError> {
        let script = "if redis.call('GET',KEYS[1])~=ARGV[1] then return 0 end; if ARGV[3]=='keep' then redis.call('SET',KEYS[1],ARGV[2],'KEEPTTL') else redis.call('SET',KEYS[1],ARGV[2],'PX',ARGV[3]) end; return 1";
        let result: i64 = self
            .cache
            .execute(
                redis::cmd("EVAL")
                    .arg(script)
                    .arg(1)
                    .arg(CacheKey::TemporaryChat(id.to_string()).name())
                    .arg(&previous.raw)
                    .arg(serde_json::to_vec(data).map_err(|_| CacheError::Unavailable)?)
                    .arg(
                        lifetime
                            .map(|l| l.milliseconds().to_string())
                            .unwrap_or_else(|| "keep".into()),
                    ),
            )
            .await?;
        Ok(result == 1)
    }
}
