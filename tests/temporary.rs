use cortex_api::{
    core::{config::Config, process_numbers},
    dal::remote::cache::RedisCache,
    domain::{
        models::{
            cache::CacheKey,
            chat_session::{NewChat, NextInput},
            context::ContextText,
            history::{FinishedOutput, FinishedStatus},
            identity::Caller,
            temporary::TemporaryFile,
        },
        services::temporary::TemporaryChats,
    },
};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;
fn fixture_caller() -> Caller {
    Caller {
        app_id: "cortex".into(),
        user_id: Some(format!("test-{}", Uuid::new_v4())),
        is_admin: false,
    }
}
fn body() -> NewChat {
    serde_json::from_value(json!({"app_id":"cortex","text":"first question","temporary":true}))
        .unwrap()
}
#[tokio::test]
async fn redis_down_refuses_start_file_and_continue() {
    let config = Config::default();
    assert_eq!(config.temporary_ttl, Duration::from_secs(3600));
    let chats = TemporaryChats::new(&config, RedisCache::new(None, Duration::from_millis(20)));
    let actor = fixture_caller();
    assert_eq!(
        chats
            .start(&actor, body(), chrono::Utc::now())
            .await
            .err()
            .unwrap()
            .number,
        process_numbers::TEMPORARY_UNAVAILABLE.number
    );
    assert_eq!(
        chats
            .append(
                &actor,
                Uuid::now_v7(),
                serde_json::from_value(json!({"text":"next"})).unwrap(),
                chrono::Utc::now()
            )
            .await
            .err()
            .unwrap()
            .number,
        process_numbers::TEMPORARY_UNAVAILABLE.number
    );
    let file = TemporaryFile {
        file_id: Uuid::nil(),
        name: "sample.txt".into(),
        pages: vec![],
    };
    assert_eq!(
        chats.save_file(&actor, file).await.err().unwrap().number,
        process_numbers::TEMPORARY_UNAVAILABLE.number
    );
    let mut permanent = body();
    permanent.temporary = false;
    assert_eq!(
        chats
            .start(&actor, permanent, chrono::Utc::now())
            .await
            .err()
            .unwrap()
            .number,
        process_numbers::INVALID_DATA.number
    );
}
#[tokio::test]
#[ignore = "requires approved Valkey; only unique cortex:temp UUID fixtures are written"]
async fn scoped_temporary_files_inputs_outputs_ttl_expiry_and_concurrent_updates() {
    dotenvy::dotenv().ok();
    let config = Config::default();
    let cache = RedisCache::from_environment();
    let chats = TemporaryChats::new(&config, cache.clone());
    let actor = fixture_caller();
    let file_id = chats
        .save_file(
            &actor,
            TemporaryFile {
                file_id: Uuid::nil(),
                name: "sample.txt".into(),
                pages: vec![ContextText {
                    label: "sample.txt".into(),
                    source: None,
                    text: "extracted text only".into(),
                }],
            },
        )
        .await
        .unwrap();
    let mut first = body();
    first.file_ids = vec![file_id.to_string()];
    let first = chats
        .start(&actor, first, chrono::Utc::now())
        .await
        .unwrap();
    let id = first.session_id;
    let key = CacheKey::TemporaryChat(id.to_string());
    let file_key = CacheKey::TemporaryChat(file_id.to_string());
    let client = redis::Client::open(std::env::var("CORTEX_REDIS_URL").unwrap())
        .unwrap_or_else(|_| panic!("test Valkey address is not valid"));
    let mut audit = client.get_multiplexed_async_connection().await.unwrap();
    let ttl: i64 = redis::cmd("PTTL")
        .arg(key.name())
        .query_async(&mut audit)
        .await
        .unwrap();
    assert!((3_590_000..=3_600_000).contains(&ttl));
    let raw = cache.get(&key).await.unwrap().unwrap();
    assert_eq!(raw["app_id"], "cortex");
    assert_eq!(
        raw["chat"]["files"][0]["pages"][0]["text"],
        "extracted text only"
    );
    assert!(raw["chat"]["outputs"][0]["output"]["memory_id"].is_null());
    let other = Caller {
        is_admin: true,
        ..fixture_caller()
    };
    assert_eq!(
        chats.get(&other, id).await.err().unwrap().number,
        process_numbers::CHAT_NOT_FOUND.number
    );
    assert!(chats.file(&other, file_id).await.is_err());
    let other_app = Caller {
        app_id: "reports".into(),
        ..actor.clone()
    };
    assert!(chats.get(&other_app, id).await.is_err());
    let done = FinishedOutput {
        session_id: id,
        output_id: first.outputs[0].output.output_id,
        status: FinishedStatus::Done,
        text: "answer".into(),
        thinking_text: None,
        memory_id: None,
        sources: vec![],
        memories: vec![],
        finished_at: chrono::Utc::now(),
    };
    let _: i64 = redis::cmd("PEXPIRE")
        .arg(key.name())
        .arg(2000)
        .query_async(&mut audit)
        .await
        .unwrap();
    chats.finish(&actor, &done).await.unwrap();
    let finish_ttl: i64 = redis::cmd("PTTL")
        .arg(key.name())
        .query_async(&mut audit)
        .await
        .unwrap();
    assert!(finish_ttl > 0 && finish_ttl <= 2000);
    assert!(chats.finish(&actor, &done).await.is_err());
    let next = || {
        serde_json::from_value::<NextInput>(
            json!({"text":"follow up","file_ids":[file_id.to_string()]}),
        )
        .unwrap()
    };
    let now = chrono::Utc::now();
    let (a, b) = tokio::join!(
        chats.append(&actor, id, next(), now),
        chats.append(&actor, id, next(), now)
    );
    assert!(a.is_ok() && b.is_ok());
    let state = chats.get(&actor, id).await.unwrap();
    assert_eq!(state.inputs.len(), 3);
    assert_eq!(state.outputs.len(), 3);
    assert_eq!(state.files.len(), 1);
    assert_eq!(state.outputs[0].output.status, "done");
    let refreshed: i64 = redis::cmd("PTTL")
        .arg(key.name())
        .query_async(&mut audit)
        .await
        .unwrap();
    assert!((3_590_000..=3_600_000).contains(&refreshed));
    let _: i64 = redis::cmd("PEXPIRE")
        .arg(key.name())
        .arg(25)
        .query_async(&mut audit)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        chats.get(&actor, id).await.err().unwrap().number,
        process_numbers::CHAT_NOT_FOUND.number
    );
    assert!(chats.append(&actor, id, next(), now).await.is_err());
    cache.remove(&key).await.unwrap();
    cache.remove(&file_key).await.unwrap();
}
