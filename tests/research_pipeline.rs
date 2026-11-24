use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use cortex_api::core::config::Config;
use cortex_api::dal::{
    local::{
        chats::Chats,
        database::{self, Target},
        research_sources::{ResearchCatalogSnapshot, ResearchSource, ResearchSourceStore},
    },
    remote::{
        cache::RedisCache, event_journal::EventJournal, hippocampus::Hippocampus,
        openalex::OpenAlexClient, reddit::RedditClient, runs::RunStore, timing::TimingStore,
        web_page::PageReader,
    },
};
use cortex_api::domain::{
    models::{
        chat_runtime::{Turn, TurnRequest},
        chat_session::NewChat,
        driver::{CallInput, CallOutcome, DriverError, DriverEvent, ModelDriver, OutputModel},
        identity::Caller,
        run_record::{ModelCall, WebMode},
        search::{SearchAdapter, SearchFailure, SearchHit},
        security::{ClassifierUnavailable, InjectionClassifier},
        usage::Pending,
        web::WebInput,
    },
    services::{
        chat_runtime::ChatRuntime, community_research::CommunityResearchService,
        crawler::WebRetriever, events::channel, exchange_memory::ExchangeMemories,
        extraction::DocumentExtractor, files::FileProxy, history::History,
        openalex::OpenAlexResearchService, recall::MemoryRecall,
        research_catalog::ResearchCatalogService, research_limits::CoverageReport,
        research_router::EvidenceStage, run_evidence::RunEvidence, search::WebSearch,
        security::SecurityShield, temporary::TemporaryChats, thinking::Thinking,
        transcription::AudioTranscriber, web::ResearchProviders,
    },
};

struct SafeClassifier;
impl InjectionClassifier for SafeClassifier {
    fn probability(&self, _: &str) -> Result<f32, ClassifierUnavailable> {
        Ok(0.01)
    }
}

struct NoAudio;
impl AudioTranscriber for NoAudio {
    fn transcribe<'a>(
        &'a self,
        _: &'a Caller,
        _: &'a str,
        _: Arc<Vec<u8>>,
        _: &'a cortex_api::domain::services::events::EventSink,
    ) -> Pending<'a, Result<String, cortex_api::core::process_numbers::ProcessNumber>> {
        Box::pin(async { Err(cortex_api::core::process_numbers::AUDIO_FAILED) })
    }
}

#[derive(Default)]
struct TestModelDriver {
    answer_prompts: Mutex<Vec<String>>,
}
impl ModelDriver for TestModelDriver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        _: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            let now = chrono::Utc::now();
            let text = if input.role == "queries" {
                "machine learning paper".to_string()
            } else {
                self.answer_prompts
                    .lock()
                    .unwrap()
                    .push(input.prompt.clone());
                let _ = events.send(DriverEvent::Token {
                    role: input.role.clone(),
                    text: "Verified answer.".into(),
                });
                "Verified answer.".to_string()
            };
            Ok(CallOutcome {
                inspection: None,
                text,
                usage: ModelCall {
                    images: None,
                    audio: None,
                    role: input.role,
                    model: output.name(&Config::default()).into(),
                    input_tokens: 10,
                    output_tokens: 4,
                    time_to_first_token_ms: Some(1),
                    total_ms: 2,
                    started_at: now,
                    finished_at: now,
                },
                error: None,
                load_ms: 0,
                tokens_per_second: None,
                tool_request: None,
            })
        })
    }
}

struct MockSearch {
    hits: Vec<SearchHit>,
}
impl SearchAdapter for MockSearch {
    fn name(&self) -> &'static str {
        "mock-search"
    }
    fn search<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async { Ok(self.hits.clone()) })
    }
}

struct Catalog200Store {
    snapshot: ResearchCatalogSnapshot,
}
impl ResearchSourceStore for Catalog200Store {
    fn load_catalog(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResearchCatalogSnapshot, String>> + Send + '_>> {
        let snap = self.snapshot.clone();
        Box::pin(async move { Ok(snap) })
    }
}

fn build_200_catalog() -> ResearchCatalogService {
    let mut sources = vec![
        ResearchSource {
            host: "openalex.org".into(),
            name: "OpenAlex".into(),
            url: "https://openalex.org/".into(),
            description: "Scholarly academic paper discovery".into(),
            languages: "en,th".into(),
            regions: "Global".into(),
            access_note: "API key recommended".into(),
            access_mode: "credentialed_api".into(),
            is_enabled: true,
            min_spacing_ms: 5,
            max_requests_per_run: 3,
            subjects: vec![
                "reference-and-scholarly-discovery".into(),
                "mathematics-statistics-and-machine-learning".into(),
            ],
        },
        ResearchSource {
            host: "reddit.com".into(),
            name: "Reddit".into(),
            url: "https://www.reddit.com/".into(),
            description: "Community discussion".into(),
            languages: "en,th".into(),
            regions: "Global".into(),
            access_note: "OAuth Data API".into(),
            access_mode: "credentialed_api".into(),
            is_enabled: true,
            min_spacing_ms: 5,
            max_requests_per_run: 3,
            subjects: vec!["regional-blogs-communities-and-practical-experience".into()],
        },
        ResearchSource {
            host: "pantip.com".into(),
            name: "Pantip".into(),
            url: "https://pantip.com/".into(),
            description: "Thai community discussion".into(),
            languages: "th".into(),
            regions: "Asia".into(),
            access_note: "No documented public API confirmed".into(),
            access_mode: "public_page".into(),
            is_enabled: true,
            min_spacing_ms: 5,
            max_requests_per_run: 2,
            subjects: vec!["regional-blogs-communities-and-practical-experience".into()],
        },
    ];
    for i in 3..200 {
        sources.push(ResearchSource {
            host: format!("catalog-{i}.example"),
            name: format!("Catalog {i}"),
            url: format!("https://catalog-{i}.example/"),
            description: "General reference".into(),
            languages: "en".into(),
            regions: "Global".into(),
            access_note: "Public".into(),
            access_mode: "public_page".into(),
            is_enabled: true,
            min_spacing_ms: 5,
            max_requests_per_run: 2,
            subjects: vec!["history-language-and-humanities".into()],
        });
    }
    ResearchCatalogService::new(Arc::new(Catalog200Store {
        snapshot: ResearchCatalogSnapshot {
            sources,
            settings: BTreeMap::new(),
        },
    }))
}

async fn mock_research_servers() -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let oa_calls = Arc::new(AtomicUsize::new(0));
    let reddit_calls = Arc::new(AtomicUsize::new(0));
    let oa_c = oa_calls.clone();
    let rd_c = reddit_calls.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let oa_url = format!("{base}/oa/fulltext");

    let app = Router::new()
        .route(
            "/works",
            get(move || {
                let oa_c = oa_c.clone();
                let oa_url = oa_url.clone();
                async move {
                    oa_c.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "results": [
                            {
                                "id": "https://openalex.org/W100",
                                "doi": "https://doi.org/10.1000/oa1",
                                "title": "Readable OpenAlex Paper",
                                "publication_year": 2026,
                                "open_access": { "is_oa": true, "oa_url": oa_url },
                                "abstract_inverted_index": { "Machine": [0], "learning": [1] }
                            },
                            {
                                "id": "https://openalex.org/W200",
                                "doi": "https://doi.org/10.1000/closed2",
                                "title": "Closed Paper Metadata Only",
                                "publication_year": 2025,
                                "open_access": { "is_oa": false, "oa_url": null },
                                "abstract_inverted_index": { "Closed": [0], "abstract": [1] }
                            }
                        ]
                    }))
                }
            }),
        )
        .route(
            "/oa/fulltext",
            get(|| async {
                "Full paper body: machine learning safety and gradient verification details."
            }),
        )
        .route(
            "/api/v1/access_token",
            post(|| async { Json(json!({ "access_token": "tok_mock" })) }),
        )
        .route(
            "/search",
            get(move || {
                let rd_c = rd_c.clone();
                async move {
                    rd_c.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "data": {
                            "children": [
                                {
                                    "data": {
                                        "id": "p1",
                                        "subreddit": "MachineLearning",
                                        "title": "Community benchmark thread",
                                        "permalink": "/r/MachineLearning/comments/p1/bench/",
                                        "score": 99,
                                        "num_comments": 12,
                                        "selftext": "Community experience with machine learning benchmarks."
                                    }
                                }
                            ]
                        }
                    }))
                }
            }),
        )
        .route(
            "/page/read",
            get(|| async {
                (
                    [("content-type", "text/html")],
                    format!(
                        "<html><head><title>Web Article</title></head><body><article><h1>Web Article</h1><p>{}</p></article></body></html>",
                        "Machine learning paper summary and benchmark results. ".repeat(25)
                    ),
                )
            }),
        )
        .route(
            "/page/unread-404",
            get(|| async { (StatusCode::NOT_FOUND, "missing") }),
        );

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, oa_calls, reddit_calls, handle)
}

#[tokio::test]
async fn real_research_entry_point_routes_subset_calls_openalex_and_reddit_and_cites_only_read_with_labels()
 {
    let (base, oa_calls, reddit_calls, srv) = mock_research_servers().await;
    let config = Config {
        research_host_spacing: Duration::from_millis(1),
        ..Default::default()
    };
    let catalog = build_200_catalog();
    let oa = Arc::new(OpenAlexResearchService::new(
        OpenAlexClient::new(&base, Some("secret-oa-key-never-logged".into()), None).unwrap(),
    ));
    let rd = Arc::new(CommunityResearchService::new(
        RedditClient::new(&base, &base, "cid", "csec", "asodya/1.0").unwrap(),
    ));
    let search = WebSearch::with_adapters(
        &config,
        vec![Arc::new(MockSearch {
            hits: vec![
                SearchHit {
                    title: "Read Web Page".into(),
                    url: format!("{base}/page/read"),
                },
                SearchHit {
                    title: "Unread Broken Link".into(),
                    url: format!("{base}/page/unread-404"),
                },
            ],
        })],
    );
    let retriever = WebRetriever::new(
        &config,
        search,
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(TestModelDriver::default()),
    )
    .with_providers(ResearchProviders {
        catalog: Some(catalog),
        openalex: Some(oa),
        community: Some(rd),
    });

    let (sink, _rx, events) = channel(&config, None);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let model = OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("alice".into()),
            is_admin: false,
        },
        Uuid::now_v7(),
        false,
    );

    // 1. Scholarly question selects subset including openalex.org, never all 200 sites, never Reddit
    let academic = retriever
        .gather(
            WebInput {
                mode: WebMode::Medium,
                question: "Find an academic paper on machine learning",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert!(!academic.gathered.selected_hosts.is_empty());
    assert!(academic.gathered.selected_hosts.len() <= 6);
    assert!(
        academic
            .gathered
            .selected_hosts
            .contains(&"openalex.org".to_string())
    );
    assert_eq!(oa_calls.load(Ordering::SeqCst), 1);
    assert_eq!(reddit_calls.load(Ordering::SeqCst), 0);

    // Citations exclude FoundLink (/page/unread-404) and MetadataOnly (Closed Paper),
    // and include ONLY read/used items with stage labels
    assert!(
        academic
            .gathered
            .evidence
            .iter()
            .any(|e| e.stage == EvidenceStage::MetadataOnly
                && e.title == "Closed Paper Metadata Only")
    );
    assert!(
        academic
            .gathered
            .evidence
            .iter()
            .any(|e| e.stage == EvidenceStage::FoundLink
                && e.canonical_url.ends_with("/page/unread-404"))
    );
    for cited in &academic.gathered.sources {
        assert!(!cited.url.contains("unread-404"));
        assert!(!cited.url.contains("closed2"));
        assert!(cited.title.starts_with('['));
    }
    assert!(
        academic
            .gathered
            .sources
            .iter()
            .any(|s| s.url.contains("/oa/fulltext"))
    );

    // 2. Community question triggers Reddit, rejects Pantip scraping without allowed path
    let community = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: "What are reddit and pantip community opinions on Rust?",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx,
        )
        .await;
    assert_eq!(reddit_calls.load(Ordering::SeqCst), 1);
    assert!(
        community
            .gathered
            .sources
            .iter()
            .any(|s| s.title.contains("community_evidence: reddit"))
    );
    assert!(matches!(
        community.gathered.coverage,
        CoverageReport::Partial { ref skipped_hosts, .. } if skipped_hosts.contains(&"pantip.com".to_string())
    ));

    drop(sink);
    events.await.unwrap();
    srv.abort();
}

#[tokio::test]
#[ignore = "requires cortex_api_test and Valkey"]
async fn chat_runtime_end_to_end_uses_catalog_router_and_saves_only_labeled_read_citations() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_research_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    let (base, oa_calls, _rd_calls, srv) = mock_research_servers().await;
    let listener_h = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hippo_addr = format!("http://{}", listener_h.local_addr().unwrap());
    let hippo_srv = tokio::spawn(async move {
        axum::serve(
            listener_h,
            Router::new()
                .route(
                    "/api/v1/entities",
                    get(|| async { Json(json!({"data":[]})) }),
                )
                .route(
                    "/api/v1/memories",
                    post(|| async { Json(json!({"data":{"id":"mem-1"}})) }),
                ),
        )
        .await
        .unwrap();
    });

    let config = Config {
        hippocampus_url: hippo_addr,
        research_host_spacing: Duration::from_millis(1),
        ..Default::default()
    };
    let Some((_, cache)) = RedisCache::from_test_environment() else {
        eprintln!("Skipping research_pipeline test: CORTEX_TEST_REDIS_URL is not set");
        return;
    };
    let store = Arc::new(
        RunStore::new(
            &config,
            reqwest::Url::parse("http://127.0.0.1:1/runs").unwrap(),
            "u".into(),
            "p".into(),
        )
        .unwrap(),
    );
    let chats = Chats { pool: pool.clone() };
    let hippocampus = Hippocampus::new(&config)
        .unwrap()
        .with_overview_cache(cache.clone());
    let shield = Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap());
    let driver = Arc::new(TestModelDriver::default());
    let temporary = TemporaryChats::new(&config, cache.clone());
    let personas = cortex_api::domain::services::personas::Personas {
        hippocampus: hippocampus.clone(),
        shield: shield.clone(),
    };
    let oa = Arc::new(OpenAlexResearchService::new(
        OpenAlexClient::new(&base, None, None).unwrap(),
    ));
    let rd = Arc::new(CommunityResearchService::new(
        RedditClient::new(&base, &base, "cid", "csec", "asodya/1.0").unwrap(),
    ));
    let web = WebRetriever::new(
        &config,
        WebSearch::with_adapters(
            &config,
            vec![Arc::new(MockSearch {
                hits: vec![SearchHit {
                    title: "Unread Broken".into(),
                    url: format!("{base}/page/unread-404"),
                }],
            })],
        ),
        PageReader::new(&config).unwrap(),
        shield.clone(),
        driver.clone(),
    )
    .with_providers(ResearchProviders {
        catalog: Some(build_200_catalog()),
        openalex: Some(oa),
        community: Some(rd),
    });

    let runtime = Arc::new(ChatRuntime {
        history: History::new(chats.clone(), cache.clone(), &config).unwrap(),
        files: FileProxy {
            hippocampus: hippocampus.clone(),
            extractor: DocumentExtractor::new(&config),
            transcriber: Arc::new(NoAudio),
            temporary: temporary.clone(),
            config: config.clone(),
        },
        temporary,
        recall: MemoryRecall::new(&config, hippocampus.clone(), shield.clone()),
        web,
        thinking: Thinking::new(&config, driver.clone()),
        memories: ExchangeMemories {
            repo: chats.clone(),
            hippocampus,
        },
        runs: RunEvidence::new(store.clone()),
        telemetry: cortex_api::domain::services::telemetry_reads::TelemetryReads {
            store,
            timing: TimingStore::new(&config, cache.clone()),
            config: config.clone(),
        },
        journal: EventJournal::new(cache, &config).unwrap(),
        controls: Default::default(),
        personas,
        shield,
        chats: chats.clone(),
        config: config.clone(),
    });

    let caller = Caller {
        app_id: "cortex".into(),
        user_id: Some("alice".into()),
        is_admin: false,
    };
    let req: NewChat = serde_json::from_value(json!({
        "app_id": "cortex",
        "text": "Summarize academic machine learning research paper",
        "needs_web": true,
        "web_mode": "medium",
        "use_memory": false,
        "thinking": false
    }))
    .unwrap();
    let finished = runtime
        .start(Turn::new(caller.clone(), TurnRequest::New(req)), None)
        .await
        .unwrap();
    let out = finished.output.unwrap();
    assert_eq!(oa_calls.load(Ordering::SeqCst), 1);
    assert!(!out.sources.is_empty());
    assert!(out.sources.iter().all(|s| s.title.starts_with('[')));
    assert!(out.sources.iter().all(|s| !s.url.contains("unread-404")));
    assert!(out.sources.iter().all(|s| !s.url.contains("closed2")));

    let saved_sources = chats
        .sources(&caller.storage_scope(), finished.ids.unwrap().output_id)
        .await
        .unwrap();
    assert_eq!(saved_sources.len(), out.sources.len());

    srv.abort();
    hippo_srv.abort();
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires live CORTEX_OPENALEX_API_KEY, CORTEX_REDDIT_*, and network"]
async fn live_authenticated_openalex_and_reddit_through_web_retriever() {
    dotenvy::dotenv().ok();
    let config = Config {
        research_host_spacing: Duration::from_millis(50),
        ..Default::default()
    };
    let oa = Arc::new(OpenAlexResearchService::new(
        OpenAlexClient::from_env().unwrap(),
    ));
    let rd = Arc::new(CommunityResearchService::new(
        RedditClient::from_env().unwrap(),
    ));
    let retriever = WebRetriever::new(
        &config,
        WebSearch::with_adapters(&config, vec![Arc::new(NoSearch)]),
        PageReader::new(&config).unwrap(),
        Arc::new(SecurityShield::new(&config, Arc::new(SafeClassifier)).unwrap()),
        Arc::new(TestModelDriver::default()),
    )
    .with_providers(ResearchProviders {
        catalog: Some(build_200_catalog()),
        openalex: Some(oa),
        community: Some(rd),
    });

    let (sink, _rx, events) = channel(&config, None);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let model = OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("live-check".into()),
            is_admin: false,
        },
        Uuid::now_v7(),
        false,
    );

    let res_oa = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: "Find an academic paper on neural networks",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx.clone(),
        )
        .await;
    assert!(
        !res_oa.gathered.evidence.is_empty(),
        "Live OpenAlex returned zero evidence items"
    );

    let res_rd = retriever
        .gather(
            WebInput {
                mode: WebMode::Fast,
                question: "What are reddit community opinions on Rust async?",
                needs_web: true,
                model: &model,
                images: vec![],
            },
            &sink,
            cancel_rx,
        )
        .await;
    assert!(
        !res_rd.gathered.sources.is_empty(),
        "Live Reddit returned zero sources (selected={:?}, coverage={:?}, evidence={:?})",
        res_rd.gathered.selected_hosts,
        res_rd.gathered.coverage,
        res_rd.gathered.evidence
    );
    assert!(
        res_rd
            .gathered
            .sources
            .iter()
            .any(|s| s.title.contains("community_evidence: reddit"))
    );

    drop(sink);
    events.await.unwrap();
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
