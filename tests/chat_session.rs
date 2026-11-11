use chrono::{TimeZone, Utc};
use cortex_api::{
    core::config::Config,
    dal::local::{
        chats::Chats,
        database::{self, Target},
    },
    domain::{
        models::{chat_session::*, identity::Caller, storage::StorageError},
        services::chat_session::{self, continue_permanent, new_permanent},
    },
};
use uuid::Uuid;
fn caller() -> Caller {
    Caller {
        app_id: "cortex".into(),
        user_id: Some("user-a".into()),
        is_admin: false,
    }
}
fn new(text: &str) -> NewChat {
    serde_json::from_value(serde_json::json!({"app_id":"cortex","text":text})).unwrap()
}
fn next(text: &str) -> NextInput {
    serde_json::from_value(serde_json::json!({"text":text})).unwrap()
}
#[test]
fn strict_request_bodies_defaults_and_no_identity_override() {
    let body = new("question");
    assert!(!body.stream);
    assert!(!body.temporary);
    assert!(!body.thinking);
    assert!(!body.use_memory);
    assert!(serde_json::from_value::<NewChat>(serde_json::json!({"text":"question"})).is_err());
    for key in [
        "session_id",
        "user_id",
        "tenant_id",
        "skip_security",
        "model",
        "capabilities",
        "max_tokens",
    ] {
        let mut value = serde_json::json!({"app_id":"cortex","text":"question"});
        value[key] = "unexpected".into();
        assert!(serde_json::from_value::<NewChat>(value).is_err());
    }
    for key in ["app_id", "persona_id", "temporary", "session_id", "user_id"] {
        let mut value = serde_json::json!({"text":"question"});
        value[key] = "unexpected".into();
        assert!(serde_json::from_value::<NextInput>(value).is_err());
    }
    assert!(
        serde_json::from_value::<NextInput>(
            serde_json::json!({"text":"question","web_mode":"auto"})
        )
        .is_err()
    );
}
#[test]
fn title_word_boundaries_unicode_and_first_file() {
    let config = Config::default();
    assert_eq!(
        chat_session::title(&config, "First line\nSecond line", &[]),
        "First line"
    );
    let line = format!("{} lastword", "word ".repeat(11));
    let title = chat_session::title(&config, &line, &[]);
    assert_eq!(title, "word ".repeat(11).trim_end());
    let long = "ภาษาไทย".repeat(20);
    let title = chat_session::title(&config, &long, &[]);
    assert_eq!(title.chars().count(), 60);
    assert!(long.starts_with(&title));
    let file = SessionFile {
        file_id: "resource".into(),
        memory_id: "memory".into(),
        name: "report.pdf".into(),
    };
    assert_eq!(chat_session::title(&config, " \n", &[file]), "report.pdf");
}
#[test]
fn ids_attribution_files_and_temporary_never_prepare_for_sql() {
    let config = Config::default();
    let actor = caller();
    let now = Utc::now();
    let prepared = new_permanent(&config, &actor, new("question"), &[], now)
        .unwrap_or_else(|_| panic!("valid input failed"));
    let ids = prepared.ids();
    for id in [ids.session_id, ids.input_id, ids.output_id] {
        assert_eq!(id.get_version_num(), 7);
    }
    assert!(ids.session_id < ids.input_id && ids.input_id < ids.output_id);
    assert_eq!(prepared.scope.user_id, "user-a");
    assert_eq!(prepared.output.status, "running");
    assert!(prepared.output.text.is_empty());
    let chat = prepared.new_chat.unwrap();
    let follow = continue_permanent(&actor, &chat, next("next"), &[], now)
        .unwrap_or_else(|_| panic!("continuation failed"));
    assert_eq!(follow.input.session_id, chat.session_id);
    assert!(follow.new_chat.is_none());
    let mut wrong = actor.clone();
    wrong.user_id = Some("user-b".into());
    assert!(continue_permanent(&wrong, &chat, next("next"), &[], now).is_err());
    let mut unknown = new("question");
    unknown.app_id = "unknown".into();
    assert_eq!(
        new_permanent(&config, &actor, unknown, &[], now)
            .err()
            .unwrap()
            .number,
        1009
    );
    let mut temporary = new("question");
    temporary.temporary = true;
    assert!(new_permanent(&config, &actor, temporary, &[], now).is_err());
    assert!(new_permanent(&config, &actor, new(""), &[], now).is_err());
    let mut file_only = new("");
    file_only.file_ids = vec!["resource".into()];
    assert!(new_permanent(&config, &actor, file_only.clone(), &[], now).is_err());
    let file = SessionFile {
        file_id: "resource".into(),
        memory_id: "memory".into(),
        name: "name.txt".into(),
    };
    let prepared = new_permanent(&config, &actor, file_only, &[file], now)
        .unwrap_or_else(|_| panic!("file input failed"));
    assert_eq!(prepared.files[0].memory_id, "memory");
    assert_eq!(prepared.new_chat.unwrap().title, "name.txt");
}
#[tokio::test]
#[ignore = "requires cortex_api_test; uses a unique temporary schema"]
async fn create_continue_and_failure_are_atomic_and_scoped() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_sessions_{}", Uuid::new_v4().simple());
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
    let migrations = sqlx::migrate!();
    migrations.run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let config = Config::default();
    let actor = caller();
    let now = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
    let mut body = new("first question");
    body.persona_id = Some("bot-persona".into());
    let first =
        new_permanent(&config, &actor, body, &[], now).unwrap_or_else(|_| panic!("prepare failed"));
    repo.save_accepted(&first).await.unwrap();
    let chat = repo
        .chat(&actor.storage_scope(), first.input.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chat.persona_id.as_deref(), Some("bot-persona"));
    assert_eq!(
        repo.output(&actor.storage_scope(), first.output.output_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    let later = now + chrono::Duration::seconds(1);
    let follow = continue_permanent(&actor, &chat, next("second question"), &[], later)
        .unwrap_or_else(|_| panic!("prepare failed"));
    repo.save_accepted(&follow).await.unwrap();
    assert_eq!(
        repo.chat(&actor.storage_scope(), chat.session_id)
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        later
    );
    let mut wrong = follow.clone();
    wrong.scope.user_id = "user-b".into();
    assert_eq!(
        repo.save_accepted(&wrong).await,
        Err(StorageError::NotFound)
    );
    // An output ID collision happens after the new chat and input inserts.
    // Neither insert may survive the failed transaction.
    let mut collision = new_permanent(&config, &actor, new("will roll back"), &[], now)
        .unwrap_or_else(|_| panic!("prepare failed"));
    collision.output.output_id = first.output.output_id;
    assert!(repo.save_accepted(&collision).await.is_err());
    assert!(
        repo.chat(&actor.storage_scope(), collision.input.session_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.input(&actor.storage_scope(), collision.input.input_id)
            .await
            .unwrap()
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_inputs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let older = continue_permanent(&actor, &chat, next("older clock"), &[], now)
        .unwrap_or_else(|_| panic!("prepare failed"));
    repo.save_accepted(&older).await.unwrap();
    assert_eq!(
        repo.chat(&actor.storage_scope(), chat.session_id)
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        later
    );
    migrations.undo(&pool, 2).await.unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND table_name='chats' AND column_name='persona_id'").bind(&schema).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);
    migrations.run(&pool).await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
