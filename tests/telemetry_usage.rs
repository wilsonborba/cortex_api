use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cortex_api::{
    core::{
        config::Config,
        process_numbers::{MISSING_USER, USAGE_UNAVAILABLE},
    },
    domain::models::usage::*,
    presentation::{telemetry, with_response_contract},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;
struct Ledger {
    users: HashMap<String, Usage>,
    down: bool,
}
impl UsageLedger for Ledger {
    fn share<'a>(
        &'a self,
        actor: &'a str,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Pending<'a, Result<Share, LedgerUnavailable>> {
        Box::pin(async move {
            if self.down {
                return Err(LedgerUnavailable);
            }
            Ok(Share {
                usage: self.users.get(actor).copied().unwrap_or_default(),
                weight: 1.0,
            })
        })
    }
    fn record<'a>(&'a self, _: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async { Ok(()) })
    }
    fn waiting<'a>(&'a self, _: Vec<WaitingCall>) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async { Ok(()) })
    }
}
async fn get(app: Router, user: Option<&str>, query: &str, admin: bool) -> (StatusCode, Value) {
    let mut request = Request::get(format!("/telemetry/usage{query}"));
    if let Some(user) = user {
        request = request.header("x-uuid", user);
    }
    if admin {
        request = request.header("x-access-level", "admin");
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap(),
    )
}
#[tokio::test]
async fn each_user_sees_only_own_usage_and_unavailable_loyalty_parts() {
    let ledger = Arc::new(Ledger {
        users: HashMap::from([
            (
                "a".into(),
                Usage {
                    input: 10,
                    output: 2,
                    images: 1,
                    audio: 0,
                },
            ),
            (
                "b".into(),
                Usage {
                    input: 70,
                    output: 3,
                    images: 0,
                    audio: 2,
                },
            ),
        ]),
        down: false,
    });
    let app = with_response_contract(telemetry::router(ledger, &Config::default()));
    for (user, input, output) in [("a", 10, 2), ("b", 70, 3)] {
        let (status, body) = get(app.clone(), Some(user), "", false).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["tokens"],
            json!({"input":input,"output":output,"total":input+output})
        );
        assert_eq!(body["data"]["window_hours"], 24);
        assert_eq!(body["data"]["formula"], "1 + age + frequency");
        assert_eq!(
            body["data"]["loyalty_weight"],
            json!({"value":1.0,"age":null,"frequency":null,"account_days":null,"sign_in_days_90":null})
        );
    }
    let (_, own) = get(app.clone(), Some("a"), "?user_id=b", true).await;
    assert_eq!(own["data"]["tokens"]["input"], 10);
    let (status, body) = get(app, None, "?user_id=a", false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], MISSING_USER.number);
}
#[tokio::test]
async fn unavailable_ledger_returns_its_number() {
    let app = with_response_contract(telemetry::router(
        Arc::new(Ledger {
            users: HashMap::new(),
            down: true,
        }),
        &Config::default(),
    ));
    let (status, body) = get(app, Some("a"), "", false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["data"]["error_number"], USAGE_UNAVAILABLE.number);
}
struct EmptyRecords;
impl UsageSource for EmptyRecords {
    fn records_since<'a>(
        &'a self,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Pending<
        'a,
        Result<Vec<cortex_api::domain::models::run_record::RunRecord>, LedgerUnavailable>,
    > {
        Box::pin(async { Ok(vec![]) })
    }
}
#[tokio::test]
#[ignore = "requires the approved Valkey namespace; unique fixture keys only"]
async fn route_values_match_real_redis_buckets() {
    use cortex_api::dal::remote::{cache::RedisCache, usage::RedisLedger};
    dotenvy::dotenv().ok();
    let cache = RedisCache::from_environment();
    let ledger = Arc::new(RedisLedger::new(
        cache.clone(),
        Arc::new(EmptyRecords),
        format!("test-unused-{}", uuid::Uuid::new_v4().simple()),
    ));
    let now = chrono::Utc::now();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for (actor, usage) in [
        (
            &a,
            Usage {
                input: 10,
                output: 2,
                images: 1,
                audio: 0,
            },
        ),
        (
            &b,
            Usage {
                input: 70,
                output: 3,
                images: 0,
                audio: 2,
            },
        ),
    ] {
        ledger
            .record(CallUsage {
                actor: actor.clone(),
                output_id: uuid::Uuid::new_v4(),
                finished_at: now,
                usage,
            })
            .await
            .unwrap();
    }
    let app = with_response_contract(telemetry::router(ledger.clone(), &Config::default()));
    for actor in [&a, &b] {
        let (status, body) = get(app.clone(), Some(actor), "", false).await;
        assert_eq!(status, StatusCode::OK);
        let values = cache
            .get_many(&ledger.bucket_keys(actor, chrono::Utc::now()))
            .await
            .unwrap();
        let mut sum = Usage::default();
        for value in values {
            sum.add(serde_json::from_value(value.unwrap()).unwrap());
        }
        assert_eq!(
            body["data"]["tokens"],
            json!({"input":sum.input,"output":sum.output,"total":sum.total()})
        );
        assert_eq!(
            body["data"]["items"],
            json!({"images":sum.images,"audio":sum.audio})
        );
    }
    for actor in [&a, &b] {
        let mut keys = ledger.bucket_keys(actor, now);
        keys.extend(ledger.bucket_keys(actor, chrono::Utc::now()));
        keys.push(cortex_api::domain::models::cache::CacheKey::Loyalty(
            actor.clone(),
        ));
        for key in keys {
            cache.remove(&key).await.unwrap();
        }
    }
}
