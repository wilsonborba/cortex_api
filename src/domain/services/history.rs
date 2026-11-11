use crate::{
    core::config::Config,
    dal::{local::chats::Chats, remote::cache::RedisCache},
    domain::models::{
        cache::{CacheError, CacheKey, Lifetime},
        history::*,
        storage::{AppUser, StorageError},
    },
};
use uuid::Uuid;
#[derive(Clone)]
pub struct History {
    repo: Chats,
    cache: RedisCache,
    limit: usize,
    lifetime: Lifetime,
}
impl History {
    pub fn new(repo: Chats, cache: RedisCache, config: &Config) -> Result<Self, CacheError> {
        Ok(Self {
            repo,
            cache,
            limit: config.history_cache_exchanges,
            lifetime: Lifetime::new(config.history_cache_ttl)?,
        })
    }
    pub async fn recent(
        &self,
        scope: &AppUser,
        session: Uuid,
    ) -> Result<Vec<HistoryExchange>, StorageError> {
        // Always check SQL ownership and the immutable-completion count first.
        // A slow old cache write cannot hide a more recent completed exchange.
        let count = self
            .repo
            .history_head(scope, session)
            .await?
            .ok_or(StorageError::NotFound)?;
        let key = CacheKey::History(session.to_string());
        if let Ok(Some(value)) = self.cache.get(&key).await
            && let Ok(cached) = serde_json::from_value::<CachedHistory>(value)
            && cached.session_id == session
            && cached.app_id == scope.app_id
            && cached.user_id == scope.user_id
            && cached.completed_count == count
            && cached.exchanges.len() <= self.limit
        {
            return Ok(cached.exchanges);
        }
        let exchanges = self.repo.recent_history(scope, session, self.limit).await?;
        let cached = CachedHistory {
            session_id: session,
            app_id: scope.app_id.clone(),
            user_id: scope.user_id.clone(),
            completed_count: count,
            exchanges: exchanges.clone(),
        };
        let _ = self
            .cache
            .put(
                &key,
                &serde_json::to_value(cached).expect("history serializes"),
                self.lifetime,
            )
            .await;
        Ok(exchanges)
    }
    pub async fn files(
        &self,
        scope: &AppUser,
        session: Uuid,
    ) -> Result<Vec<ChatFile>, StorageError> {
        self.repo
            .history_head(scope, session)
            .await?
            .ok_or(StorageError::NotFound)?;
        self.repo.all_chat_files(scope, session).await
    }
    pub async fn save(&self, scope: &AppUser, output: &FinishedOutput) -> Result<(), StorageError> {
        self.repo.finish_output(scope, output).await?;
        let _ = self
            .cache
            .remove(&CacheKey::History(output.session_id.to_string()))
            .await;
        // Cache faults never undo the permanent SQL answer.
        let _ = self.recent(scope, output.session_id).await;
        Ok(())
    }
}
