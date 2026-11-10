//! Run only in an owner-approved MSI GPU window. Default tests skip this file.
use cortex_api::{
    core::config::Config,
    dal::remote::{
        cache::RedisCache,
        ollama::Ollama,
        runs::{RunDatabase, RunStore},
        usage::RedisLedger,
    },
    domain::{
        models::{cache::CacheKey, driver::*, identity::Caller, run_record::*},
        services::balancer::Scheduler,
    },
};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
#[tokio::test]
#[ignore = "requires an owner-approved MSI GPU window and Cortex test stores"]
async fn measure_8192_context_through_the_real_queue() {
    dotenvy::dotenv().ok();
    let config = Config::default();
    assert_eq!(config.ollama_url, "http://127.0.0.1:11434");
    assert_eq!(config.model_context_tokens, 8192);
    let actor = Uuid::new_v4().to_string();
    let caller = Caller {
        app_id: "cortex".into(),
        user_id: Some(actor.clone()),
        is_admin: false,
    };
    let queue_id = format!("test-model-{}", Uuid::new_v4().simple());
    let cache = RedisCache::from_environment();
    let store = Arc::new(RunStore::from_environment(RunDatabase::Test).unwrap());
    store.initialize().await.unwrap();
    let ledger = Arc::new(RedisLedger::new(
        cache.clone(),
        store.clone(),
        queue_id.clone(),
    ));
    let queue = Scheduler::new(ledger.clone());
    let driver = Ollama::new(config.clone(), queue.clone());
    let permit = queue.acquire(actor.clone(), Uuid::new_v4()).await.unwrap();
    driver.release_models_for_local_job(&permit).await.unwrap();
    drop(permit);
    let mut rows = vec![];
    let mut records = vec![];
    let mut keys = BTreeSet::new();
    for (name, vision) in [
        ("text cold", false),
        ("text warm", false),
        ("vision cold", true),
        ("vision warm", true),
        ("text after vision", false),
    ] {
        println!("Starting {name}");
        let output = OutputModel::new(&caller, Uuid::new_v4(), vision);
        let (events, _) = mpsc::unbounded_channel();
        let (_cancel, rx) = watch::channel(false);
        let input = CallInput {
            role: "answer".into(),
            system: "Answer the request briefly.".into(),
            prompt: if vision {
                "Describe the image in one short sentence.".into()
            } else {
                "Write the numbers from 1 to 32, separated by spaces.".into()
            },
            images: if vision {
                vec!["iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZ1cAAAAASUVORK5CYII=".into()]
            } else {
                vec![]
            },
            temperature: 0.0,
            max_tokens: 128,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(600),
            driver.call(&output, input, events, rx),
        )
        .await
        .expect("The model measurement timed out")
        .unwrap();
        assert!(result.error.is_none(), "The local model measurement failed");
        assert!(result.usage.output_tokens > 0);
        let speed = result
            .tokens_per_second
            .expect("The model did not return evaluation timing");
        println!(
            "{name}: load_ms={} ttft_ms={} total_ms={} tokens={} tokens_per_second={speed:.2}",
            result.load_ms,
            result.usage.time_to_first_token_ms.unwrap_or(0),
            result.usage.total_ms,
            result.usage.output_tokens
        );
        rows.push(format!(
            "| {name} | {} | {} | {} | {} | {} | {} | {speed:.2} |",
            result.usage.model,
            result.load_ms,
            result.usage.time_to_first_token_ms.unwrap_or(0),
            result.usage.total_ms,
            result.usage.input_tokens,
            result.usage.output_tokens
        ));
        let run = RunRecord {
            session_id: Uuid::now_v7(),
            output_id: output.output_id,
            app_id: caller.app_id.clone(),
            user_id: actor.clone(),
            created_at: chrono::Utc::now(),
            mode: RunMode {
                web_mode: WebMode::Fast,
                thinking: false,
            },
            total_ms: result.usage.total_ms,
            stages: vec![StageRecord {
                name: "Answer".into(),
                status: "ok".into(),
                error_number: None,
                notice_number: None,
                time_ms: result.usage.total_ms,
            }],
            calls: vec![result.usage],
            counts: RunCounts {
                pages_read: 0,
                chunks_used: 0,
                files: if vision { 1 } else { 0 },
                file_pages: 0,
                audio_seconds: 0.0,
            },
        };
        store.put(&run).await.unwrap();
        records.push(run.output_id);
        for key in ledger.bucket_keys(&actor, chrono::Utc::now()) {
            keys.insert(key.name());
        }
    }
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("ollama-8192-night.md"),format!("# MSI Ollama measurement\n\nUTC: {}\n\nContext: 8192. Temperature: 0. Maximum output: 128. All calls use the real Cortex queue, unique Valkey keys and cortex_runs_test. No model or quantization change. Token rate is Ollama eval_count divided by eval_duration. Model load and first-token times include driver load work; queue wait is excluded. The text model may use both GPU and CPU, as approved.\n\n| Case | Model | Load ms | First token ms | Total ms | Input tokens | Output tokens | Tokens/s |\n|---|---|---:|---:|---:|---:|---:|---:|\n{}\n",chrono::Utc::now().to_rfc3339(),rows.join("\n"))).unwrap();
    let permit = queue.acquire(actor.clone(), Uuid::new_v4()).await.unwrap();
    driver.release_models_for_local_job(&permit).await.unwrap();
    drop(permit);
    drop(driver);
    drop(queue);
    for id in records {
        store.remove(id).await.unwrap();
    }
    for key in ledger.bucket_keys(&actor, chrono::Utc::now()) {
        cache.remove(&key).await.unwrap();
    }
    cache
        .remove(&CacheKey::Loyalty(actor.clone()))
        .await
        .unwrap();
    cache.remove(&CacheKey::WaitQueue(queue_id)).await.unwrap();
    // Calls should finish inside one hour, but remove any earlier fixture buckets too.
    let url = std::env::var("CORTEX_REDIS_URL").unwrap();
    let client =
        redis::Client::open(url).unwrap_or_else(|_| panic!("The test cache address is not valid"));
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    for key in keys {
        let _: i64 = redis::cmd("DEL")
            .arg(key)
            .query_async(&mut connection)
            .await
            .unwrap();
    }
}
