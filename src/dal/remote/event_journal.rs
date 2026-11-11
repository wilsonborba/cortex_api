use crate::{
    core::config::Config,
    dal::remote::cache::RedisCache,
    domain::models::{
        cache::{CacheError, CacheKey, Lifetime},
        replay::StoredEvent,
    },
};
use uuid::Uuid;
#[derive(Clone)]
pub struct EventJournal {
    cache: RedisCache,
    lifetime: Lifetime,
    batch: usize,
}
impl EventJournal {
    pub fn new(cache: RedisCache, config: &Config) -> Result<Self, CacheError> {
        Ok(Self {
            cache,
            lifetime: Lifetime::new(config.event_retention)?,
            batch: config.event_read_batch.max(1),
        })
    }
    /// Append and expiry are atomic. A repeated identical event is safe after
    /// a lost acknowledgement. Gaps and changed duplicates reject replay.
    pub async fn append(&self, output: Uuid, event: &StoredEvent) -> Result<(), CacheError> {
        if !event.valid() {
            return Err(CacheError::Unavailable);
        }
        let key = CacheKey::Events(output.to_string());
        let payload = serde_json::to_vec(event).map_err(|_| CacheError::Unavailable)?;
        let script = "local rows=redis.call('XREVRANGE',KEYS[1],'+','-','COUNT',1); if #rows==0 then if ARGV[1]~='1-0' then return -1 end else local last=rows[1]; if last[1]==ARGV[1] then if last[2][2]~=ARGV[2] then return -1 end; redis.call('PEXPIRE',KEYS[1],ARGV[4]); return 1 end; if last[1]~=ARGV[3] then return -1 end end; redis.call('XADD',KEYS[1],ARGV[1],'event',ARGV[2]); redis.call('PEXPIRE',KEYS[1],ARGV[4]); return 1";
        let result: i64 = self
            .cache
            .execute(
                redis::cmd("EVAL")
                    .arg(script)
                    .arg(1)
                    .arg(key.name())
                    .arg(format!("{}-0", event.id))
                    .arg(payload)
                    .arg(format!("{}-0", event.id - 1))
                    .arg(self.lifetime.milliseconds()),
            )
            .await?;
        if result == 1 {
            Ok(())
        } else {
            Err(CacheError::Unavailable)
        }
    }
    /// One bounded page, strictly after the explicit event number. None means
    /// the stream has expired or does not exist. No Last-Event-ID header.
    pub async fn after(
        &self,
        output: Uuid,
        after: u64,
    ) -> Result<Option<Vec<StoredEvent>>, CacheError> {
        let key = CacheKey::Events(output.to_string());
        let exists: bool = self
            .cache
            .execute(redis::cmd("EXISTS").arg(key.name()))
            .await?;
        if !exists {
            return Ok(None);
        }
        let reply: redis::streams::StreamRangeReply = self
            .cache
            .execute(
                redis::cmd("XRANGE")
                    .arg(key.name())
                    .arg(format!("({after}-0"))
                    .arg("+")
                    .arg("COUNT")
                    .arg(self.batch),
            )
            .await?;
        let mut events = vec![];
        let mut expected = after;
        for row in reply.ids {
            let raw = row.map.get("event").ok_or(CacheError::Unavailable)?;
            let bytes: Vec<u8> =
                redis::from_redis_value(raw).map_err(|_| CacheError::Unavailable)?;
            let event: StoredEvent =
                serde_json::from_slice(&bytes).map_err(|_| CacheError::Unavailable)?;
            expected = expected.checked_add(1).ok_or(CacheError::Unavailable)?;
            if !event.valid() || event.id != expected || row.id != format!("{}-0", event.id) {
                return Err(CacheError::Unavailable);
            }
            events.push(event);
        }
        Ok(Some(events))
    }
}
