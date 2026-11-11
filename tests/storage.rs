use cortex_api::{
    dal::local::{
        chats::Chats,
        database::{self, Target},
    },
    domain::models::storage::*,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;
#[tokio::test]
#[ignore = "requires cortex_api_test; uses a unique temporary schema"]
async fn every_table_scope_constraints_and_reversible_migrations() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_storage_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    let migrations = sqlx::migrate!();
    migrations.run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let actor = AppUser {
        app_id: "fixture".into(),
        user_id: "user-a".into(),
    };
    let wrong = AppUser {
        app_id: "fixture".into(),
        user_id: "user-b".into(),
    };
    let other_app = AppUser {
        app_id: "other".into(),
        user_id: "user-a".into(),
    };
    let clock = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let chat = Chat {
        session_id: Uuid::now_v7(),
        app_id: actor.app_id.clone(),
        user_id: actor.user_id.clone(),
        title: "Fixture".into(),
        persona_id: None,
        is_pinned: false,
        created_at: clock,
        updated_at: clock,
    };
    repo.insert_chat(&chat).await.unwrap();
    assert_eq!(
        repo.chat(&actor, chat.session_id).await.unwrap(),
        Some(chat.clone())
    );
    assert_eq!(chat.session_id.get_version_num(), 7);
    let mut invalid = chat.clone();
    invalid.session_id = Uuid::new_v4();
    assert!(repo.insert_chat(&invalid).await.is_err());
    let input = ChatInput {
        input_id: Uuid::new_v4(),
        session_id: chat.session_id,
        text: "Question".into(),
        needs_web: false,
        web_mode: "auto".into(),
        use_memory: true,
        memory_topic: Some("fixture".into()),
        thinking: true,
        created_at: clock,
    };
    repo.insert_input(&actor, &input).await.unwrap();
    assert_eq!(
        repo.input(&actor, input.input_id).await.unwrap(),
        Some(input.clone())
    );
    let file = InputFile {
        input_id: input.input_id,
        file_id: "resource-fixture".into(),
        memory_id: "file-memory-fixture".into(),
        position: 0,
    };
    repo.insert_file(&actor, &file).await.unwrap();
    assert_eq!(
        repo.files(&actor, input.input_id).await.unwrap(),
        vec![file.clone()]
    );
    let output = ChatOutput {
        output_id: Uuid::new_v4(),
        input_id: input.input_id,
        session_id: chat.session_id,
        status: "done".into(),
        text: "Answer".into(),
        thinking_text: Some("Reason".into()),
        memory_id: Some("exchange-memory-fixture".into()),
        created_at: clock,
        finished_at: Some(clock),
    };
    repo.insert_output(&actor, &output).await.unwrap();
    assert_eq!(
        repo.output(&actor, output.output_id).await.unwrap(),
        Some(output.clone())
    );
    let source = OutputSource {
        output_id: output.output_id,
        position: 0,
        title: "Source".into(),
        url: "https://example.invalid".into(),
    };
    repo.insert_source(&actor, &source).await.unwrap();
    assert_eq!(
        repo.sources(&actor, output.output_id).await.unwrap(),
        vec![source.clone()]
    );
    let memory = OutputMemory {
        output_id: output.output_id,
        position: 0,
        memory_id: "memory-fixture".into(),
        title: "Memory".into(),
    };
    repo.insert_memory(&actor, &memory).await.unwrap();
    assert_eq!(
        repo.memories(&actor, output.output_id).await.unwrap(),
        vec![memory.clone()]
    );
    for scope in [&wrong, &other_app] {
        assert!(repo.chat(scope, chat.session_id).await.unwrap().is_none());
        assert!(repo.input(scope, input.input_id).await.unwrap().is_none());
        assert!(
            repo.output(scope, output.output_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.files(scope, input.input_id).await.unwrap().is_empty());
        assert!(
            repo.sources(scope, output.output_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.memories(scope, output.output_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            repo.insert_input(scope, &input).await,
            Err(StorageError::NotFound)
        );
        assert_eq!(
            repo.insert_output(scope, &output).await,
            Err(StorageError::NotFound)
        );
        assert_eq!(
            repo.insert_file(scope, &file).await,
            Err(StorageError::NotFound)
        );
        assert_eq!(
            repo.insert_source(scope, &source).await,
            Err(StorageError::NotFound)
        );
        assert_eq!(
            repo.insert_memory(scope, &memory).await,
            Err(StorageError::NotFound)
        );
    }
    let mut duplicate = output.clone();
    duplicate.output_id = Uuid::new_v4();
    assert!(repo.insert_output(&actor, &duplicate).await.is_err());
    let mut mismatched = output.clone();
    mismatched.session_id = Uuid::now_v7();
    assert_eq!(
        repo.insert_output(&actor, &mismatched).await,
        Err(StorageError::NotFound)
    );
    let forbidden:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND (data_type IN ('ARRAY','json','jsonb') OR column_name IN ('deleted_at','clean','expires_at','is_temporary'))").bind(&schema).fetch_one(&pool).await.unwrap();
    assert_eq!(forbidden, 0);
    sqlx::query("DELETE FROM chats WHERE session_id=$1")
        .bind(chat.session_id)
        .execute(&pool)
        .await
        .unwrap();
    for table in [
        "chats",
        "chat_inputs",
        "chat_input_files",
        "chat_outputs",
        "chat_output_sources",
        "chat_output_memories",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    migrations.undo(&pool, 0).await.unwrap();
    let tables:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema=$1 AND table_name!='_sqlx_migrations'").bind(&schema).fetch_one(&pool).await.unwrap();
    assert_eq!(tables, 0);
    migrations.run(&pool).await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
