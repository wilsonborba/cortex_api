use cortex_api::core::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let config = Config::default();
    let logs = cortex_api::core::logs::init(&config);
    use cortex_api::dal::remote::runs::{RunDatabase, RunStore};
    // The night-run development API uses test stores. Cutover selects primary stores.
    let store = std::sync::Arc::new(
        RunStore::from_environment(RunDatabase::Test)
            .map_err(|_| "The run record store is not configured")?,
    );
    if store.initialize().await.is_err() {
        tracing::warn!("Run record database setup failed");
    }
    let ledger = std::sync::Arc::new(cortex_api::dal::remote::usage::RedisLedger::new(
        cortex_api::dal::remote::cache::RedisCache::from_environment(),
        store,
        config.local_queue_name.into(),
    ));
    let chat_pool =
        cortex_api::dal::local::database::connect(cortex_api::dal::local::database::Target::Test)
            .await?;
    let routes = cortex_api::presentation::models::router(config.clone())
        .merge(cortex_api::presentation::logs::router(logs))
        .merge(cortex_api::presentation::telemetry::router(ledger, &config))
        .merge(cortex_api::presentation::chats::router(
            cortex_api::dal::local::chats::Chats { pool: chat_pool },
            config.clone(),
        ));
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "Development server started");
    axum::serve(
        listener,
        cortex_api::presentation::with_response_contract(routes)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
