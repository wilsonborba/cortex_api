use chrono::{Duration, TimeZone, Utc};
use cortex_api::{
    core::config::Config,
    dal::remote::loyalty::{Credentials, SupabaseClient},
    domain::models::loyalty::LoyaltySnapshot,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};
fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap()
}
#[test]
fn caps_and_same_day_signins_use_the_aligned_formula() {
    let now = now();
    let today = now.date_naive();
    let value = LoyaltySnapshot::calculate(
        now - Duration::days(730),
        now,
        [today, today, today - Duration::days(1)],
    );
    assert_eq!(value.account_days, 730);
    assert_eq!(value.sign_in_days_90, 2);
    assert_eq!(value.age, 1.0);
    assert!((value.frequency - 2.0 / 90.0).abs() < 1e-12);
    assert!(value.valid());
    let daily = LoyaltySnapshot::calculate(
        now - Duration::days(365),
        now,
        (0..=90).map(|n| (now - Duration::days(n)).date_naive()),
    );
    assert_eq!(daily.frequency, 1.0);
    assert_eq!(daily.weight, 3.0);
    let new = LoyaltySnapshot::calculate(now, now, []);
    assert_eq!(new.weight, 1.0);
}
#[test]
fn window_boundaries_and_invalid_cached_values_are_checked() {
    let now = now();
    let value = LoyaltySnapshot::calculate(
        now + Duration::days(1),
        now,
        [
            (now - Duration::days(91)).date_naive(),
            (now - Duration::days(90)).date_naive(),
            now.date_naive(),
            (now + Duration::days(1)).date_naive(),
        ],
    );
    assert_eq!(value.account_days, 0);
    assert_eq!(value.sign_in_days_90, 2);
    assert!(
        !LoyaltySnapshot {
            weight: f64::NAN,
            ..value
        }
        .valid()
    );
    assert!(
        !LoyaltySnapshot {
            frequency: 0.9,
            ..value
        }
        .valid()
    );
    assert!(
        !LoyaltySnapshot {
            weight: 4.0,
            ..value
        }
        .valid()
    );
}
#[tokio::test]
async fn source_uses_read_only_sql_and_login_days_and_rejects_outage() {
    use axum::{
        Json, Router,
        extract::State,
        http::HeaderMap,
        routing::{get, post},
    };
    let queries = Arc::new(Mutex::new(Vec::<String>::new()));
    async fn accounts(
        State(queries): State<Arc<Mutex<Vec<String>>>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer fixture-token"
        );
        let query = body["query"].as_str().unwrap();
        assert!(query.starts_with("SELECT "));
        assert!(query.contains("auth.sessions"));
        assert!(query.contains("AT TIME ZONE 'UTC'"));
        assert!(query.contains("to_char(u.created_at"));
        queries.lock().unwrap().push(query.into());
        Json(
            json!([{"user_id":"00000000-0000-0000-0000-000000000001","created_at":"2025-11-11T00:00:00Z","last_sign_in_at":"2026-11-10T12:00:00Z","session_days":["2026-11-10","2026-11-10"]}]),
        )
    }
    async fn audit(uri: axum::http::Uri) -> Json<Value> {
        let params = url::form_urlencoded::parse(uri.query().unwrap().as_bytes())
            .collect::<std::collections::HashMap<_, _>>();
        assert!(params["iso_timestamp_start"].ends_with('Z'));
        assert!(params["iso_timestamp_end"].ends_with('Z'));
        assert!(!params["iso_timestamp_start"].contains('+'));
        let query = &params["sql"];
        assert!(query.contains("='login'"));
        assert!(!query.contains("token_refreshed"));
        assert!(!query.contains("user_signedup"));
        Json(
            json!({"result":[{"user_id":"00000000-0000-0000-0000-000000000001","signin_day":"2026-11-09"}],"error":null}),
        )
    }
    let app = Router::new()
        .route(
            "/v1/projects/fixture/database/query/read-only",
            post(accounts),
        )
        .route("/v1/projects/fixture/analytics/endpoints/logs", get(audit))
        .with_state(queries.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = Config {
        loyalty_source_timeout: StdDuration::from_millis(100),
        ..Config::default()
    };
    let client = SupabaseClient::new(
        format!("http://{address}/").parse().unwrap(),
        Credentials {
            project_ref: "fixture".into(),
            access_token: "fixture-token".into(),
        },
        config.clone(),
    )
    .unwrap();
    let ids = [uuid::Uuid::from_u128(1)];
    let accounts = client.accounts(&ids, now()).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(queries.lock().unwrap().len(), 1);
    let days = client.audit_days(&ids, now()).await.unwrap();
    assert_eq!(days[&ids[0]].len(), 1);
    server.abort();
    let _ = server.await;
    assert!(client.accounts(&ids, now()).await.is_err());
}

use cortex_api::dal::remote::cache::RedisCache;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
};
use uuid::Uuid;
struct LocalValkey {
    child: Child,
    dir: PathBuf,
}
impl Drop for LocalValkey {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn valkey() -> (LocalValkey, RedisCache, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = root.join(".scratch/valkey/usr/bin/valkey-server");
    assert!(bin.exists(), "run scripts/setup-test-valkey.sh");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dir = root.join(format!(".scratch/valkey-test-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(bin)
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--save",
            "",
            "--appendonly",
            "no",
            "--daemonize",
            "no",
        ])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("local test Valkey did not start");
    let process = LocalValkey { child, dir };
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
    let url = format!("redis://127.0.0.1:{port}/0");
    (
        process,
        RedisCache::new(Some(&url), StdDuration::from_secs(1)),
        url,
    )
}
#[tokio::test]
#[ignore = "requires the isolated local Valkey binary; no shared cache keys"]
async fn refresh_reaches_scheduler_and_telemetry_and_outage_keeps_snapshot() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use cortex_api::{
        dal::remote::usage::RedisLedger,
        domain::{
            models::{
                cache::CacheKey,
                loyalty::{LoyaltySource, LoyaltyUnavailable, Pending},
                run_record::RunRecord,
                usage::{LedgerUnavailable, UsageLedger, UsageSource},
            },
            tasks::loyalty::refresh,
        },
        presentation::{self, telemetry},
    };
    use tower::ServiceExt;
    struct Source(Option<LoyaltySnapshot>);
    impl LoyaltySource for Source {
        fn fetch<'a>(
            &'a self,
            _: chrono::DateTime<Utc>,
        ) -> Pending<'a, Result<Vec<(String, LoyaltySnapshot)>, LoyaltyUnavailable>> {
            Box::pin(async move {
                self.0
                    .map(|v| vec![("fixture-user".into(), v)])
                    .ok_or(LoyaltyUnavailable)
            })
        }
    }
    struct Records;
    impl UsageSource for Records {
        fn records_since<'a>(
            &'a self,
            _: chrono::DateTime<Utc>,
        ) -> cortex_api::domain::models::usage::Pending<'a, Result<Vec<RunRecord>, LedgerUnavailable>>
        {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let (_process, cache, _url) = valkey().await;
    let snapshot =
        LoyaltySnapshot::calculate(now() - Duration::days(365), now(), [now().date_naive()]);
    assert_eq!(
        refresh(&Source(Some(snapshot)), &cache, &Config::default(), now())
            .await
            .unwrap(),
        1
    );
    let ledger = Arc::new(RedisLedger::new(
        cache.clone(),
        Arc::new(Records),
        "fixture-queue".into(),
    ));
    let share = ledger.share("fixture-user", now()).await.unwrap();
    assert_eq!(share.weight, snapshot.weight);
    assert_eq!(share.loyalty, Some(snapshot));
    assert!(
        refresh(&Source(None), &cache, &Config::default(), now())
            .await
            .is_err()
    );
    let stored = cache
        .get(&CacheKey::Loyalty("fixture-user".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["weight"], snapshot.weight);
    let app =
        presentation::with_response_contract(telemetry::router(ledger.clone(), &Config::default()));
    let response = app
        .oneshot(
            Request::get("/telemetry/usage")
                .header("x-uuid", "fixture-user")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["data"]["loyalty_weight"]["value"], snapshot.weight);
    assert_eq!(body["data"]["loyalty_weight"]["account_days"], 365);
    assert_eq!(body["data"]["loyalty_weight"]["sign_in_days_90"], 1);
    let unknown = ledger.share("unknown", now()).await.unwrap();
    assert_eq!(unknown.weight, 1.0);
    assert!(unknown.loyalty.is_none());
}
