use cortex_api::{
    dal::remote::cache::RedisCache,
    domain::models::cache::{CacheError, CacheKey, Lifetime},
};
use serde_json::json;
use std::time::Duration;
fn keys(id: &str) -> Vec<CacheKey> {
    vec![
        CacheKey::History(id.into()),
        CacheKey::TemporaryChat(id.into()),
        CacheKey::Events(id.into()),
        CacheKey::Usage {
            user: id.into(),
            hour: "fixture".into(),
        },
        CacheKey::Timing(format!("fixture-{id}")),
    ]
}
#[test]
fn all_key_names_and_lifetimes_are_bounded() {
    for key in keys("fixture") {
        assert!(key.name().starts_with("cortex:"));
    }
    assert_eq!(
        Lifetime::new(Duration::ZERO).unwrap_err(),
        CacheError::InvalidLifetime
    );
    assert_eq!(
        Lifetime::new(Duration::from_nanos(1)).unwrap_err(),
        CacheError::InvalidLifetime
    );
    assert!(Lifetime::new(Duration::from_secs(u64::MAX)).is_err());
    assert_eq!(
        Lifetime::new(Duration::from_millis(1))
            .unwrap()
            .milliseconds(),
        1
    );
}
#[tokio::test]
async fn missing_and_down_store_return_unavailable_for_every_key_kind() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    for cache in [
        RedisCache::new(None, Duration::from_millis(100)),
        RedisCache::new(
            Some(&format!("redis://{address}/0")),
            Duration::from_millis(100),
        ),
    ] {
        for key in keys("fixture") {
            assert_eq!(
                cache
                    .put(
                        &key,
                        &json!({"fixture":true}),
                        Lifetime::new(Duration::from_secs(1)).unwrap()
                    )
                    .await,
                Err(CacheError::Unavailable)
            );
            assert_eq!(cache.get(&key).await, Err(CacheError::Unavailable));
            assert_eq!(cache.remove(&key).await, Err(CacheError::Unavailable));
        }
    }
}
#[tokio::test]
#[ignore = "requires the approved Valkey test namespace; writes only unique fixture keys"]
async fn real_valkey_audits_every_written_key_and_expiry() {
    dotenvy::dotenv().ok();
    let url = std::env::var("CORTEX_REDIS_URL").unwrap();
    let cache = RedisCache::from_environment();
    let client =
        redis::Client::open(url).unwrap_or_else(|_| panic!("The test Redis address is not valid"));
    let mut audit = client.get_multiplexed_async_connection().await.unwrap();
    let id = format!("test-{}", uuid::Uuid::new_v4().simple());
    let mut written = vec![];
    for key in keys(&id) {
        cache
            .put(
                &key,
                &json!({"fixture":true}),
                Lifetime::new(Duration::from_secs(30)).unwrap(),
            )
            .await
            .unwrap();
        written.push(key);
    }
    for key in &written {
        assert!(key.name().starts_with("cortex:"));
        let ttl: i64 = redis::cmd("PTTL")
            .arg(key.name())
            .query_async(&mut audit)
            .await
            .unwrap();
        assert!(ttl > 0 && ttl <= 30000);
        assert_eq!(cache.get(key).await.unwrap(), Some(json!({"fixture":true})));
        cache.remove(key).await.unwrap();
        assert_eq!(cache.get(key).await.unwrap(), None);
    }
    let key = CacheKey::History(format!("{id}-expires"));
    cache
        .put(
            &key,
            &json!(1),
            Lifetime::new(Duration::from_millis(30)).unwrap(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(70)).await;
    assert_eq!(cache.get(&key).await.unwrap(), None);
}

#[tokio::test]
async fn stalled_store_has_a_bounded_wait() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let cache = RedisCache::new(
        Some(&format!("redis://{address}")),
        Duration::from_millis(50),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        cache.get(&CacheKey::History("fixture".into())),
    )
    .await
    .unwrap();
    assert_eq!(result, Err(CacheError::Unavailable));
    task.abort();
}
