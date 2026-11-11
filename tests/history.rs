use chrono::{TimeZone, Utc};
use cortex_api::{
    core::config::Config,
    dal::{
        local::{
            chats::Chats,
            database::{self, Target},
        },
        remote::cache::RedisCache,
    },
    domain::{
        models::{
            cache::{CacheKey, Lifetime},
            chat_session::*,
            context::ContextInput,
            events::{Memory, Source},
            history::*,
            identity::Caller,
            storage::StorageError,
        },
        services::{
            chat_session::{continue_permanent, new_permanent},
            context::assemble,
            history::History,
        },
    },
};
use std::time::Duration;
use uuid::Uuid;
fn finished(prepared: &PreparedInput, status: FinishedStatus, text: String) -> FinishedOutput {
    FinishedOutput {
        session_id: prepared.input.session_id,
        output_id: prepared.output.output_id,
        status,
        text,
        thinking_text: Some("private thinking".into()),
        memory_id: Some("exchange-memory".into()),
        sources: vec![Source {
            title: "Source".into(),
            url: "https://private-source.example".into(),
        }],
        memories: vec![Memory {
            id: "memory-used".into(),
            title: "Memory".into(),
        }],
        finished_at: prepared.input.created_at + chrono::Duration::milliseconds(1),
    }
}
#[tokio::test]
#[ignore = "requires cortex_api_test and Valkey; unique temporary schema and UUID cache key"]
async fn save_history_cache_loss_scope_and_earlier_files() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_history_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let cache = RedisCache::from_environment();
    let config = Config::default();
    let history = History::new(repo.clone(), cache.clone(), &config).unwrap();
    let actor = Caller {
        app_id: "cortex".into(),
        user_id: Some("history-user".into()),
        is_admin: false,
    };
    let scope = actor.storage_scope();
    let now = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
    let file = SessionFile {
        file_id: "earlier-resource".into(),
        memory_id: "earlier-memory".into(),
        name: "earlier.txt".into(),
    };
    let body: NewChat = serde_json::from_value(
        serde_json::json!({"app_id":"cortex","text":"question0","file_ids":["earlier-resource"]}),
    )
    .unwrap();
    let first = new_permanent(&config, &actor, body, std::slice::from_ref(&file), now)
        .unwrap_or_else(|_| panic!("prepare failed"));
    repo.save_accepted(&first).await.unwrap();
    let session = first.input.session_id;
    let chat = first.new_chat.clone().unwrap();
    let key = CacheKey::History(session.to_string());
    // A running answer is never presented as a completed history exchange.
    assert!(history.recent(&scope, session).await.unwrap().is_empty());
    history
        .save(
            &scope,
            &finished(&first, FinishedStatus::Done, "answer0".into()),
        )
        .await
        .unwrap();
    let mut newest = first.clone();
    let mut old_snapshot = None;
    for index in 1..25 {
        let body:NextInput=serde_json::from_value(serde_json::json!({"text":format!("question{index}"),"file_ids":if index==1{vec!["earlier-resource"]}else{vec![]}})).unwrap();
        let files = if index == 1 {
            vec![file.clone()]
        } else {
            vec![]
        };
        let prepared = continue_permanent(
            &actor,
            &chat,
            body,
            &files,
            now + chrono::Duration::seconds(index),
        )
        .unwrap_or_else(|_| panic!("prepare failed"));
        repo.save_accepted(&prepared).await.unwrap();
        let answer = if index == 22 {
            "x".repeat(3000)
        } else {
            format!("answer{index}")
        };
        history
            .save(&scope, &finished(&prepared, FinishedStatus::Done, answer))
            .await
            .unwrap();
        if index == 23 {
            old_snapshot = cache.get(&key).await.unwrap();
        }
        newest = prepared;
    }
    let recent = history.recent(&scope, session).await.unwrap();
    assert_eq!(recent.len(), 20);
    assert_eq!(recent[0].input, "question24");
    assert_eq!(recent[19].input, "question5");
    let encoded = serde_json::to_string(&recent).unwrap();
    assert!(!encoded.contains("private thinking"));
    assert!(!encoded.contains("private-source"));
    let context = assemble(
        &config,
        ContextInput {
            question: "follow-up".into(),
            history: recent.iter().map(HistoryExchange::context).collect(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(context.history.len(), 2);
    assert_eq!(context.history[0].output, "answer24");
    assert_eq!(context.history[1].output, "answer23");
    let files = history.files(&scope, session).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_id, "earlier-resource");
    assert_eq!(files[0].memory_id, "earlier-memory");
    // A delayed old writer may finish after Save. Its old count forces a miss.
    cache
        .put(
            &key,
            &old_snapshot.unwrap(),
            Lifetime::new(Duration::from_secs(3600)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        history.recent(&scope, session).await.unwrap()[0].input,
        "question24"
    );
    let mut foreign = cache.get(&key).await.unwrap().unwrap();
    foreign["user_id"] = "other-user".into();
    cache
        .put(
            &key,
            &foreign,
            Lifetime::new(Duration::from_secs(3600)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        history.recent(&scope, session).await.unwrap()[0].input,
        "question24"
    );
    cache.remove(&key).await.unwrap();
    assert_eq!(
        history.recent(&scope, session).await.unwrap()[0].input,
        "question24"
    );
    let down = History::new(
        repo.clone(),
        RedisCache::new(None, Duration::from_millis(100)),
        &config,
    )
    .unwrap();
    assert_eq!(
        down.recent(&scope, session).await.unwrap()[0].input,
        "question24"
    );
    for status in [FinishedStatus::Failed, FinishedStatus::Cancelled] {
        let body: NextInput =
            serde_json::from_value(serde_json::json!({"text":"partial input"})).unwrap();
        let prepared = continue_permanent(
            &actor,
            &chat,
            body,
            &[],
            now + chrono::Duration::seconds(30),
        )
        .unwrap_or_else(|_| panic!("prepare failed"));
        repo.save_accepted(&prepared).await.unwrap();
        down.save(&scope, &finished(&prepared, status, "partial text".into()))
            .await
            .unwrap();
        let saved = repo
            .output(&scope, prepared.output.output_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, status.name());
        assert_eq!(saved.text, "partial text");
        assert_eq!(
            history.recent(&scope, session).await.unwrap()[0].input,
            "question24"
        );
    }
    for wrong in [
        cortex_api::domain::models::storage::AppUser {
            app_id: "cortex".into(),
            user_id: "other-user".into(),
        },
        cortex_api::domain::models::storage::AppUser {
            app_id: "reports".into(),
            user_id: scope.user_id.clone(),
        },
    ] {
        assert!(matches!(
            history.recent(&wrong, session).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            history.files(&wrong, session).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            history
                .save(
                    &wrong,
                    &finished(&newest, FinishedStatus::Done, "bad".into())
                )
                .await,
            Err(StorageError::NotFound)
        ));
    }
    let client = redis::Client::open(std::env::var("CORTEX_REDIS_URL").unwrap())
        .unwrap_or_else(|_| panic!("Test Redis URL invalid"));
    let mut audit = client
        .get_multiplexed_async_connection()
        .await
        .unwrap_or_else(|_| panic!("Redis unavailable"));
    let ttl: i64 = redis::cmd("PTTL")
        .arg(key.name())
        .query_async(&mut audit)
        .await
        .unwrap_or_else(|_| panic!("TTL audit failed"));
    assert!(ttl > 0 && ttl <= 3600000);
    assert!(key.name().starts_with("cortex:"));
    assert_eq!(
        repo.sources(&scope, newest.output.output_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repo.memories(&scope, newest.output.output_id)
            .await
            .unwrap()
            .len(),
        1
    );
    // A rejected empty request never produces a storage plan or input row.
    let empty: NextInput = serde_json::from_value(serde_json::json!({"text":""})).unwrap();
    assert!(continue_permanent(&actor, &chat, empty, &[], now).is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_inputs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 27);
    cache.remove(&key).await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
