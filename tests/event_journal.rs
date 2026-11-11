use cortex_api::{
    core::config::Config,
    dal::remote::{cache::RedisCache, event_journal::EventJournal},
    domain::models::{
        cache::{CacheError, CacheKey},
        events::{NativeEvent, OutputEvent},
        replay::StoredEvent,
    },
};
use std::time::Duration;
use uuid::Uuid;
fn event(id: u64) -> StoredEvent {
    StoredEvent::from(&OutputEvent {
        id,
        event: NativeEvent::Token(format!("piece{id}")),
    })
}
#[tokio::test]
async fn absent_store_returns_unavailable() {
    let journal = EventJournal::new(
        RedisCache::new(None, Duration::from_millis(100)),
        &Config::default(),
    )
    .unwrap();
    assert!(matches!(
        journal.append(Uuid::now_v7(), &event(1)).await,
        Err(CacheError::Unavailable)
    ));
    assert!(matches!(
        journal.after(Uuid::now_v7(), 0).await,
        Err(CacheError::Unavailable)
    ));
}
#[tokio::test]
#[ignore = "requires the approved Valkey namespace; unique event fixture key only"]
async fn real_stream_order_after_paging_atomic_ttl_and_duplicate_rules() {
    dotenvy::dotenv().ok();
    let cache = RedisCache::from_environment();
    let config = Config {
        event_read_batch: 2,
        ..Default::default()
    };
    let journal = EventJournal::new(cache.clone(), &config).unwrap();
    let output = Uuid::now_v7();
    let key = CacheKey::Events(output.to_string());
    assert!(journal.after(output, 0).await.unwrap().is_none());
    assert!(journal.append(output, &event(2)).await.is_err());
    for id in 1..=5 {
        journal.append(output, &event(id)).await.unwrap();
    }
    journal.append(output, &event(5)).await.unwrap();
    let mut changed = event(5);
    changed.body["message"] = "changed".into();
    assert!(journal.append(output, &changed).await.is_err());
    assert!(journal.append(output, &event(7)).await.is_err());
    let first = journal.after(output, 0).await.unwrap().unwrap();
    assert_eq!(first.iter().map(|v| v.id).collect::<Vec<_>>(), [1, 2]);
    let next = journal.after(output, 2).await.unwrap().unwrap();
    assert_eq!(next.iter().map(|v| v.id).collect::<Vec<_>>(), [3, 4]);
    let last = journal.after(output, 4).await.unwrap().unwrap();
    assert_eq!(last[0].id, 5);
    assert_eq!(last[0].body["data"]["text"], "piece5");
    assert!(journal.after(output, 5).await.unwrap().unwrap().is_empty());
    let client = redis::Client::open(std::env::var("CORTEX_REDIS_URL").unwrap())
        .unwrap_or_else(|_| panic!("Test Redis URL invalid"));
    let mut audit = client
        .get_multiplexed_async_connection()
        .await
        .unwrap_or_else(|_| panic!("Test Redis unavailable"));
    let ttl: i64 = redis::cmd("PTTL")
        .arg(key.name())
        .query_async(&mut audit)
        .await
        .unwrap_or_else(|_| panic!("TTL audit failed"));
    assert!(ttl > 0 && ttl <= 3600000);
    assert!(key.name().starts_with("cortex:"));
    cache.remove(&key).await.unwrap();
    assert!(journal.after(output, 0).await.unwrap().is_none());
    let short = EventJournal::new(
        cache.clone(),
        &Config {
            event_retention: Duration::from_millis(30),
            ..Default::default()
        },
    )
    .unwrap();
    short.append(output, &event(1)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(short.after(output, 0).await.unwrap().is_none());
    cache.remove(&key).await.unwrap();
}
