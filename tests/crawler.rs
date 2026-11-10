use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
};
use cortex_api::{
    core::config::Config,
    dal::remote::web_page::PageReader,
    domain::{
        models::{
            driver::*,
            identity::Caller,
            run_record::{ModelCall, WebMode},
            search::*,
            security::*,
            usage::Pending,
            web::WebInput,
        },
        services::{
            crawler::WebRetriever, events::channel, search::WebSearch, security::SecurityShield,
        },
    },
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, watch};
use uuid::Uuid;
struct Safe;
impl InjectionClassifier for Safe {
    fn probability(&self, _: &str) -> Result<f32, ClassifierUnavailable> {
        Ok(0.01)
    }
}
#[derive(Clone, Default)]
struct Site {
    hits: Arc<Mutex<BTreeMap<String, usize>>>,
    mixed: bool,
    slow: bool,
    external: Option<String>,
}
async fn page(State(state): State<Site>, Path(path): Path<String>) -> Response {
    *state.hits.lock().unwrap().entry(path.clone()).or_default() += 1;
    if state.slow && path != "root/0" {
        return std::future::pending::<Response>().await;
    }
    let next = if path.starts_with("root/") {
        1
    } else if path.starts_with("level1/") {
        2
    } else {
        3
    };
    let mut links = String::new();
    if state.mixed {
        links.push_str("<a href='/blocked'>Rust memory</a><a href='/download.PDF'>Rust memory</a><a href='mailto:mail@example.invalid'>Rust memory</a><a href='/noise'>unrelated orchard</a><a href='/root/0'>Rust memory</a>");
    }
    if let Some(external) = &state.external {
        links.push_str(&format!(
            "<a href='{external}/level1/remote'>Rust memory</a>"
        ));
    }
    for i in 0..5 {
        links.push_str(&format!("<a href='/level{next}/{i}'>Rust memory</a>"));
    }
    let text = if next >= 3 {
        "Rust memory ownership and borrowing protect memory safety. "
    } else {
        "A compiler translates source programs into machine instructions. "
    };
    ([("content-type","text/html")],format!("<html><head><title>{path}</title></head><body><article><h1>{path}</h1><p>{}</p><p>{}</p>{links}</article></body></html>",text.repeat(30),text.repeat(30))).into_response()
}
async fn server(
    mixed: bool,
    slow: bool,
    external: Option<String>,
) -> (String, Site, tokio::task::JoinHandle<()>) {
    let state = Site {
        mixed,
        slow,
        external,
        ..Default::default()
    };
    let app = Router::new()
        .route(
            "/robots.txt",
            get(|State(state): State<Site>| async move {
                *state
                    .hits
                    .lock()
                    .unwrap()
                    .entry("robots.txt".into())
                    .or_default() += 1;
                (
                    [("content-type", "text/plain")],
                    "User-agent: *\nDisallow: /blocked\nDisallow: /root\n",
                )
            }),
        )
        .route("/{*path}", get(page))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, state, task)
}
struct Search {
    base: String,
    calls: Arc<AtomicUsize>,
}
impl SearchAdapter for Search {
    fn name(&self) -> &'static str {
        "local-site"
    }
    fn search<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((0..5)
                .map(|i| SearchHit {
                    title: format!("root {i}"),
                    url: format!("{}/root/{i}", self.base),
                })
                .collect())
        })
    }
}
struct Driver {
    calls: AtomicUsize,
    slow: bool,
    entered: Arc<Semaphore>,
}
impl ModelDriver for Driver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        _: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(input.role, "queries");
            let start = chrono::Utc::now();
            self.entered.add_permits(1);
            if self.slow {
                while !*cancel.borrow() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
            }
            let end = chrono::Utc::now();
            Ok(CallOutcome {
                text: "rust memory\nrust safety\nrust borrowing\nrust pointers".into(),
                usage: ModelCall {
                    images: None,
                    audio: None,
                    role: "queries".into(),
                    model: output.name(&Config::default()).into(),
                    input_tokens: 12,
                    output_tokens: 4,
                    time_to_first_token_ms: Some(1),
                    total_ms: 2,
                    started_at: start,
                    finished_at: end,
                },
                error: if self.slow {
                    Some(DriverError::Cancelled)
                } else {
                    None
                },
                load_ms: 0,
                tokens_per_second: None,
            })
        })
    }
}
fn setup(config: &Config, base: &str, driver: Arc<Driver>) -> (WebRetriever, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let search = WebSearch::with_adapters(
        config,
        vec![Arc::new(Search {
            base: base.into(),
            calls: calls.clone(),
        })],
    );
    let reader = PageReader::new(config).unwrap();
    let shield = Arc::new(SecurityShield::new(config, Arc::new(Safe)).unwrap());
    (
        WebRetriever::new(config, search, reader, shield, driver),
        calls,
    )
}
fn output_model() -> OutputModel {
    OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("fixture".into()),
            is_admin: false,
        },
        Uuid::now_v7(),
        false,
    )
}
fn driver(slow: bool) -> Arc<Driver> {
    Arc::new(Driver {
        calls: AtomicUsize::new(0),
        slow,
        entered: Arc::new(Semaphore::new(0)),
    })
}
#[tokio::test]
async fn fixed_depth_and_page_budgets_and_one_query_call() {
    for (mode, total, queries) in [
        (WebMode::Fast, 5, 0),
        (WebMode::Medium, 10, 3),
        (WebMode::Deep, 15, 4),
    ] {
        let (base, site, server) = server(false, false, None).await;
        let config = Config::default();
        assert_eq!(config.medium_web_timeout, Duration::from_secs(30));
        assert_eq!(config.deep_web_timeout, Duration::from_secs(60));
        let driver = driver(false);
        let (web, search_calls) = setup(&config, &base, driver.clone());
        let (sink, _rx, events) = channel(&config, None);
        let (_control, cancel) = watch::channel(false);
        let model = output_model();
        let result = web
            .gather(
                WebInput {
                    mode,
                    question: "rust memory ownership borrowing",
                    needs_web: true,
                    model: &model,
                    images: vec![],
                },
                &sink,
                cancel,
            )
            .await;
        assert!(result.error.is_none());
        assert_eq!(result.gathered.pages_read, total);
        assert!(result.gathered.texts.len() <= 8);
        assert_eq!(
            driver.calls.load(Ordering::SeqCst),
            usize::from(mode != WebMode::Fast)
        );
        assert_eq!(result.calls.len(), usize::from(mode != WebMode::Fast));
        assert_eq!(
            search_calls.load(Ordering::SeqCst),
            if mode == WebMode::Fast { 1 } else { queries }
        );
        let hits = site.hits.lock().unwrap().clone();
        assert!(hits.keys().all(|p| !p.starts_with("level3/")));
        assert_eq!(
            hits.get("robots.txt").copied().unwrap_or(0),
            usize::from(mode != WebMode::Fast)
        );
        if mode == WebMode::Deep {
            assert!(
                result.gathered.texts.iter().any(|t| t
                    .source
                    .as_deref()
                    .unwrap()
                    .contains("level2/"))
            );
        }
        drop(sink);
        events.await.unwrap();
        server.abort();
    }
}
#[tokio::test]
async fn sublinks_filter_duplicates_files_zero_scores_and_robots() {
    let (base, site, server) = server(true, false, None).await;
    let config = Config::default();
    let (web, _) = setup(&config, &base, driver(false));
    let (sink, _rx, events) = channel(&config, None);
    let (_control, cancel) = watch::channel(false);
    let model = output_model();
    let result = web
        .gather(
            WebInput {
                mode: WebMode::Medium,
                question: "rust memory",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel,
        )
        .await;
    assert!(result.error.is_none());
    let hits = site.hits.lock().unwrap().clone();
    assert_eq!(hits.get("robots.txt"), Some(&1));
    assert!(!hits.contains_key("blocked"));
    assert!(!hits.contains_key("download.PDF"));
    assert!(!hits.contains_key("noise"));
    assert_eq!(hits.get("root/0"), Some(&1));
    assert!(result.gathered.pages_read <= 10);
    drop(sink);
    events.await.unwrap();
    server.abort();
}
#[tokio::test]
async fn time_limit_keeps_partial_pages() {
    let (base, site, server) = server(false, true, None).await;
    let config = Config {
        medium_web_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let (web, _) = setup(&config, &base, driver(false));
    let (sink, _rx, events) = channel(&config, None);
    let (_control, cancel) = watch::channel(false);
    let model = output_model();
    let result = web
        .gather(
            WebInput {
                mode: WebMode::Medium,
                question: "rust memory",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel,
        )
        .await;
    assert!(result.error.is_none());
    assert!(!result.gathered.texts.is_empty());
    assert!(
        result
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == 1026)
    );
    assert_eq!(result.calls.len(), 1);
    assert!(
        !site
            .hits
            .lock()
            .unwrap()
            .keys()
            .any(|p| p.starts_with("level1/"))
    );
    drop(sink);
    events.await.unwrap();
    server.abort();
}
#[tokio::test]
async fn query_deadline_and_user_cancel_preserve_attempt_usage() {
    let (base, _site, server) = server(false, false, None).await;
    let config = Config {
        medium_web_timeout: Duration::from_millis(30),
        ..Default::default()
    };
    let (web, _) = setup(&config, &base, driver(true));
    let (sink, _rx, events) = channel(&config, None);
    let (_control, cancel) = watch::channel(false);
    let model = output_model();
    let result = web
        .gather(
            WebInput {
                mode: WebMode::Medium,
                question: "rust memory",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel,
        )
        .await;
    assert!(result.error.is_none());
    assert_eq!(result.calls.len(), 1);
    assert!(
        result
            .gathered
            .notices
            .iter()
            .any(|n| n.process.number == 1026)
    );
    let slow = driver(true);
    let entered = slow.entered.clone();
    let (web, _) = setup(&Config::default(), &base, slow);
    let (control, cancel) = watch::channel(false);
    let child_sink = sink.clone();
    let work = tokio::spawn(async move {
        let model = output_model();
        web.gather(
            WebInput {
                mode: WebMode::Deep,
                question: "rust memory",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &child_sink,
            cancel,
        )
        .await
    });
    entered.acquire().await.unwrap().forget();
    control.send(true).unwrap();
    let result = work.await.unwrap();
    assert_eq!(result.error, Some(DriverError::Cancelled));
    assert_eq!(result.calls.len(), 1);
    drop(sink);
    events.await.unwrap();
    server.abort();
}
#[tokio::test]
async fn cross_origin_sublinks_are_allowed() {
    let (remote, remote_site, remote_server) = server(false, false, None).await;
    let (base, _site, server) = server(false, false, Some(remote)).await;
    let config = Config::default();
    let (web, _) = setup(&config, &base, driver(false));
    let (sink, _rx, events) = channel(&config, None);
    let (_control, cancel) = watch::channel(false);
    let model = output_model();
    let result = web
        .gather(
            WebInput {
                mode: WebMode::Medium,
                question: "rust memory",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel,
        )
        .await;
    assert!(result.error.is_none());
    assert_eq!(
        remote_site.hits.lock().unwrap().get("level1/remote"),
        Some(&1)
    );
    assert_eq!(remote_site.hits.lock().unwrap().get("robots.txt"), Some(&1));
    drop(sink);
    events.await.unwrap();
    server.abort();
    remote_server.abort();
}
