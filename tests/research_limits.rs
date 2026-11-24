use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc, watch};
use uuid::Uuid;

use cortex_api::core::{config::Config, process_numbers};
use cortex_api::dal::remote::{cache::RedisCache, web_page::PageReader};
use cortex_api::domain::{
    models::{
        driver::{CallInput, CallOutcome, DriverError, DriverEvent, ModelDriver, OutputModel},
        identity::Caller,
        run_record::{ModelCall, WebMode},
        search::{SearchAdapter, SearchFailure, SearchHit},
        security::{ClassifierUnavailable, InjectionClassifier},
        usage::Pending,
        web::WebInput,
    },
    services::{
        crawler::WebRetriever,
        events::channel,
        research_limits::{
            ConditionalCacheMeta, CoverageReport, FetchDecision, InMemoryLimitStore,
            PolitenessConfig, ResearchPageCache, ResearchPolitenessGuard, SharedLimitStore,
            ValkeyLimitStore, evaluate_publisher_policy, parse_bounded_retry_after,
            parse_rate_limit_wait,
        },
        search::WebSearch,
        security::SecurityShield,
    },
};

struct SafeClassifier;
impl InjectionClassifier for SafeClassifier {
    fn probability(&self, _: &str) -> Result<f32, ClassifierUnavailable> {
        Ok(0.01)
    }
}

struct NoDriver;
impl ModelDriver for NoDriver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        _: mpsc::UnboundedSender<DriverEvent>,
        _: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            let now = chrono::Utc::now();
            Ok(CallOutcome {
                inspection: None,
                text: "rust query".into(),
                usage: ModelCall {
                    images: None,
                    audio: None,
                    role: input.role,
                    model: output.name(&Config::default()).into(),
                    input_tokens: 1,
                    output_tokens: 1,
                    time_to_first_token_ms: None,
                    total_ms: 1,
                    started_at: now,
                    finished_at: now,
                },
                error: None,
                load_ms: 0,
                tokens_per_second: None,
            })
        })
    }
}

struct NoSearch;
impl SearchAdapter for NoSearch {
    fn name(&self) -> &'static str {
        "none"
    }
    fn search<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async { Ok(vec![]) })
    }
}

fn output_model() -> OutputModel {
    OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("u1".into()),
            is_admin: false,
        },
        Uuid::now_v7(),
        false,
    )
}

fn html_article(title: &str) -> String {
    let body = "Rust ownership protects memory safety without garbage collection. ".repeat(30);
    format!(
        "<html><head><title>{title}</title></head><body><article><h1>{title}</h1><p>{body}</p></article></body></html>"
    )
}

#[tokio::test]
async fn shared_quota_limits_across_users_and_429_cooldown_and_headers() {
    let store = Arc::new(InMemoryLimitStore::default());
    let guard = ResearchPolitenessGuard::new(
        PolitenessConfig {
            max_global_concurrency: 2,
            default_host_spacing_ms: 10,
            request_timeout_ms: 5_000,
            max_response_bytes: 1024 * 1024,
            max_backoff_secs: 20,
            provider_quota_per_minute: 2,
        },
        store,
    );

    let p1 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(p1.is_ok());
    drop(p1);

    let p2 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(p2.is_ok());
    drop(p2);

    let p3 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(matches!(p3, Err(FetchDecision::RateLimitedCooldown { .. })));

    assert_eq!(parse_bounded_retry_after(Some("120"), 20), Some(20));
    assert_eq!(
        parse_rate_limit_wait(None, Some("0"), Some("12"), 20),
        Some(12)
    );
    assert!(matches!(
        evaluate_publisher_policy("pantip.com", Some("unavailable"), true),
        FetchDecision::BlockedStop { .. }
    ));
    assert!(matches!(
        evaluate_publisher_policy("example.com", Some("public_page"), false),
        FetchDecision::BlockedStop { .. }
    ));

    let meta = ConditionalCacheMeta {
        etag: Some("\"v1\"".into()),
        last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
    };
    assert_eq!(meta.request_headers().len(), 2);
}

#[tokio::test]
async fn real_research_path_handles_429_retry_after_403_stop_and_challenge() {
    let hits_429 = Arc::new(AtomicUsize::new(0));
    let hits_403 = Arc::new(AtomicUsize::new(0));
    let hits_chal = Arc::new(AtomicUsize::new(0));

    let (c429, c403, cchal) = (hits_429.clone(), hits_403.clone(), hits_chal.clone());
    let app = Router::new()
        .route(
            "/rl",
            get(move || {
                let c = c429.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("retry-after", "7")],
                        "rate limited",
                    )
                }
            }),
        )
        .route(
            "/forbidden",
            get(move || {
                let c = c403.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::FORBIDDEN, "forbidden")
                }
            }),
        )
        .route(
            "/challenge",
            get(move || {
                let c = cchal.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    (
                        [("content-type", "text/html")],
                        "<html><body><div class='cf-challenge'>Verify you are human</div></body></html>",
                    )
                }
            }),
        )
        .route(
            "/ok",
            get(|| async { ([("content-type", "text/html")], html_article("Good")) }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let srv = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let config = Config {
        research_host_spacing: Duration::from_millis(5),
        ..Default::default()
    };
    let store: Arc<dyn SharedLimitStore> = Arc::new(InMemoryLimitStore::default());
    let politeness = Arc::new(ResearchPolitenessGuard::from_config(&config, store));
    let page_cache = Arc::new(ResearchPageCache::in_memory(config.research_cache_ttl));
    let retriever = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    )
    .with_politeness(politeness, page_cache);

    let (sink, _rx, events) = channel(&config, None);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let model = output_model();

    // 1. 429 sets cooldown in shared store, stops without retry loop, second call hits cooldown immediately
    let res_429 = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("Read {base}/rl"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert_eq!(hits_429.load(Ordering::SeqCst), 1);
    assert!(
        res_429
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == process_numbers::WEB_RATE_LIMITED.number)
    );
    assert!(matches!(
        res_429.gathered.coverage,
        CoverageReport::Partial { .. }
    ));

    // Cooldown prevents hitting /ok on the same cooled-down host
    let res_cooled = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("Read {base}/ok"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert!(
        res_cooled
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == process_numbers::WEB_RATE_LIMITED.number)
    );
    assert!(res_cooled.gathered.texts.is_empty());

    // 2. Fresh guard for 403 and challenge stop cleanly with 1 attempt each
    let retriever2 = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    );
    let res_403 = retriever2
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("Read {base}/forbidden"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert_eq!(hits_403.load(Ordering::SeqCst), 1);
    assert!(
        res_403
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == process_numbers::WEB_SOURCE_BLOCKED.number)
    );

    let retriever3 = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    );
    let res_chal = retriever3
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("Read {base}/challenge"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx,
        )
        .await;
    assert_eq!(hits_chal.load(Ordering::SeqCst), 1);
    assert!(
        res_chal
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == process_numbers::WEB_SOURCE_BLOCKED.number)
    );

    drop(sink);
    events.await.unwrap();
    srv.abort();
}

#[tokio::test]
async fn real_research_path_enforces_per_host_spacing_and_global_concurrency() {
    // Part A: per-host spacing and single in-flight per host
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let times = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let (inf, maxf, tlog) = (in_flight.clone(), max_in_flight.clone(), times.clone());
    let app = Router::new().route(
        "/p/{idx}",
        get(move || {
            let (inf, maxf, tlog) = (inf.clone(), maxf.clone(), tlog.clone());
            async move {
                let cur = inf.fetch_add(1, Ordering::SeqCst) + 1;
                maxf.fetch_max(cur, Ordering::SeqCst);
                tlog.lock().unwrap().push(Instant::now());
                tokio::time::sleep(Duration::from_millis(10)).await;
                inf.fetch_sub(1, Ordering::SeqCst);
                ([("content-type", "text/html")], html_article("Spaced"))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let srv = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let config = Config {
        research_host_spacing: Duration::from_millis(60),
        research_max_global_concurrency: 2,
        ..Default::default()
    };
    let retriever = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    );
    let (sink, _rx, events) = channel(&config, None);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let model = output_model();
    let res = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("{base}/p/1 {base}/p/2 {base}/p/3"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert_eq!(res.gathered.pages_read, 3);
    assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
    let recorded = times.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3);
    assert!(recorded[1].duration_since(recorded[0]) >= Duration::from_millis(50));
    assert!(recorded[2].duration_since(recorded[1]) >= Duration::from_millis(50));
    srv.abort();

    // Part B: global concurrency across distinct hosts (using 127.0.0.1 vs [::1] vs localhost / distinct guards?
    // Wait: distinct ports on 127.0.0.1 share host "127.0.0.1", so verify global semaphore directly + via 2 hostnames)
    let global_active = Arc::new(AtomicUsize::new(0));
    let global_max = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (ga, gm, ent, rel) = (
        global_active.clone(),
        global_max.clone(),
        entered.clone(),
        release.clone(),
    );
    let app_g = Router::new().route(
        "/g",
        get(move || {
            let (ga, gm, ent, rel) = (ga.clone(), gm.clone(), ent.clone(), rel.clone());
            async move {
                let c = ga.fetch_add(1, Ordering::SeqCst) + 1;
                gm.fetch_max(c, Ordering::SeqCst);
                ent.add_permits(1);
                rel.acquire().await.unwrap().forget();
                ga.fetch_sub(1, Ordering::SeqCst);
                ([("content-type", "text/html")], html_article("Global"))
            }
        }),
    );
    let l1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p1 = l1.local_addr().unwrap().port();
    let l2 = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let p2 = l2.local_addr().unwrap().port();
    let app_g2 = app_g.clone();
    let s1 = tokio::spawn(async move { axum::serve(l1, app_g).await.unwrap() });
    let s2 = tokio::spawn(async move { axum::serve(l2, app_g2).await.unwrap() });
    let config_g = Config {
        research_host_spacing: Duration::from_millis(1),
        research_max_global_concurrency: 2,
        ..Default::default()
    };
    let retriever_g = WebRetriever::new(
        &config_g,
        WebSearch::with_adapters(&config_g, vec![Arc::new(NoSearch)]),
        PageReader::new(&config_g).unwrap(),
        Arc::new(SecurityShield::new(&config_g, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    );
    let q = format!("http://127.0.0.1:{p1}/g http://localhost:{p1}/g http://127.0.0.2:{p2}/g");
    let sink_g = sink.clone();
    let m_g = model.clone();
    let c_g = cancel_rx.clone();
    let task = tokio::spawn(async move {
        retriever_g
            .gather(
                WebInput {
                    mode: WebMode::Fast,
                    question: &q,
                    needs_web: false,
                    model: &m_g,
                    images: vec![],
                },
                &sink_g,
                c_g,
            )
            .await
    });
    entered.acquire_many(2).await.unwrap().forget();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(global_active.load(Ordering::SeqCst), 2);
    release.add_permits(3);
    let res_g = task.await.unwrap();
    assert_eq!(res_g.gathered.pages_read, 3);
    assert_eq!(global_max.load(Ordering::SeqCst), 2);

    drop(sink);
    events.await.unwrap();
    s1.abort();
    s2.abort();
}

#[tokio::test]
async fn real_research_path_honors_robots_and_conditional_cache() {
    let private_hits = Arc::new(AtomicUsize::new(0));
    let cached_hits = Arc::new(AtomicUsize::new(0));
    let nostore_hits = Arc::new(AtomicUsize::new(0));
    let (ph, ch, nh) = (
        private_hits.clone(),
        cached_hits.clone(),
        nostore_hits.clone(),
    );
    let app = Router::new()
        .route(
            "/robots.txt",
            get(|| async {
                (
                    [("content-type", "text/plain")],
                    "User-agent: *\nDisallow: /private\n",
                )
            }),
        )
        .route(
            "/private",
            get(move || {
                let ph = ph.clone();
                async move {
                    ph.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/html")], html_article("Private"))
                }
            }),
        )
        .route(
            "/cached",
            get(move |headers: HeaderMap| {
                let ch = ch.clone();
                async move {
                    ch.fetch_add(1, Ordering::SeqCst);
                    if headers.get("if-none-match").and_then(|v| v.to_str().ok())
                        == Some("\"etag-1\"")
                    {
                        return StatusCode::NOT_MODIFIED.into_response();
                    }
                    (
                        StatusCode::OK,
                        [
                            ("content-type", "text/html"),
                            ("etag", "\"etag-1\""),
                            ("cache-control", "public, max-age=60"),
                        ],
                        html_article("Cached"),
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/nostore",
            get(move |headers: HeaderMap| {
                let nh = nh.clone();
                async move {
                    nh.fetch_add(1, Ordering::SeqCst);
                    assert!(headers.get("if-none-match").is_none());
                    (
                        StatusCode::OK,
                        [
                            ("content-type", "text/html"),
                            ("etag", "\"etag-no\""),
                            ("cache-control", "no-store"),
                        ],
                        html_article("NoStore"),
                    )
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let srv = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let config = Config {
        research_host_spacing: Duration::from_millis(1),
        ..Default::default()
    };
    let retriever = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(NoDriver),
    )
    .with_robots_on_direct_reads(true);

    let (sink, _rx, events) = channel(&config, None);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let model = output_model();

    // Robots blocks /private before fetch
    let r_priv = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: &format!("Read {base}/private"),
                needs_web: false,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert_eq!(private_hits.load(Ordering::SeqCst), 0);
    assert!(
        r_priv
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == process_numbers::WEB_SOURCE_BLOCKED.number)
    );

    // Conditional cache: first read stores ETag, second read gets 304 and keeps cached page
    for _ in 0..2 {
        let r_cache = retriever
            .gather(
                WebInput {
                    mode: WebMode::Fast,
                    question: &format!("Read {base}/cached"),
                    needs_web: false,
                    model: &model,
                    images: vec![],
                },
                &sink,
                cancel_rx.clone(),
            )
            .await;
        assert!(!r_cache.gathered.texts.is_empty());
    }
    assert_eq!(cached_hits.load(Ordering::SeqCst), 2);

    // Cache-Control: no-store is never cached
    for _ in 0..2 {
        let r_ns = retriever
            .gather(
                WebInput {
                    mode: WebMode::Fast,
                    question: &format!("Read {base}/nostore"),
                    needs_web: false,
                    model: &model,
                    images: vec![],
                },
                &sink,
                cancel_rx.clone(),
            )
            .await;
        assert!(!r_ns.gathered.texts.is_empty());
    }
    assert_eq!(nostore_hits.load(Ordering::SeqCst), 2);

    drop(sink);
    events.await.unwrap();
    srv.abort();
}

#[tokio::test]
#[ignore = "requires approved Valkey namespace"]
async fn valkey_limit_store_enforces_shared_quota_and_cooldown_across_instances() {
    dotenvy::dotenv().ok();
    let cache = RedisCache::from_environment();
    assert!(cache.is_configured(), "CORTEX_REDIS_URL must be configured");
    let unique = Uuid::new_v4().simple().to_string();
    let scope = format!("test-provider-{unique}");
    let host = format!("test-{unique}.example");

    let cfg = PolitenessConfig {
        max_global_concurrency: 2,
        default_host_spacing_ms: 5,
        request_timeout_ms: 5_000,
        max_response_bytes: 1024 * 1024,
        max_backoff_secs: 15,
        provider_quota_per_minute: 2,
    };
    let guard_user_a =
        ResearchPolitenessGuard::new(cfg.clone(), Arc::new(ValkeyLimitStore::new(cache.clone())));
    let guard_user_b = ResearchPolitenessGuard::new(cfg, Arc::new(ValkeyLimitStore::new(cache)));

    assert!(
        guard_user_a
            .acquire_host(&host, 5, &scope, 2, 30)
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        guard_user_b
            .acquire_host(&host, 5, &scope, 2, 30)
            .await
            .unwrap()
            .is_ok()
    );
    let over = guard_user_a
        .acquire_host(&host, 5, &scope, 2, 30)
        .await
        .unwrap();
    assert!(matches!(
        over,
        Err(FetchDecision::RateLimitedCooldown { .. })
    ));

    let d429 = guard_user_a
        .handle_status(&host, 429, Some("10"), false)
        .await
        .unwrap();
    assert_eq!(d429, FetchDecision::RateLimitedCooldown { wait_secs: 10 });
    let blocked_b = guard_user_b
        .acquire_host(&host, 5, &format!("other-{unique}"), 5, 30)
        .await
        .unwrap();
    assert!(matches!(
        blocked_b,
        Err(FetchDecision::RateLimitedCooldown { .. })
    ));
}
