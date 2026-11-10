use chrono::{Duration, Utc};
use cortex_api::{
    dal::remote::{
        cache::RedisCache,
        runs::{RunDatabase, RunStore},
        usage::RedisLedger,
    },
    domain::models::{cache::CacheKey, run_record::*, usage::*},
};
use std::sync::Arc;
use uuid::Uuid;
fn call(now: chrono::DateTime<Utc>, input: u64, output: u64, images: u32, audio: u32) -> ModelCall {
    ModelCall {
        images: Some(images),
        audio: Some(audio),
        role: "answer".into(),
        model: "fixture".into(),
        input_tokens: input,
        output_tokens: output,
        time_to_first_token_ms: Some(1),
        total_ms: 1,
        started_at: now,
        finished_at: now,
    }
}
fn record(actor: &str, now: chrono::DateTime<Utc>, calls: Vec<ModelCall>) -> RunRecord {
    RunRecord {
        session_id: Uuid::now_v7(),
        output_id: Uuid::new_v4(),
        app_id: "fixture".into(),
        user_id: actor.into(),
        created_at: now,
        mode: RunMode {
            web_mode: WebMode::Fast,
            thinking: false,
        },
        total_ms: 1,
        stages: vec![],
        calls,
        counts: RunCounts {
            pages_read: 0,
            chunks_used: 0,
            files: 0,
            file_pages: 0,
            audio_seconds: 0.0,
        },
    }
}
async fn audit(cache: &RedisCache, keys: &[CacheKey]) {
    let url = std::env::var("CORTEX_REDIS_URL").unwrap();
    let client =
        redis::Client::open(url).unwrap_or_else(|_| panic!("The test cache address is not valid"));
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    for key in keys {
        assert!(key.name().starts_with("cortex:"));
        let ttl: i64 = redis::cmd("PTTL")
            .arg(key.name())
            .query_async(&mut connection)
            .await
            .unwrap();
        assert!(ttl > 0, "Every written fixture key needs a TTL");
        cache.get(key).await.unwrap();
    }
}
#[tokio::test]
#[ignore = "requires the Cortex Valkey namespace and cortex_runs_test"]
async fn loss_rebuilds_saved_and_pending_calls_without_duplicates() {
    dotenvy::dotenv().ok();
    let now = chrono::DateTime::from_timestamp(Utc::now().timestamp().div_euclid(3600) * 3600, 0)
        .unwrap();
    let actor = format!("test-{}", Uuid::new_v4().simple());
    let queue = format!("test-{}", Uuid::new_v4().simple());
    let cache = RedisCache::from_environment();
    let source = Arc::new(RunStore::from_environment(RunDatabase::Test).unwrap());
    source.initialize().await.unwrap();
    let saved = record(
        &actor,
        now,
        vec![
            call(now, 10, 2, 1, 0),
            call(now - Duration::hours(2), 20, 3, 0, 1),
            call(now - Duration::hours(25), 9999, 9999, 0, 0),
        ],
    );
    source.put(&saved).await.unwrap();
    let other = record("other-fixture-user", now, vec![call(now, 1000, 1000, 0, 0)]);
    source.put(&other).await.unwrap();
    let ledger = RedisLedger::new(cache.clone(), source.clone(), queue.clone());
    let initial = ledger.share(&actor, now).await.unwrap();
    assert_eq!(
        initial.usage,
        Usage {
            input: 30,
            output: 5,
            images: 1,
            audio: 1
        }
    );
    assert_eq!(initial.weight, 1.0);
    let mut pending = record(&actor, now, vec![call(now, 7, 2, 0, 1)]);
    ledger
        .record(CallUsage {
            actor: actor.clone(),
            output_id: pending.output_id,
            finished_at: now,
            usage: Usage {
                input: 7,
                output: 2,
                images: 0,
                audio: 1,
            },
        })
        .await
        .unwrap();
    assert_eq!(ledger.share(&actor, now).await.unwrap().usage.total(), 44);
    let keys = ledger.bucket_keys(&actor, now);
    for key in &keys {
        cache.remove(key).await.unwrap();
    }
    let rebuilt = ledger.share(&actor, now).await.unwrap();
    assert_eq!(
        rebuilt.usage,
        Usage {
            input: 37,
            output: 7,
            images: 1,
            audio: 2
        }
    );
    source.put(&pending).await.unwrap();
    for key in &keys {
        cache.remove(key).await.unwrap();
    }
    assert_eq!(ledger.share(&actor, now).await.unwrap().usage.total(), 44);
    // A new call after partial Redis loss must not hide saved counts in that hour.
    cache.remove(keys.last().unwrap()).await.unwrap();
    pending.output_id = Uuid::new_v4();
    ledger
        .record(CallUsage {
            actor: actor.clone(),
            output_id: pending.output_id,
            finished_at: now,
            usage: Usage {
                input: 5,
                output: 0,
                images: 0,
                audio: 0,
            },
        })
        .await
        .unwrap();
    assert_eq!(ledger.share(&actor, now).await.unwrap().usage.total(), 49);
    ledger
        .waiting(vec![WaitingCall {
            ticket: 1,
            actor: actor.clone(),
        }])
        .await
        .unwrap();
    let mut all_keys = keys;
    all_keys.push(CacheKey::Loyalty(actor.clone()));
    all_keys.push(CacheKey::WaitQueue(queue));
    audit(&cache, &all_keys).await;
    for key in all_keys {
        cache.remove(&key).await.unwrap();
    }
    source.remove(saved.output_id).await.unwrap();
    source.remove(other.output_id).await.unwrap();
    // The earlier pending record was saved before its local output id changed.
    let found = source
        .records_since(now - Duration::seconds(1))
        .await
        .unwrap();
    for r in found.into_iter().filter(|r| r.user_id == actor) {
        source.remove(r.output_id).await.unwrap();
    }
    let paged_actor = format!("test-pages-{}", Uuid::new_v4().simple());
    let mut outputs = vec![];
    for _ in 0..201 {
        let run = record(&paged_actor, now, vec![call(now, 1, 1, 0, 0)]);
        source.put(&run).await.unwrap();
        outputs.push(run.output_id);
    }
    let page_ledger = RedisLedger::new(
        cache.clone(),
        source.clone(),
        format!("unused-{}", Uuid::new_v4().simple()),
    );
    assert_eq!(
        page_ledger
            .share(&paged_actor, now)
            .await
            .unwrap()
            .usage
            .total(),
        402
    );
    let mut page_keys = page_ledger.bucket_keys(&paged_actor, now);
    page_keys.push(CacheKey::Loyalty(paged_actor));
    audit(&cache, &page_keys).await;
    for key in page_keys {
        cache.remove(&key).await.unwrap();
    }
    for id in outputs {
        source.remove(id).await.unwrap();
    }
}
