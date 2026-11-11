use cortex_api::core::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let production = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [mode] if mode == "--production" => true,
        _ => return Err("Use cortex-api [--production]".into()),
    };
    let config = if production {
        Config::production()
    } else {
        Config::default()
    };
    let logs = cortex_api::core::logs::init(&config);
    use cortex_api::dal::remote::runs::{RunDatabase, RunStore};
    // Store selection follows the explicit launch mode, never a secret value.
    let store = std::sync::Arc::new(
        RunStore::from_environment(if production {
            RunDatabase::Primary
        } else {
            RunDatabase::Test
        })
        .map_err(|_| "The run record store is not configured")?,
    );
    if store.initialize().await.is_err() {
        if production {
            return Err("The production run database is unavailable".into());
        }
        tracing::warn!("Run record database setup failed");
    }
    let _retention = cortex_api::domain::tasks::run_retention::spawn(store.clone(), config.clone());
    let loyalty_cache = cortex_api::dal::remote::cache::RedisCache::from_environment();
    let _loyalty =
        match cortex_api::dal::remote::loyalty::SupabaseLoyalty::from_environment(&config) {
            Ok(source) => Some(cortex_api::domain::tasks::loyalty::spawn(
                std::sync::Arc::new(source),
                loyalty_cache.clone(),
                config.clone(),
            )),
            Err(_) => {
                tracing::warn!("Loyalty source is not configured; base weight remains available");
                None
            }
        };
    let ledger = std::sync::Arc::new(cortex_api::dal::remote::usage::RedisLedger::new(
        loyalty_cache,
        store.clone(),
        config.local_queue_name.into(),
    ));
    let chat_pool = cortex_api::dal::local::database::connect(if production {
        cortex_api::dal::local::database::Target::Primary
    } else {
        cortex_api::dal::local::database::Target::Test
    })
    .await?;
    let hippocampus = cortex_api::dal::remote::hippocampus::Hippocampus::new(&config)
        .map_err(|_| "The Hippocampus URL is not valid")?;
    let (runtime, personas) = {
        use cortex_api::{
            dal::{
                local::{
                    chats::Chats,
                    injection_classifier::{MissingClassifier, OnnxInjectionClassifier},
                },
                remote::{
                    cache::RedisCache, event_journal::EventJournal, ollama::Ollama,
                    web_page::PageReader,
                },
            },
            domain::{
                models::security::InjectionClassifier,
                services::{
                    balancer::Scheduler, chat_runtime::ChatRuntime, crawler::WebRetriever,
                    exchange_memory::ExchangeMemories, extraction::DocumentExtractor,
                    files::FileProxy, history::History, recall::MemoryRecall,
                    run_evidence::RunEvidence, search::WebSearch, security::SecurityShield,
                    temporary::TemporaryChats, thinking::Thinking,
                },
            },
        };
        let classifier: std::sync::Arc<dyn InjectionClassifier> =
            match OnnxInjectionClassifier::load(&config) {
                Ok(guard) => std::sync::Arc::new(guard),
                Err(_) => {
                    tracing::warn!("Injection classifier is not available; every input is refused");
                    std::sync::Arc::new(MissingClassifier)
                }
            };
        let shield = std::sync::Arc::new(
            SecurityShield::new(&config, classifier)
                .map_err(|_| "The shield config is not valid")?,
        );
        let queue = Scheduler::new(ledger.clone());
        let ollama = std::sync::Arc::new(Ollama::new(config.clone(), queue.clone()));
        let driver: std::sync::Arc<dyn cortex_api::domain::models::driver::ModelDriver> =
            ollama.clone();
        let chats = Chats {
            pool: chat_pool.clone(),
        };
        let temporary = TemporaryChats::new(&config, RedisCache::from_environment());
        let personas = cortex_api::domain::services::personas::Personas {
            hippocampus: hippocampus.clone(),
            shield: shield.clone(),
        };
        let runtime = std::sync::Arc::new(ChatRuntime {
            history: History::new(chats.clone(), RedisCache::from_environment(), &config)
                .map_err(|_| "The history cache config is not valid")?,
            files: FileProxy {
                hippocampus: hippocampus.clone(),
                extractor: DocumentExtractor::new(&config),
                transcriber: match &config.transcribe_location {
                    cortex_api::core::config::JobLocation::Local => std::sync::Arc::new(
                        cortex_api::domain::services::transcription::LocalTranscriber {
                            queue: queue.clone(),
                            ollama: ollama.clone(),
                            config: config.clone(),
                        },
                    )
                        as std::sync::Arc<
                            dyn cortex_api::domain::services::transcription::AudioTranscriber,
                        >,
                    cortex_api::core::config::JobLocation::Remote(url) => std::sync::Arc::new(
                        cortex_api::domain::services::transcription::RemoteTranscriber {
                            worker: cortex_api::dal::remote::file_worker::FileWorker::new(
                                &config, url,
                            ),
                            queue: queue.clone(),
                        },
                    ),
                },
                temporary: temporary.clone(),
                config: config.clone(),
            },
            temporary,
            recall: MemoryRecall::new(&config, hippocampus.clone(), shield.clone()),
            web: WebRetriever::new(
                &config,
                WebSearch::new(&config).map_err(|_| "The web search config is not valid")?,
                PageReader::new(&config).map_err(|_| "The web page reader is not valid")?,
                shield.clone(),
                driver.clone(),
            ),
            thinking: Thinking::new(&config, driver),
            memories: ExchangeMemories {
                repo: chats.clone(),
                hippocampus: hippocampus.clone(),
            },
            runs: RunEvidence::new(store.clone()),
            telemetry: cortex_api::domain::services::telemetry_reads::TelemetryReads {
                store: store.clone(),
                timing: cortex_api::dal::remote::timing::TimingStore::new(
                    &config,
                    RedisCache::from_environment(),
                ),
                config: config.clone(),
            },
            journal: EventJournal::new(RedisCache::from_environment(), &config)
                .map_err(|_| "The event journal config is not valid")?,
            controls: Default::default(),
            personas: personas.clone(),
            shield,
            chats,
            config: config.clone(),
        });
        (runtime, personas)
    };
    let files = runtime.files.clone();
    let routes = cortex_api::presentation::models::router(config.clone())
        .merge(cortex_api::presentation::chat_inputs::router(runtime))
        .merge(cortex_api::presentation::personas::router(personas))
        .merge(cortex_api::presentation::chat_delete::router(
            cortex_api::domain::services::chat_delete::ChatDelete {
                history: cortex_api::domain::services::chat_history::ChatHistory {
                    repo: cortex_api::dal::local::chats::Chats {
                        pool: chat_pool.clone(),
                    },
                    config: config.clone(),
                    hippocampus: hippocampus.clone(),
                },
                hippocampus: hippocampus.clone(),
            },
        ))
        .merge(cortex_api::presentation::telemetry_reads::router(
            cortex_api::domain::services::telemetry_reads::TelemetryReads {
                store,
                timing: cortex_api::dal::remote::timing::TimingStore::new(
                    &config,
                    cortex_api::dal::remote::cache::RedisCache::from_environment(),
                ),
                config: config.clone(),
            },
        ))
        .merge(cortex_api::presentation::memory_graph::router(
            cortex_api::domain::services::memory_graph::MemoryGraph {
                hippocampus: hippocampus.clone(),
                config: config.clone(),
            },
        ))
        .merge(cortex_api::presentation::files::router(files))
        .merge(cortex_api::presentation::logs::router(logs))
        .merge(cortex_api::presentation::telemetry::router(ledger, &config))
        .merge(cortex_api::presentation::chats::router(
            cortex_api::dal::local::chats::Chats { pool: chat_pool },
            config.clone(),
        ));
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "Server started");
    axum::serve(
        listener,
        cortex_api::presentation::with_response_contract(routes)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
