use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use cortex_api::{
    core::config::Config,
    dal::local::{
        chats::Chats,
        database::{self, Target},
    },
    domain::models::{inspection::InspectionCapture, storage::*},
    presentation,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
#[ignore = "Uses the guarded test database and a unique temporary schema"]
async fn relational_roundtrip_scoping_cascade_and_reversible_migration() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_inspection_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(
            database::options(Target::Test)
                .unwrap()
                .options([("search_path", schema.as_str())]),
        )
        .await
        .unwrap();
    let migrations = sqlx::migrate!();
    migrations.run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let owner = AppUser {
        app_id: "cortex".into(),
        user_id: "synthetic-owner".into(),
    };
    let other = AppUser {
        app_id: "cortex".into(),
        user_id: "synthetic-other".into(),
    };
    let chat = Chat {
        session_id: Uuid::now_v7(),
        app_id: owner.app_id.clone(),
        user_id: owner.user_id.clone(),
        title: "Synthetic answer".into(),
        persona_id: None,
        model_id: "cortex-t0".into(),
        is_pinned: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    repo.insert_chat(&chat).await.unwrap();
    let input = ChatInput {
        input_id: Uuid::new_v4(),
        session_id: chat.session_id,
        text: "Synthetic input".into(),
        needs_web: false,
        web_mode: "fast".into(),
        use_memory: false,
        memory_topic: None,
        thinking: false,
        model_id: "cortex-t0".into(),
        created_at: chrono::Utc::now(),
    };
    repo.insert_input(&owner, &input).await.unwrap();
    let output = ChatOutput {
        output_id: Uuid::new_v4(),
        input_id: input.input_id,
        session_id: chat.session_id,
        status: "done".into(),
        text: "Aก💡 B".into(),
        thinking_text: None,
        memory_id: None,
        created_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
    };
    repo.insert_output(&owner, &output).await.unwrap();
    assert_eq!(
        repo.inspection(&owner, output.output_id)
            .await
            .unwrap()
            .reason
            .as_deref(),
        Some("not_recorded")
    );
    let mut capture = InspectionCapture::default();
    for byte in output.text.bytes() {
        capture.push(Some(&json!([{"bytes":[byte],"logprob":-0.2,"top_logprobs":[{"token":"x","logprob":-1.5},{"token":"","bytes":[],"logprob":-2.0}]}])),64);
    }
    let data = capture.finish("synthetic-model", &output.text, true);
    repo.save_inspection(&owner, output.output_id, &data)
        .await
        .unwrap();
    repo.save_inspection(&owner, output.output_id, &data)
        .await
        .unwrap();
    let read = repo.inspection(&owner, output.output_id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&read).unwrap(),
        serde_json::to_value(&data).unwrap()
    );
    assert!(repo.inspection(&other, output.output_id).await.is_err());
    assert!(
        repo.save_inspection(&other, output.output_id, &data)
            .await
            .is_err()
    );
    assert_eq!(
        repo.output(&owner, output.output_id)
            .await
            .unwrap()
            .unwrap()
            .text,
        output.text
    );
    let app = presentation::with_response_contract(presentation::chats::router(
        repo.clone(),
        Config::default(),
    ));
    let path = format!(
        "/chats/{}/outputs/{}/inspection",
        chat.session_id, output.output_id
    );
    for (user, status) in [
        (&owner.user_id, StatusCode::OK),
        (&other.user_id, StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("x-uuid", user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let wrong_path = format!(
        "/chats/{}/outputs/{}/inspection",
        Uuid::now_v7(),
        output.output_id
    );
    assert_eq!(
        app.oneshot(
            Request::builder()
                .uri(wrong_path)
                .header("x-uuid", &owner.user_id)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        sqlx::query(
            "UPDATE output_inspection_tokens SET logprob='NaN'::double precision WHERE output_id=$1"
        )
        .bind(output.output_id)
        .execute(&pool)
        .await
        .is_err()
    );
    let nested:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND table_name LIKE 'output_inspection%' AND (data_type IN ('json','jsonb','ARRAY'))").bind(&schema).fetch_one(&pool).await.unwrap();
    assert_eq!(nested, 0);
    sqlx::query("DELETE FROM chats WHERE session_id=$1")
        .bind(chat.session_id)
        .execute(&pool)
        .await
        .unwrap();
    for table in [
        "output_inspections",
        "output_inspection_tokens",
        "output_inspection_alternatives",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    migrations.undo(&pool, 3).await.unwrap();
    migrations.run(&pool).await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
