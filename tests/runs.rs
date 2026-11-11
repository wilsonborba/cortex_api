use cortex_api::{
    dal::remote::runs::{RunDatabase, RunStore},
    domain::models::run_record::*,
};
use serde_json::{Value, json};
use uuid::Uuid;
fn record(mode: WebMode, thinking: bool, total_ms: u64) -> RunRecord {
    let clock = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    RunRecord {
        session_id: Uuid::now_v7(),
        output_id: Uuid::new_v4(),
        app_id: "fixture".into(),
        user_id: "fixture-user".into(),
        created_at: clock,
        mode: RunMode {
            web_mode: mode,
            thinking,
        },
        total_ms,
        stages: vec![StageRecord {
            name: "Save".into(),
            status: "ok".into(),
            error_number: None,
            notice_number: None,
            time_ms: 1,
        }],
        calls: vec![ModelCall {
            images: None,
            audio: None,
            role: "answer".into(),
            model: "fixture-model".into(),
            input_tokens: 20,
            output_tokens: 10,
            time_to_first_token_ms: Some(10),
            total_ms: 50,
            started_at: clock,
            finished_at: clock,
        }],
        counts: RunCounts {
            pages_read: 0,
            chunks_used: 0,
            files: 0,
            file_pages: 0,
            audio_seconds: 0.0,
        },
    }
}
#[test]
fn typed_guardrail_rejects_unknown_fields_at_every_level() {
    let r = record(WebMode::Fast, false, 100);
    let value = serde_json::to_value(&r).unwrap();
    for key in ["prompt", "file_content", "schema_version"] {
        let mut invalid = value.clone();
        invalid[key] = json!("fixture");
        assert!(serde_json::from_value::<RunRecord>(invalid).is_err());
    }
    for field in ["mode", "counts"] {
        let mut invalid = value.clone();
        invalid[field]["unknown"] = json!(true);
        assert!(serde_json::from_value::<RunRecord>(invalid).is_err());
    }
    for field in ["stages", "calls"] {
        let mut invalid = value.clone();
        invalid[field][0]["text"] = json!("fixture");
        assert!(serde_json::from_value::<RunRecord>(invalid).is_err());
    }
    let mut invalid = r;
    invalid.counts.audio_seconds = f64::NAN;
    assert!(serde_json::to_value(&invalid).is_err());
}
fn metric<'a>(stats: &'a Value, mode: &str, thinking: bool, index: usize) -> Option<&'a Value> {
    stats["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == json!([mode, thinking]))
        .map(|r| &r["value"][index])
}
#[tokio::test]
#[ignore = "requires the approved Cortex CouchDB test database"]
async fn initialize_indexes_view_typed_roundtrip_and_grouped_stats() {
    dotenvy::dotenv().ok();
    let store = RunStore::from_environment(RunDatabase::Test).unwrap();
    store.initialize().await.unwrap();
    store.initialize().await.unwrap();
    let before = store.stats().await.unwrap();
    let fast_count = metric(&before, "fast", false, 0).map_or(0, |v| v["count"].as_u64().unwrap());
    let fast_sum = metric(&before, "fast", false, 0).map_or(0.0, |v| v["sum"].as_f64().unwrap());
    let one = record(WebMode::Fast, false, 100);
    let two = record(WebMode::Fast, false, 200);
    let mut failed = record(WebMode::Deep, true, 300);
    failed.stages[0].status = "failed".into();
    failed.stages[0].error_number = Some(1008);
    for run in [&one, &two, &failed] {
        store.put(run).await.unwrap();
        assert_eq!(store.get(run.output_id).await.unwrap(), Some(run.clone()));
    }
    store.put(&one).await.unwrap();
    let stats = store.stats().await.unwrap();
    let fast = metric(&stats, "fast", false, 0).unwrap();
    assert_eq!(fast["count"].as_u64().unwrap(), fast_count + 2);
    assert_eq!(fast["sum"].as_f64().unwrap(), fast_sum + 300.0);
    assert!(metric(&stats, "deep", true, 0).is_some());
    let input = metric(&stats, "fast", false, 1).unwrap();
    assert!(input["sum"].as_f64().unwrap() >= 40.0);
    assert_eq!(metric(&stats, "deep", true, 4).unwrap()["min"], 0);
    let config = cortex_api::core::config::Config::default();
    let url = std::env::var("CORTEX_COUCHDB_URL").unwrap();
    let user = std::env::var("CORTEX_COUCHDB_USERNAME").unwrap();
    let password = std::env::var("CORTEX_COUCHDB_PASSWORD").unwrap();
    let response = reqwest::Client::new()
        .get(format!(
            "{}/{}/_index",
            url.trim_end_matches('/'),
            config.run_test_database
        ))
        .basic_auth(user, Some(password))
        .send()
        .await
        .unwrap_or_else(|_| panic!("The index test request failed"));
    let indexes: Value = response.json().await.unwrap();
    for fields in [
        json!([{"user_id":"asc"},{"created_at":"asc"}]),
        json!([{"created_at":"asc"}]),
    ] {
        assert!(
            indexes["indexes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["def"]["fields"] == fields)
        );
    }
    for run in [&one, &two, &failed] {
        store.remove(run.output_id).await.unwrap();
        store.remove(run.output_id).await.unwrap();
        assert_eq!(store.get(run.output_id).await.unwrap(), None);
    }
}
