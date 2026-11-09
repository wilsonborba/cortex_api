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
    async fn execute<T: FromRedisValue>(&self, command: &redis::Cmd) -> Result<T, CacheError> {
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
