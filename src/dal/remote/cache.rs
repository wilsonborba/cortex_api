use crate::{
    core::config::Config,
    domain::models::cache::{CacheError, CacheKey, Lifetime},
};
use redis::{AsyncConnectionConfig, FromRedisValue, aio::MultiplexedConnection};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
#[derive(Clone)]
pub struct RedisCache {
    client: Option<redis::Client>,
    connection: Arc<Mutex<Option<MultiplexedConnection>>>,
    timeout: Duration,
}
impl RedisCache {
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var("CORTEX_REDIS_URL").ok().as_deref(),
            Config::default().cache_operation_timeout,
        )
    }
    pub fn new(url: Option<&str>, timeout: Duration) -> Self {
        Self {
            client: url.and_then(|url| redis::Client::open(url).ok()),
            connection: Default::default(),
            timeout,
        }
    }
    async fn connection(&self) -> Result<MultiplexedConnection, CacheError> {
        let mut saved = self.connection.lock().await;
        if let Some(connection) = saved.as_ref() {
            return Ok(connection.clone());
        }
        let config = AsyncConnectionConfig::new()
            .set_connection_timeout(self.timeout)
            .set_response_timeout(self.timeout);
        let connection = self
            .client
            .as_ref()
            .ok_or(CacheError::Unavailable)?
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .map_err(|_| CacheError::Unavailable)?;
        *saved = Some(connection.clone());
        Ok(connection)
    }
    pub(crate) async fn execute<T: FromRedisValue>(
        &self,
        command: &redis::Cmd,
    ) -> Result<T, CacheError> {
        let result = tokio::time::timeout(self.timeout, async {
            let mut connection = self.connection().await?;
            command
                .query_async(&mut connection)
                .await
                .map_err(|_| CacheError::Unavailable)
        })
        .await
        .unwrap_or(Err(CacheError::Unavailable));
        if result.is_err()
            && let Ok(mut saved) = self.connection.try_lock()
        {
            *saved = None;
        }
        result
    }
    pub async fn put(
        &self,
        key: &CacheKey,
        value: &Value,
        lifetime: Lifetime,
    ) -> Result<(), CacheError> {
        let mut command = redis::cmd("SET");
        command
            .arg(key.name())
            .arg(serde_json::to_vec(value).expect("JSON value"))
            .arg("PX")
            .arg(lifetime.milliseconds());
        self.execute(&command).await
    }
    pub async fn get(&self, key: &CacheKey) -> Result<Option<Value>, CacheError> {
        let value: Option<Vec<u8>> = self.execute(redis::cmd("GET").arg(key.name())).await?;
        value
            .map(|v| serde_json::from_slice(&v).map_err(|_| CacheError::Unavailable))
            .transpose()
    }
    pub async fn remove(&self, key: &CacheKey) -> Result<(), CacheError> {
        let _: i64 = self.execute(redis::cmd("DEL").arg(key.name())).await?;
        Ok(())
    }
}

impl RedisCache {
    pub async fn get_many(&self, keys: &[CacheKey]) -> Result<Vec<Option<Value>>, CacheError> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let values: Vec<Option<Vec<u8>>> = self
            .execute(redis::cmd("MGET").arg(keys.iter().map(CacheKey::name).collect::<Vec<_>>()))
            .await?;
        values
            .into_iter()
            .map(|v| {
                v.map(|v| serde_json::from_slice(&v).map_err(|_| CacheError::Unavailable))
                    .transpose()
            })
            .collect()
    }
    pub async fn add_usage(
        &self,
        actor: &str,
        hour: i64,
        usage: crate::domain::models::usage::Usage,
        lifetime: Lifetime,
    ) -> Result<(), CacheError> {
        let key = CacheKey::Usage {
            user: actor.into(),
            hour: hour.to_string(),
        };
        let script = "local raw=redis.call('GET',KEYS[1]); local v={input=0,output=0,images=0,audio=0}; if raw then v=cjson.decode(raw) end; v.input=v.input+tonumber(ARGV[2]); v.output=v.output+tonumber(ARGV[3]); v.images=v.images+tonumber(ARGV[4]); v.audio=v.audio+tonumber(ARGV[5]); redis.call('SET',KEYS[1],cjson.encode(v),'PX',ARGV[1]); return 1";
        let _: i64 = self
            .execute(
                redis::cmd("EVAL")
                    .arg(script)
                    .arg(1)
                    .arg(key.name())
                    .arg(lifetime.milliseconds())
                    .arg(usage.input)
                    .arg(usage.output)
                    .arg(usage.images)
                    .arg(usage.audio),
            )
            .await?;
        Ok(())
    }
}
