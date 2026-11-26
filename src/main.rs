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
        .map_err(|_| "The Hippocampus URL is not valid")?
        .with_overview_cache(cortex_api::dal::remote::cache::RedisCache::from_environment());
    let (runtime, personas, agent_runs) = {
        use cortex_api::{
            dal::{
                local::{
                    chats::Chats,
                    injection_classifier::{MissingClassifier, OnnxInjectionClassifier},
                    research_sources::PostgresResearchSourceStore,
                },
                remote::{
                    cache::RedisCache, event_journal::EventJournal, ollama::Ollama,
                    openalex::OpenAlexClient, reddit::RedditClient, web_page::PageReader,
                },
            },
            domain::{
                models::security::InjectionClassifier,
                services::{
                    balancer::Scheduler,
                    chat_runtime::ChatRuntime,
                    community_research::CommunityResearchService,
                    crawler::WebRetriever,
                    exchange_memory::ExchangeMemories,
                    extraction::DocumentExtractor,
                    files::FileProxy,
                    history::History,
                    openalex::OpenAlexResearchService,
                    recall::MemoryRecall,
                    research_catalog::ResearchCatalogService,
                    research_limits::{
                        ResearchPageCache, ResearchPolitenessGuard, ValkeyLimitStore,
                    },
                    run_evidence::RunEvidence,
                    search::WebSearch,
                    security::SecurityShield,
                    temporary::TemporaryChats,
                    thinking::Thinking,
                    web::ResearchProviders,
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
        let prism_supervisor =
            std::sync::Arc::new(cortex_api::dal::remote::prism_supervisor::PrismSupervisor::new());
        let ollama = std::sync::Arc::new(
            Ollama::new(config.clone(), queue.clone())
                .with_prism_supervisor(prism_supervisor.clone()),
        );
        let prism = std::sync::Arc::new(cortex_api::dal::remote::prism::Prism::managed(
            config.clone(),
            queue.clone(),
            prism_supervisor,
        ));
        let driver: std::sync::Arc<dyn cortex_api::domain::models::driver::ModelDriver> =
            std::sync::Arc::new(cortex_api::dal::remote::model_router::ModelRouter::new(
                ollama.clone(),
                prism,
                queue.clone(),
            ));
        let chats = Chats {
            pool: chat_pool.clone(),
        };
        let temporary = TemporaryChats::new(&config, RedisCache::from_environment());
        let personas = cortex_api::domain::services::personas::Personas {
            hippocampus: hippocampus.clone(),
            shield: shield.clone(),
        };
        let agent_runs = cortex_api::domain::services::agent_runs::AgentRunService::new(
            std::sync::Arc::new(
                cortex_api::dal::local::agent_runs::PostgresAgentRunStore::new(chat_pool.clone()),
            ),
            120_000,
            4,
            2,
            8192,
        );
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
            web: {
                let catalog = ResearchCatalogService::new(std::sync::Arc::new(
                    PostgresResearchSourceStore::new(chat_pool.clone()),
                ));
                let openalex = OpenAlexClient::from_env()
                    .ok()
                    .map(|c| std::sync::Arc::new(OpenAlexResearchService::new(c)));
                let community = RedditClient::from_env()
                    .ok()
                    .map(|c| std::sync::Arc::new(CommunityResearchService::new(c)));
                WebRetriever::new(
                    &config,
                    WebSearch::new(&config).map_err(|_| "The web search config is not valid")?,
                    PageReader::new(&config).map_err(|_| "The web page reader is not valid")?,
                    shield.clone(),
                    driver.clone(),
                )
                .with_politeness(
                    std::sync::Arc::new(ResearchPolitenessGuard::from_config(
                        &config,
                        std::sync::Arc::new(ValkeyLimitStore::new(RedisCache::from_environment())),
                    )),
                    std::sync::Arc::new(ResearchPageCache::new(
                        RedisCache::from_environment(),
                        config.research_cache_ttl,
                    )),
                )
                .with_providers(ResearchProviders {
                    catalog: Some(catalog),
                    openalex,
                    community,
                })
                .with_robots_on_direct_reads(true)
            },
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
            agent_runs: agent_runs.clone(),
            personas: personas.clone(),
            shield,
            chats,
            config: config.clone(),
        });
        (runtime, personas, agent_runs)
    };
    let remote_cli = cortex_api::domain::services::remote_cli::RemoteCliService::new(
        std::sync::Arc::new(
            cortex_api::dal::local::remote_cli::PostgresRemoteCliStore::new(chat_pool.clone()),
        ),
        cortex_api::dal::remote::cache::RedisCache::from_environment(),
        &config,
    );
    let _remote_cli_cleanup = {
        let remote_cli = remote_cli.clone();
        let command_ttl = config.remote_cli_command_ttl;
        tokio::spawn(async move {
            let period = (command_ttl / 2).max(std::time::Duration::from_secs(2));
            let mut timer = tokio::time::interval(period);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let _ = remote_cli.expire_pending_commands(now_ms).await;
                let _ = remote_cli.purge_expired_sessions(now_ms).await;
            }
        })
    };
    let app = cortex_api::presentation::app_router(cortex_api::presentation::AppServices {
        config: config.clone(),
        runtime,
        personas,
        chat_delete: cortex_api::domain::services::chat_delete::ChatDelete {
            history: cortex_api::domain::services::chat_history::ChatHistory {
                repo: cortex_api::dal::local::chats::Chats {
                    pool: chat_pool.clone(),
                },
                config: config.clone(),
                hippocampus: hippocampus.clone(),
            },
            hippocampus,
        },
        telemetry_reads: cortex_api::domain::services::telemetry_reads::TelemetryReads {
            store,
            timing: cortex_api::dal::remote::timing::TimingStore::new(
                &config,
                cortex_api::dal::remote::cache::RedisCache::from_environment(),
            ),
            config: config.clone(),
        },
        memory_graph: cortex_api::domain::services::memory_graph::MemoryGraph {
            hippocampus: cortex_api::dal::remote::hippocampus::Hippocampus::new(&config)
                .map_err(|_| "The Hippocampus URL is not valid")?
                .with_overview_cache(
                    cortex_api::dal::remote::cache::RedisCache::from_environment(),
                ),
            config: config.clone(),
            records: Some(cortex_api::dal::local::memory_records::MemoryRecords {
                pool: chat_pool.clone(),
            }),
            cache: cortex_api::dal::remote::cache::RedisCache::from_environment(),
        },
        chats: cortex_api::dal::local::chats::Chats {
            pool: chat_pool.clone(),
        },
        logs,
        ledger,
        remote_cli,
        agent_runs,
    });
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "Server started");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
